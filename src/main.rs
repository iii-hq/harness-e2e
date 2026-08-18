use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use harness_e2e::dashboard;
use harness_e2e::fault::{FaultEvaluation, FaultJournal, FaultPlan, FaultProfile};
use harness_e2e::judge::JudgeConfig;
use harness_e2e::report::E2eReport;
use harness_e2e::scenarios::{self, ScenarioId, ScenarioSuite};
use harness_e2e::suite::{run_suite, SubjectConfig, SuiteRunConfig};
use harness_e2e::wire::PermissionMode;

#[derive(Debug, Parser)]
#[command(name = "harness-e2e", about = "Run real-stack quality scenarios")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the code-defined scenario ids as a JSON array.
    List(ListArgs),
    /// Print one scenario's prompt and its declared assessments.
    Show(ShowArgs),
    /// List models registered in the running stack.
    Models(ModelsArgs),
    /// Execute one or more quality scenarios against a running stack.
    Run(RunArgs),
    /// Print a human-readable summary from a saved results.json.
    #[command(alias = "inspect")]
    Report(ReportArgs),
    /// Run E2E scenarios and compare local executions in a browser.
    #[command(alias = "serve")]
    Dashboard(dashboard::DashboardArgs),
    /// Materialize an immutable, deterministic fault plan for a protected supervisor.
    FaultPlan(FaultPlanArgs),
    /// Classify observed recovery from a protected supervisor's fault journal.
    FaultEvaluate(FaultEvaluateArgs),
}

#[derive(Debug, Args)]
struct ModelsArgs {
    #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
    url: String,

    /// Show models registered by only this provider.
    #[arg(long)]
    provider: Option<String>,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
    url: String,

    #[arg(long, env = "HARNESS_E2E_MODEL")]
    model: String,

    #[arg(long, env = "HARNESS_E2E_PROVIDER")]
    provider: String,

    #[arg(long, env = "HARNESS_E2E_JUDGE_MODEL")]
    judge_model: Option<String>,

    #[arg(long, env = "HARNESS_E2E_JUDGE_PROVIDER")]
    judge_provider: Option<String>,

    #[arg(long, env = "HARNESS_E2E_OUTPUT", default_value = "target/e2e")]
    output: PathBuf,

    /// Store this manual execution in a dashboard-compatible history directory.
    #[arg(long, env = "HARNESS_E2E_RUNS_DIR")]
    runs_dir: Option<PathBuf>,

    #[arg(long, default_value_t = 1)]
    runs: u32,

    /// Materialize deterministic scenario inputs from this seed.
    #[arg(long, env = "HARNESS_E2E_SEED")]
    seed: Option<u64>,

    /// Add a deterministic case seed. Repeat to run rotating cases alongside the fixed seed.
    #[arg(long = "rotating-seed")]
    rotating_seeds: Vec<u64>,

    /// Retry transient provider and transport failures, never hard-gate or resource failures.
    #[arg(long, env = "HARNESS_E2E_TECHNICAL_RETRIES")]
    technical_retries: Option<u8>,

    /// Emit a progress heartbeat while a scenario runs. Set to 0 to disable.
    #[arg(
        long,
        env = "HARNESS_E2E_PROGRESS_INTERVAL_SECONDS",
        default_value_t = 15
    )]
    progress_interval_seconds: u64,

    /// Run only the selected scenario. Repeat to select more than one.
    #[arg(long, value_enum)]
    scenario: Vec<ScenarioId>,

    /// Which suite to run when no scenario is named. Extended scenarios stay
    /// out of the default selection and are opted into per run.
    #[arg(long, value_enum, default_value_t = ScenarioSuite::Canonical)]
    suite: ScenarioSuite,

    /// Approval mode for the sessions this run owns. Scenario runs are
    /// unattended, so the default lifts them out of `manual`, where every
    /// function call waits for a human.
    #[arg(long, value_enum, default_value_t = PermissionMode::Full)]
    permission_mode: PermissionMode,
}

