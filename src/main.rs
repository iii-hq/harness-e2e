use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use harness_e2e::control::{scenarios_list, ScenariosListRequest};
use harness_e2e::dashboard;
use harness_e2e::fault::{FaultEvaluation, FaultJournal, FaultPlan, FaultProfile};
use harness_e2e::judge::JudgeConfig;
use harness_e2e::manifest;
use harness_e2e::markdown::{self, ScenarioKey};
use harness_e2e::report::E2eReport;
#[cfg(test)]
use harness_e2e::scenarios::ScenarioId;
use harness_e2e::suite::{run_suite, SubjectConfig, SuiteRunConfig};
use harness_e2e::worker::{self, WorkerArgs};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(name = "harness-e2e", about = "Run real-stack quality scenarios")]
struct Cli {
    /// Print the Registry worker manifest as JSON and exit.
    #[arg(long)]
    manifest: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the harness-e2e worker explicitly (the default when no command is given).
    Worker(WorkerArgs),
    /// Print every built-in and Markdown scenario id as a JSON array.
    List,
    /// Print the canonical materialized scenario catalog used by campaign tooling.
    Catalog {
        /// Fixed catalog materialization seed embedded in campaign revisions.
        #[arg(long, default_value_t = 4404)]
        seed: u64,
    },
    /// Validate every scenarios/*.md file without running a model.
    ValidateScenarios(ValidateScenariosArgs),
    /// List models registered in the running stack.
    Models(ModelsArgs),
    /// Execute one or more quality scenarios against a running stack.
    Run(RunArgs),
    /// Replay one exact immutable materialized Markdown plan.
    ReplayMaterialized(ReplayMaterializedArgs),
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

    /// Opt-in behavioral audit analyzer over each run's transcript. Supply
    /// together with --audit-provider; omit both to keep the audit
    /// deterministic-only.
    #[arg(long, env = "HARNESS_E2E_AUDIT_MODEL", requires = "audit_provider")]
    audit_model: Option<String>,

    #[arg(long, env = "HARNESS_E2E_AUDIT_PROVIDER", requires = "audit_model")]
    audit_provider: Option<String>,

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
    #[arg(long)]
    scenario: Vec<ScenarioKey>,
}

#[derive(Debug, Args)]
struct ValidateScenariosArgs {
    #[arg(long, default_value = "scenarios")]
    directory: PathBuf,

    #[arg(long, default_value = "config/campaigns")]
    campaigns: PathBuf,

    /// Optional previous scenarios directory used to enforce version bumps.
    #[arg(long)]
    base_directory: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ReplayMaterializedArgs {
    /// Path to an archived materialized-plan.json.
    plan: PathBuf,

    #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
    url: String,

    #[arg(long, env = "HARNESS_E2E_OUTPUT", default_value = "target/e2e-replay")]
    output: PathBuf,

    #[arg(
        long,
        env = "HARNESS_E2E_PROGRESS_INTERVAL_SECONDS",
        default_value_t = 15
    )]
    progress_interval_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct ReplayModel {
    model: String,
    provider: String,
}

#[derive(Debug, Deserialize)]
struct ReplayCampaign {
    runs: u32,
    technical_retries: u8,
}

#[derive(Debug, Deserialize)]
struct ReplayMarkdownPlan {
    schema: String,
    scenario: markdown::CompiledMarkdownScenario,
    seed: u64,
    subject: ReplayModel,
    auxiliary: ReplayModel,
    audit: Option<ReplayModel>,
    campaign: ReplayCampaign,
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
    let cli = Cli::parse();
    if cli.manifest {
        println!("{}", serde_json::to_string(&manifest::build_manifest())?);
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match cli.command {
        None => worker::serve(WorkerArgs::default()).await,
        Some(Command::Worker(args)) => worker::serve(args).await,
        Some(Command::List) => {
            let ids = markdown::all_keys()?
                .into_iter()
                .map(|scenario| scenario.to_string())
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string(&ids)?);
            Ok(())
        }
        Some(Command::Catalog { seed }) => {
            let catalog = scenarios_list(ScenariosListRequest { seed: Some(seed) })?;
            println!("{}", serde_json::to_string(&catalog)?);
            Ok(())
        }
        Some(Command::ValidateScenarios(args)) => validate_scenarios(args),
        Some(Command::Models(args)) => models(args).await,
        Some(Command::Run(args)) => run(args).await,
        Some(Command::ReplayMaterialized(args)) => replay_materialized(args).await,
        Some(Command::Report(args)) => report(args),
        Some(Command::Dashboard(args)) => dashboard::serve(args).await,
        Some(Command::FaultPlan(args)) => fault_plan(args),
        Some(Command::FaultEvaluate(args)) => fault_evaluate(args),
    }
}