#[derive(Debug, Args)]
struct ShowArgs {
    /// The scenario to print.
    #[arg(long, value_enum)]
    scenario: ScenarioId,

    /// Run id the prompt is rendered for. Scenario prompts name per-run
    /// functions, scopes, and sessions, so a real run's ids differ from these.
    #[arg(long, default_value = "showcase")]
    run_id: String,
}

#[derive(Debug, Args)]
struct ListArgs {
    /// Print only the scenarios in this suite.
    #[arg(long, value_enum)]
    suite: Option<ScenarioSuite>,
}

#[derive(Debug, Args)]
struct ReportArgs {
    /// Path to results.json or to the directory containing it.
    input: PathBuf,

    /// Include passing gates and fully awarded criteria.
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Args)]
struct FaultPlanArgs {
    /// FaultProfile JSON.
    #[arg(long)]
    profile: PathBuf,

    /// Destination for the materialized FaultPlan JSON.
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct FaultEvaluateArgs {
    /// FaultProfile JSON.
    #[arg(long)]
    profile: PathBuf,

    /// Materialized FaultPlan JSON given to the protected supervisor.
    #[arg(long)]
    plan: PathBuf,

    /// FaultJournal JSON written by the protected supervisor.
    #[arg(long)]
    journal: PathBuf,

    /// Canonical results.json or its containing directory. Omit for cancellation drills.
    #[arg(long)]
    results: Option<PathBuf>,

    /// Destination for the FaultEvaluation JSON.
    #[arg(long)]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match Cli::parse().command {
        Command::List(args) => {
            let ids: Vec<_> = ScenarioId::ALL
                .iter()
                .filter(|scenario| args.suite.is_none_or(|suite| scenario.suite() == suite))
                .map(|scenario| scenario.as_str())
                .collect();
            println!("{}", serde_json::to_string(&ids)?);
            Ok(())
        }
        Command::Show(args) => {
            let spec = args.scenario.spec(&args.run_id);
            println!("# {} (version {})", spec.id, spec.version);
            println!(
                "# suite: {:?}; max_turns: {}; stuck_timeout: {}s",
                args.scenario.suite(),
                spec.execution.max_turns,
                spec.execution.stuck_timeout_seconds
            );
            for criterion in &spec.criteria {
                let policy = match criterion.policy {
                    harness_e2e::assessment::AssessmentPolicy::HardGate => "hard gate",
                    harness_e2e::assessment::AssessmentPolicy::Advisory => "advisory",
                };
                println!(
                    "# {} [{policy}, {} points]: {}",
                    criterion.id, criterion.weight, criterion.description
                );
            }
            println!();
            println!("{}", spec.prompt);
            Ok(())
        }
        Command::Models(args) => models(args).await,
        Command::Run(args) => run(args).await,
        Command::Report(args) => report(args),
        Command::Dashboard(args) => dashboard::serve(args).await,
        Command::FaultPlan(args) => fault_plan(args),
        Command::FaultEvaluate(args) => fault_evaluate(args),
    }
}

fn fault_plan(args: FaultPlanArgs) -> Result<()> {
    let profile = FaultProfile::read(&args.profile)?;
    let plan = profile.materialize()?;
    plan.write(&args.output)?;
    println!("{}", args.output.display());
    Ok(())
}

fn fault_evaluate(args: FaultEvaluateArgs) -> Result<()> {
    let profile = FaultProfile::read(&args.profile)?;
    let plan = FaultPlan::read(&args.plan)?;
    let journal = FaultJournal::read(&args.journal)?;
    let report = args
        .results
        .as_deref()
        .map(E2eReport::read_from)
        .transpose()?
        .map(|(report, _)| report);
    let evaluation = FaultEvaluation::evaluate(&profile, &plan, &journal, report.as_ref())?;
    evaluation.write(&args.output)?;
    println!("{}", args.output.display());
    if !evaluation.passed() {
        bail!(
            "fault recovery classified as {:?}",
            evaluation.classification
        );
    }
    Ok(())
}

async fn models(args: ModelsArgs) -> Result<()> {
    let context = harness_e2e::context::E2eContext::connect(&args.url)
        .await
        .context("connect to the iii stack")?;
    if !context.function_exists("harness::send").await? {
        bail!(
            "connected iii stack does not expose harness::send; verify --url points to the Harness stack"
        );
    }
    if !context.function_exists("router::models::list").await? {
        bail!(
            "connected Harness stack does not expose router::models::list; start its llm-router before loading models"
        );
    }
    let result = harness_e2e::catalog::list(&context, args.provider.as_deref()).await;
    context.shutdown().await;
    let models = result?;
    print!("{}", harness_e2e::catalog::summary(&models));
    Ok(())
}

fn report(args: ReportArgs) -> Result<()> {
    let (report, path) = E2eReport::read_from(&args.input)?;
    print!("{}", report.summary(args.verbose));
    println!("report: {}", path.display());
    Ok(())
}

async fn run(args: RunArgs) -> Result<()> {
    let selected_scenarios = scenarios::selected_in(&args.scenario, args.suite);
    let technical_retries = args.technical_retries.unwrap_or_else(|| {
        if selected_scenarios.iter().any(|scenario| {
            scenario.execution_kind() == scenarios::ScenarioExecutionKind::CompositeFlow
        }) {
            0
        } else {
            1
        }
    });
    let subject = SubjectConfig {
        model: args.model,
        provider: args.provider,
    };
    let execution_id = args
        .runs_dir
        .as_ref()
        .map(|_| uuid::Uuid::new_v4().simple().to_string());
    let output = args
        .runs_dir
        .as_ref()
        .zip(execution_id.as_ref())
        .map_or(args.output, |(runs_dir, execution_id)| {
            runs_dir.join(execution_id).join("results")
        });
    let judge = Some(judge_config(
        &subject,
        args.judge_model,
        args.judge_provider,
    ));
    let outcome = run_suite(SuiteRunConfig {
        permission_mode: args.permission_mode,
        url: args.url,
        execution_id,
        subject,
        judge,
        output,
        scenarios: selected_scenarios,
        runs: args.runs,
        seed: args.seed,
        rotating_seeds: args.rotating_seeds,
        technical_retries,
        progress_interval: (args.progress_interval_seconds > 0)
            .then(|| std::time::Duration::from_secs(args.progress_interval_seconds)),
        control: None,
    })
    .await
    .context("run E2E quality suite")?;

    print!("{}", outcome.report.summary(false));
    println!("report: {}", outcome.report_path.display());
    if !outcome.report.passed {
        bail!("E2E suite failed");
    }
    tracing::info!(path = %outcome.report_path.display(), "E2E quality suite passed");
    Ok(())
}