fn validate_scenarios(args: ValidateScenariosArgs) -> Result<()> {
    let scenarios = markdown::validate_directory(&args.directory, &args.campaigns)?;
    if let Some(base_directory) = args.base_directory.as_deref() {
        markdown::validate_version_progression(&scenarios, base_directory, &args.campaigns)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "valid": true,
            "scenario_count": scenarios.len(),
            "scenarios": scenarios,
        }))?
    );
    Ok(())
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
    let selected_scenarios = markdown::selected_keys(&args.scenario)?;
    let technical_retries = args.technical_retries.unwrap_or_else(|| {
        if selected_scenarios
            .iter()
            .any(|scenario| !scenario.execution_kind().replay_safe())
        {
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
    let has_markdown = selected_scenarios
        .iter()
        .any(|scenario| scenario.built_in().is_none());
    if has_markdown && (args.judge_model.is_none() || args.judge_provider.is_none()) {
        bail!("Markdown scenarios require explicit --judge-model and --judge-provider values");
    }
    let judge = Some(judge_config(
        &subject,
        args.judge_model,
        args.judge_provider,
    ));
    let audit_analyzer = args
        .audit_model
        .zip(args.audit_provider)
        .map(|(model, provider)| JudgeConfig { model, provider });
    let outcome = run_suite(SuiteRunConfig {
        url: args.url,
        execution_id,
        subject,
        judge,
        audit_analyzer,
        output,
        scenarios: selected_scenarios,
        local_markdown_scenarios: Vec::new(),
        runs: args.runs,
        seed: args.seed,
        rotating_seeds: args.rotating_seeds,
        technical_retries,
        progress_interval: (args.progress_interval_seconds > 0)
            .then(|| std::time::Duration::from_secs(args.progress_interval_seconds)),
        control: None,
        observation_contract: None,
        materialized_markdown_plan: None,
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

async fn replay_materialized(args: ReplayMaterializedArgs) -> Result<()> {
    let bytes = std::fs::read(&args.plan)
        .with_context(|| format!("read materialized plan {}", args.plan.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode materialized plan {}", args.plan.display()))?;
    let frozen: ReplayMarkdownPlan = serde_json::from_value(value.clone())
        .context("decode materialized Markdown replay fields")?;
    if frozen.schema != "harness-e2e-materialized-markdown-plan/v2" {
        bail!(
            "unsupported materialized Markdown plan schema '{}'; expected v2",
            frozen.schema
        );
    }
    let key = frozen.scenario.id.parse::<ScenarioKey>()?;
    if key.built_in().is_some() {
        bail!("materialized replay accepts only Markdown scenarios");
    }
    let subject = SubjectConfig {
        model: frozen.subject.model,
        provider: frozen.subject.provider,
    };
    let judge = JudgeConfig {
        model: frozen.auxiliary.model,
        provider: frozen.auxiliary.provider,
    };
    let audit_analyzer = frozen.audit.map(|audit| JudgeConfig {
        model: audit.model,
        provider: audit.provider,
    });
    let outcome = run_suite(SuiteRunConfig {
        url: args.url,
        execution_id: None,
        subject,
        judge: Some(judge),
        audit_analyzer,
        output: args.output,
        scenarios: vec![key],
        local_markdown_scenarios: Vec::new(),
        runs: frozen.campaign.runs,
        seed: Some(frozen.seed),
        rotating_seeds: Vec::new(),
        technical_retries: frozen.campaign.technical_retries,
        progress_interval: (args.progress_interval_seconds > 0)
            .then(|| std::time::Duration::from_secs(args.progress_interval_seconds)),
        control: None,
        observation_contract: None,
        materialized_markdown_plan: Some(value),
    })
    .await
    .context("replay immutable Markdown plan")?;
    print!("{}", outcome.report.summary(false));
    println!("report: {}", outcome.report_path.display());
    if !outcome.report.passed {
        bail!("materialized Markdown replay failed");
    }
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
            Some(Command::List)
        ));
    }

    #[test]
    fn no_subcommand_runs_the_worker_with_defaults() {
        let cli = Cli::try_parse_from(["harness-e2e"]).unwrap();
        assert!(!cli.manifest);
        assert!(cli.command.is_none());
    }

    #[test]
    fn manifest_flag_needs_no_worker_configuration() {
        let cli = Cli::try_parse_from(["harness-e2e", "--manifest"]).unwrap();
        assert!(cli.manifest);
        assert!(cli.command.is_none());
    }

    #[test]
    fn worker_subcommand_accepts_local_overrides() {
        let cli = Cli::try_parse_from([
            "harness-e2e",
            "worker",
            "--url",
            "ws://127.0.0.1:5000",
            "--data-dir",
            "target/worker-data",
        ])
        .unwrap();
        let Some(Command::Worker(args)) = cli.command else {
            panic!("expected worker command");
        };
        assert_eq!(args.url, "ws://127.0.0.1:5000");
        assert_eq!(args.data_dir, Some(PathBuf::from("target/worker-data")));
    }

    #[test]
    fn models_subcommand_accepts_an_optional_provider() {
        let cli =
            Cli::try_parse_from(["harness-e2e", "models", "--provider", "openai-codex"]).unwrap();
        let Some(Command::Models(args)) = cli.command else {
            panic!("expected models command");
        };
        assert_eq!(args.provider.as_deref(), Some("openai-codex"));
        assert_eq!(args.url, "ws://127.0.0.1:49134");
    }

    #[test]
    fn run_accepts_markdown_scenario() {
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
        let Some(Command::Run(args)) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(
            args.scenario,
            [ScenarioKey::Markdown("persistent_state".into())]
        );
        assert_eq!(args.output, PathBuf::from("target/e2e"));
        assert!(args.runs_dir.is_none());
        assert_eq!(args.technical_retries, None);
        assert_eq!(args.progress_interval_seconds, 15);
    }

    #[test]
    fn run_rejects_removed_prefixed_markdown_scenario_id() {
        assert!(Cli::try_parse_from([
            "harness-e2e",
            "run",
            "--model",
            "model",
            "--provider",
            "provider",
            "--scenario",
            "markdown_persistent_state",
        ])
        .is_err());
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
        let Some(Command::Run(args)) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(args.scenario, [ScenarioId::SecurityReview.into()]);
        assert_eq!(args.technical_retries, None);
    }

    #[test]
    fn dashboard_is_available_as_serve_alias() {
        for name in ["dashboard", "serve"] {
            let cli = Cli::try_parse_from(["harness-e2e", name]).unwrap();
            let Some(Command::Dashboard(args)) = cli.command else {
                panic!("expected dashboard command");
            };
            assert_eq!(args.listen.to_string(), "0.0.0.0:4173");
            assert_eq!(args.url, "ws://127.0.0.1:49134");
            assert!(!args.view_only);
        }
        let cli = Cli::try_parse_from(["harness-e2e", "dashboard", "--view-only"]).unwrap();
        let Some(Command::Dashboard(args)) = cli.command else {
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
        let Some(Command::Report(args)) = cli.command else {
            panic!("expected report command");
        };
        assert_eq!(args.input, PathBuf::from("target/e2e"));
    }

    #[test]
    fn replay_materialized_requires_only_the_archived_plan_path() {
        let cli = Cli::try_parse_from([
            "harness-e2e",
            "replay-materialized",
            "evidence/run/attempt/materialized-plan.json",
        ])
        .unwrap();
        let Some(Command::ReplayMaterialized(args)) = cli.command else {
            panic!("expected replay-materialized command");
        };
        assert_eq!(
            args.plan,
            PathBuf::from("evidence/run/attempt/materialized-plan.json")
        );
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
        let Some(Command::FaultPlan(args)) = cli.command else {
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
        let Some(Command::FaultEvaluate(args)) = cli.command else {
            panic!("expected fault-evaluate command");
        };
        assert!(args.results.is_none());
    }
}