fn judge_config(
    subject: &SubjectConfig,
    model: Option<String>,
    provider: Option<String>,
) -> JudgeConfig {
    JudgeConfig {
        model: model.unwrap_or_else(|| subject.model.clone()),
        provider: provider.unwrap_or_else(|| subject.provider.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_subcommand_needs_no_model_configuration() {
        assert!(matches!(
            Cli::try_parse_from(["harness-e2e", "list"])
                .unwrap()
                .command,
            Command::List(ListArgs { suite: None })
        ));
    }

    #[test]
    fn list_and_run_accept_a_suite_selector() {
        let Command::List(args) =
            Cli::try_parse_from(["harness-e2e", "list", "--suite", "extended"])
                .unwrap()
                .command
        else {
            panic!("expected list command");
        };
        assert_eq!(args.suite, Some(ScenarioSuite::Extended));

        let Command::Run(args) = Cli::try_parse_from([
            "harness-e2e",
            "run",
            "--url",
            "ws://127.0.0.1:49134",
            "--model",
            "m",
            "--provider",
            "p",
            "--suite",
            "extended",
        ])
        .unwrap()
        .command
        else {
            panic!("expected run command");
        };
        assert_eq!(args.suite, ScenarioSuite::Extended);
        assert!(args.scenario.is_empty());
    }

    #[test]
    fn a_run_without_a_suite_selector_stays_canonical() {
        let Command::Run(args) = Cli::try_parse_from([
            "harness-e2e",
            "run",
            "--url",
            "ws://127.0.0.1:49134",
            "--model",
            "m",
            "--provider",
            "p",
        ])
        .unwrap()
        .command
        else {
            panic!("expected run command");
        };
        assert_eq!(args.suite, ScenarioSuite::Canonical);
    }

    #[test]
    fn models_subcommand_accepts_an_optional_provider() {
        let cli =
            Cli::try_parse_from(["harness-e2e", "models", "--provider", "openai-codex"]).unwrap();
        let Command::Models(args) = cli.command else {
            panic!("expected models command");
        };
        assert_eq!(args.provider.as_deref(), Some("openai-codex"));
        assert_eq!(args.url, "ws://127.0.0.1:49134");
    }

    #[test]
    fn run_accepts_code_defined_scenario() {
        let cli = Cli::try_parse_from([
            "harness-e2e",
            "run",
            "--model",
            "model",
            "--provider",
            "provider",
            "--scenario",
            "persistent_state",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(args.scenario, [ScenarioId::PersistentState]);
        assert_eq!(args.output, PathBuf::from("target/e2e"));
        assert!(args.runs_dir.is_none());
        assert_eq!(args.technical_retries, None);
        assert_eq!(args.progress_interval_seconds, 15);
    }

    #[test]
    fn security_review_uses_the_common_run_command() {
        let cli = Cli::try_parse_from([
            "harness-e2e",
            "run",
            "--model",
            "gpt-5-codex",
            "--provider",
            "openai-codex",
            "--scenario",
            "security_review",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(args.scenario, [ScenarioId::SecurityReview]);
        assert_eq!(args.technical_retries, None);
    }

    #[test]
    fn dashboard_is_available_as_serve_alias() {
        for name in ["dashboard", "serve"] {
            let cli = Cli::try_parse_from(["harness-e2e", name]).unwrap();
            let Command::Dashboard(args) = cli.command else {
                panic!("expected dashboard command");
            };
            assert_eq!(args.listen.to_string(), "0.0.0.0:4173");
            assert_eq!(args.url, "ws://127.0.0.1:49134");
            assert!(!args.view_only);
        }
        let cli = Cli::try_parse_from(["harness-e2e", "dashboard", "--view-only"]).unwrap();
        let Command::Dashboard(args) = cli.command else {
            panic!("expected dashboard command");
        };
        assert!(args.view_only);
    }

    #[test]
    fn final_analyzer_defaults_to_the_subject_for_every_run() {
        let subject = SubjectConfig {
            model: "model".into(),
            provider: "provider".into(),
        };
        let judge = judge_config(&subject, None, None);
        assert_eq!(judge.model, subject.model);
        assert_eq!(judge.provider, subject.provider);
    }

    #[test]
    fn report_subcommand_accepts_a_results_directory() {
        let cli = Cli::try_parse_from(["harness-e2e", "report", "target/e2e"]).unwrap();
        let Command::Report(args) = cli.command else {
            panic!("expected report command");
        };
        assert_eq!(args.input, PathBuf::from("target/e2e"));
    }

    #[test]
    fn fault_commands_require_explicit_evidence_paths() {
        let cli = Cli::try_parse_from([
            "harness-e2e",
            "fault-plan",
            "--profile",
            "profile.json",
            "--output",
            "plan.json",
        ])
        .unwrap();
        let Command::FaultPlan(args) = cli.command else {
            panic!("expected fault-plan command");
        };
        assert_eq!(args.profile, PathBuf::from("profile.json"));

        let cli = Cli::try_parse_from([
            "harness-e2e",
            "fault-evaluate",
            "--profile",
            "profile.json",
            "--plan",
            "plan.json",
            "--journal",
            "journal.json",
            "--output",
            "evaluation.json",
        ])
        .unwrap();
        let Command::FaultEvaluate(args) = cli.command else {
            panic!("expected fault-evaluate command");
        };
        assert!(args.results.is_none());
    }
}
