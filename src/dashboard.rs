mod api;
mod assessment_projection;
mod assets;
mod bus;
mod controller;
mod plans;
mod presenter;
mod proxy;
mod read_model;
mod store;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Args)]
pub struct DashboardArgs {
    /// Address used by the local dashboard.
    #[arg(long, default_value = "0.0.0.0:4173")]
    pub listen: SocketAddr,

    /// WebSocket URL of the running Harness stack.
    #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
    pub url: String,

    /// Directory that owns local run metadata, logs, and reports.
    #[arg(long, default_value = "target/harness-e2e-local-runs")]
    pub runs_dir: PathBuf,

    /// Present retained reports without exposing local execution endpoints.
    #[arg(long)]
    pub view_only: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct Defaults {
    url: String,
    model: String,
    provider: String,
    judge_model: String,
    judge_provider: String,
    runs: u32,
    technical_retries: u8,
    seed: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    // iii adds this routing metadata when a browser/worker invokes the
    // function. It is accepted at the boundary but never persisted.
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(super) _caller_worker_id: Option<String>,
    #[serde(default)]
    label: String,
    url: String,
    model: String,
    provider: String,
    #[serde(default)]
    judge_model: String,
    #[serde(default)]
    judge_provider: String,
    scenarios: Vec<String>,
    runs: u32,
    technical_retries: u8,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    plan_context: Option<plans::PlanContext>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Running,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

impl JobStatus {
    fn active(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunMetadata {
    id: String,
    label: String,
    status: JobStatus,
    started_at: String,
    completed_at: String,
    returncode: Option<i32>,
    error: String,
    request: RunRequest,
    #[serde(default)]
    plan_context: Option<plans::PlanContext>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JobView {
    #[serde(flatten)]
    metadata: RunMetadata,
    log: String,
    log_from: u64,
    log_offset: u64,
    log_truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct RunSnapshot {
    job: Option<JobView>,
    defaults: Defaults,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

pub async fn serve(args: DashboardArgs) -> Result<()> {
    api::serve(args).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use serde_json::json;

    use super::controller::{build_run_command, validate_request};
    use super::presenter::{
        contract_fingerprint, execution_detail_value, execution_summary, load_execution_summaries,
    };
    use super::read_model::{DashboardReadModel, EvaluatedVersionsRequest, TestsListRequest};
    use super::store::write_metadata;
    use super::*;
    use crate::identity::{ExecutionIdentity, StackIdentity, SystemUnderTestIdentity};
    use crate::report::{
        CostReport, E2eManifest, E2eReport, E2eRunReport, E2eScenarioReport, ModelArtifact,
        RunStatus,
    };
    use crate::scenarios::ExecutionPolicy;
    use crate::wire::{ControlPlaneEvidence, FunctionContractEvidence};

    const TEST_DIGEST: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn request() -> RunRequest {
        RunRequest {
            _caller_worker_id: None,
            label: " first run ".into(),
            url: "ws://127.0.0.1:49134".into(),
            model: "model".into(),
            provider: "provider".into(),
            judge_model: String::new(),
            judge_provider: String::new(),
            scenarios: vec!["direct_answer".into()],
            runs: 1,
            technical_retries: 1,
            seed: Some(42),
            plan_context: None,
        }
    }

    fn report() -> E2eReport {
        let execution = ExecutionIdentity {
            execution_id: "execution".into(),
            lane: "local".into(),
            started_at: "2026-08-07T12:00:00Z".into(),
            completed_at: "2026-08-07T12:00:02Z".into(),
        };
        let system = SystemUnderTestIdentity {
            stack: StackIdentity::Source {
                workers_repository: "iii-hq/workers".into(),
                workers_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            },
            engine_version: "0.22.0".into(),
            engine_revision: Some("engine-revision".into()),
            harness_version: "1.8.0".into(),
            e2e_repository: "iii-hq/workers".into(),
            e2e_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            contract_hashes: BTreeMap::from([("harness::status".into(), TEST_DIGEST.into())]),
        };
        let mut run = E2eRunReport::new(
            "run".into(),
            "attempt".into(),
            1,
            "session".into(),
            "prompt".into(),
        );
        run.wall_time_ms = 1_500;
        run.score = Some(90);
        run.status = RunStatus::Passed;
        run.cost = CostReport {
            subject_usd: Some(0.1),
            judge_usd: Some(0.0),
            total_usd: Some(0.1),
        };
        E2eReport::new(
            execution,
            system,
            ModelArtifact {
                model: "model".into(),
                provider: "provider".into(),
                context_window: 100,
                max_output_tokens: 10,
                supports_tools: Some(true),
                supports_vision: Some(false),
            },
            None,
            None,
            None,
            vec![E2eScenarioReport::aggregate(
                "direct_answer",
                1,
                ExecutionPolicy {
                    max_turns: 1,
                    max_output_tokens: Some(10),
                    max_total_tokens: 100,
                    stuck_timeout_seconds: 10,
                    max_validation_retries: None,
                },
                vec![run],
            )],
        )
    }

    fn manifest(report: &E2eReport) -> E2eManifest {
        E2eManifest {
            execution: report.execution.clone(),
            system_under_test: report.system_under_test.clone(),
            subject: report.subject.clone(),
            judge: report.judge.clone(),
            control_plane: ControlPlaneEvidence {
                functions: vec![FunctionContractEvidence {
                    function_id: "harness::status".into(),
                    request_schema: json!({"type": "object"}),
                    response_schema: json!({"type": "object"}),
                    sha256: TEST_DIGEST.into(),
                }],
            },
            worker_contracts: Vec::new(),
        }
    }

    fn write_report(output: &Path) {
        let mut report = report();
        let manifest = manifest(&report);
        report.write_to(output, &manifest).unwrap();
    }

    fn metadata() -> RunMetadata {
        RunMetadata {
            id: "local-20260807T120000-abcdef12".into(),
            label: "first run".into(),
            status: JobStatus::Completed,
            started_at: "2026-08-07T12:00:00Z".into(),
            completed_at: "2026-08-07T12:00:02Z".into(),
            returncode: Some(0),
            error: String::new(),
            request: request(),
            plan_context: None,
        }
    }

    #[test]
    fn validates_and_normalizes_run_requests() {
        let mut value = request();
        validate_request(&mut value).unwrap();
        assert_eq!(value.label, "first run");
        value.url = "https://example.com".into();
        assert!(validate_request(&mut value).is_err());
    }

    #[test]
    fn local_requests_allow_rust_defined_composite_scenarios() {
        let mut value = request();
        value.scenarios = vec!["security_review".into(), "direct_answer".into()];
        value.technical_retries = 0;
        validate_request(&mut value).expect("local plans must start composite scenarios");
    }

    #[test]
    fn accepts_engine_caller_metadata_without_persisting_it() {
        let mut value = serde_json::to_value(request()).expect("request should serialize");
        value["_caller_worker_id"] = serde_json::json!("browser-worker");
        let decoded: RunRequest =
            serde_json::from_value(value).expect("engine metadata should be accepted");
        assert_eq!(decoded._caller_worker_id.as_deref(), Some("browser-worker"));

        let serialized = serde_json::to_value(decoded).expect("request should serialize");
        assert!(serialized.get("_caller_worker_id").is_none());
    }

    #[test]
    fn builds_self_invocation_without_cargo() {
        let command = build_run_command(
            Path::new("/tmp/harness-e2e"),
            &request(),
            Path::new("/tmp/results"),
        );
        let std = command.as_std();
        assert_eq!(std.get_program(), "/tmp/harness-e2e");
        let args: Vec<_> = std
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect();
        assert_eq!(args[0], "run");
        assert!(args.contains(&"direct_answer".into()));
        assert!(!args.iter().any(|value| value == "cargo"));
    }

    #[test]
    fn local_summary_and_detail_use_the_static_dashboard_contract() {
        let report = report();
        let summary = execution_summary(&metadata(), Some(&report)).unwrap();
        assert_eq!(summary["status"], "passed");
        assert!(summary["totals"].get("average_score").is_none());
        assert_eq!(summary["run_id"], "execution");
        assert_eq!(summary["lane"], "local");
        assert_eq!(summary["stack"]["mode"], "source");
        assert_eq!(
            summary["source"]["sha"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(summary["subjects"][0]["engine_revision"], "engine-revision");
        assert_eq!(summary["assessment_summary"]["run_count"], 1);
        assert_eq!(
            summary["assessment_summary"]["ai_availability"]["not_evaluated"],
            1
        );
        assert_eq!(
            summary["subjects"][0]["scenarios"][0]["assessment_summary"]["run_count"],
            1
        );
        assert!(summary["workflow_url"].is_null());
        assert_eq!(
            summary["detail_path"],
            "runs/local-20260807T120000-abcdef12.json"
        );
        assert_eq!(
            summary["subjects"][0]["scenarios"][0]["id"],
            "direct_answer"
        );
        let detail = execution_detail_value(&metadata(), &report).unwrap();
        assert_eq!(
            detail["reports"][0]["report"]["scenarios"][0]["scenario_id"],
            "direct_answer"
        );
        assert_eq!(
            detail["reports"][0]["report"]["assessment_contract"]["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            detail["reports"][0]["report"]["scenarios"][0]["runs"][0]["assessment"]["run_id"],
            "run"
        );
    }

    #[test]
    fn contract_fingerprint_matches_the_browser_implementation() {
        let value = json!({
            "case_id": "direct_answer:canonical",
            "execution_policy": {},
            "scenario_id": "direct_answer",
            "scenario_version": 1,
        });
        assert_eq!(contract_fingerprint(&value), "fnv1a32:7fdd620a");
    }

    #[test]
    fn local_store_accepts_native_and_control_plane_runs() {
        let root = tempfile::tempdir().unwrap();
        let control = root.path().join("execution");
        write_report(&control);

        let metadata = metadata();
        let native = root.path().join(&metadata.id);
        write_metadata(&native, &metadata).unwrap();
        write_report(&native.join("results"));
        let summaries = load_execution_summaries(root.path()).unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().any(|summary| {
            summary["id"] == "execution"
                && summary["label"] == "e2e::* control-plane run"
                && summary["status"] == "passed"
        }));
        assert!(summaries.iter().any(|summary| summary["id"] == metadata.id));
    }

    #[test]
    fn versioned_test_catalog_pools_raw_scores_and_keeps_evidence_lazy() {
        let root = tempfile::tempdir().unwrap();
        for (index, (revision, scores)) in [
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                vec![10, 100, 100],
            ),
            ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", vec![80, 90]),
        ]
        .into_iter()
        .enumerate()
        {
            let mut value = report();
            value.judge = Some(ModelArtifact {
                model: "judge-model".into(),
                provider: "judge-provider".into(),
                context_window: 100,
                max_output_tokens: 10,
                supports_tools: Some(false),
                supports_vision: Some(false),
            });
            value.judge_protocol = Some("json".into());
            let execution = &mut value.execution;
            execution.execution_id = format!("execution-{index}");
            execution.completed_at = format!("2026-08-0{}T12:00:02Z", index + 7);
            let completed_at = execution.completed_at.clone();
            if let StackIdentity::Source {
                workers_revision, ..
            } = &mut value.system_under_test.stack
            {
                *workers_revision = revision.into();
            }
            let runs = scores
                .into_iter()
                .enumerate()
                .map(|(run_index, score)| {
                    let mut run = E2eRunReport::new(
                        format!("run-{index}-{run_index}"),
                        format!("attempt-{run_index}"),
                        1,
                        format!("session-{index}-{run_index}"),
                        "prompt".into(),
                    );
                    run.status = RunStatus::Passed;
                    run.score = Some(score);
                    run.wall_time_ms = 1_000 + u64::from(score);
                    run
                })
                .collect();
            value.scenarios = vec![E2eScenarioReport::aggregate(
                "direct_answer",
                1,
                ExecutionPolicy {
                    max_turns: 1,
                    max_output_tokens: Some(10),
                    max_total_tokens: 100,
                    stuck_timeout_seconds: 10,
                    max_validation_retries: None,
                },
                runs,
            )];

            let mut run_metadata = metadata();
            run_metadata.id = format!("local-version-{index}");
            run_metadata.completed_at = completed_at;
            let run_dir = root.path().join(&run_metadata.id);
            write_metadata(&run_dir, &run_metadata).unwrap();
            let manifest = manifest(&value);
            value.write_to(&run_dir.join("results"), &manifest).unwrap();
        }

        let model = DashboardReadModel::load(root.path()).unwrap();
        let evaluated = model.evaluated_versions(EvaluatedVersionsRequest::default());
        assert_eq!(evaluated.cohorts.len(), 1);
        assert_eq!(evaluated.versions.len(), 2);
        let cohort_id = evaluated.cohorts[0].id.clone();
        let from = evaluated
            .versions
            .iter()
            .find(|version| version.label.contains("aaaaaaaaaaaa"))
            .unwrap()
            .id
            .clone();
        let to = evaluated
            .versions
            .iter()
            .find(|version| version.label.contains("bbbbbbbbbbbb"))
            .unwrap()
            .id
            .clone();
        let catalog = model
            .tests_list(TestsListRequest {
                limit: Some(100),
                cohort_id: Some(cohort_id.clone()),
                from_version_id: Some(from.clone()),
                to_version_id: Some(to.clone()),
                ..TestsListRequest::default()
            })
            .unwrap();
        let row = catalog
            .rows
            .iter()
            .find(|row| row.test_id == "direct_answer")
            .unwrap();
        let result = row.result.as_ref().unwrap();
        assert_eq!(result.from.as_ref().unwrap().median_score, Some(100.0));
        assert_eq!(result.to.as_ref().unwrap().median_score, Some(85.0));
        assert_eq!(result.delta.score, Some(-15.0));
        assert_eq!(result.compatibility, "compatible");
        assert!(result.compatibility_reasons.is_empty());
        assert_eq!(
            result.from.as_ref().unwrap().assessment_summary.run_count,
            3
        );
        assert!(result.from_observations.is_empty());
        assert!(result.to_observations.is_empty());

        let detail = model
            .test_version_get(super::read_model::TestVersionGetRequest {
                test_id: "direct_answer".into(),
                test_version: 1,
                cohort_id,
                from_version_id: from,
                to_version_id: to,
            })
            .unwrap();
        assert_eq!(detail.from_observations.len(), 1);
        assert_eq!(detail.to_observations.len(), 1);
        assert_eq!(detail.from_observations[0].assessment_summary.run_count, 3);
        assert!(detail.from_observations[0]
            .assessment_profile_sha256
            .starts_with("sha256:"));
        assert!(detail.from_observations[0]
            .analyzer_profile_sha256
            .starts_with("sha256:"));
        assert!(model
            .tests_list(TestsListRequest {
                cursor: Some("stale:1".into()),
                ..TestsListRequest::default()
            })
            .unwrap_err()
            .to_string()
            .contains("stale"));

        let history = model
            .test_history(super::read_model::TestHistoryRequest {
                test_id: "direct_answer".into(),
                test_version: None,
                limit: Some(1),
                ..super::read_model::TestHistoryRequest::default()
            })
            .unwrap();
        assert_eq!(history.test_version, 1);
        assert_eq!(history.total, 2);
        assert_eq!(history.observations.len(), 1);
        assert!(history.next_cursor.is_some());
        assert_eq!(history.series.len(), 2);
        assert_eq!(history.subject_models.len(), 1);
        assert_eq!(history.subject_models[0].provider, "provider");
        assert_eq!(history.subject_models[0].models, vec!["model"]);
        assert_eq!(history.judge_models.len(), 1);
        assert_eq!(history.judge_models[0].provider, "judge-provider");
        assert_eq!(history.judge_models[0].models, vec!["judge-model"]);
        assert_ne!(
            history.series[0].system_version_id,
            history.series[1].system_version_id
        );
        assert_eq!(history.observations[0].median_tokens, None);
        assert!(history.observations[0].median_duration_seconds.is_some());
        assert_eq!(
            history.observations[0].harness_revision.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            history.observations[0].system_revision.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        let filtered = model
            .test_history(super::read_model::TestHistoryRequest {
                test_id: "direct_answer".into(),
                test_version: Some(1),
                case_id: Some(history.observations[0].case_id.clone()),
                subject_provider: Some("provider".into()),
                subject_model: Some("model".into()),
                judge_provider: Some("judge-provider".into()),
                judge_model: Some("judge-model".into()),
                result: Some("passed".into()),
                ..super::read_model::TestHistoryRequest::default()
            })
            .unwrap();
        assert_eq!(filtered.total, 2);
        let no_matching_judge = model
            .test_history(super::read_model::TestHistoryRequest {
                test_id: "direct_answer".into(),
                judge_provider: Some("other-provider".into()),
                ..super::read_model::TestHistoryRequest::default()
            })
            .unwrap();
        assert_eq!(no_matching_judge.total, 0);
    }

    #[test]
    fn metric_history_keeps_contracts_in_separate_series() {
        let root = tempfile::tempdir().unwrap();
        for (index, max_turns) in [(0, 1), (1, 2)] {
            let mut value = report();
            value.execution.execution_id = format!("execution-contract-{index}");
            value.execution.completed_at = format!("2026-08-0{}T12:00:02Z", index + 7);
            value.scenarios[0].execution_policy.max_turns = max_turns;

            let mut run_metadata = metadata();
            run_metadata.id = format!("local-contract-{index}");
            run_metadata.completed_at = value.execution.completed_at.clone();
            let run_dir = root.path().join(&run_metadata.id);
            write_metadata(&run_dir, &run_metadata).unwrap();
            let manifest = manifest(&value);
            value.write_to(&run_dir.join("results"), &manifest).unwrap();
        }

        let model = DashboardReadModel::load(root.path()).unwrap();
        let history = model
            .test_history(super::read_model::TestHistoryRequest {
                test_id: "direct_answer".into(),
                limit: Some(100),
                ..super::read_model::TestHistoryRequest::default()
            })
            .unwrap();

        assert_eq!(history.total, 2);
        assert_eq!(history.series.len(), 2);
        assert_eq!(
            history
                .series
                .iter()
                .map(|series| series.execution_count)
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert_ne!(
            history.observations[0].contract_sha256,
            history.observations[1].contract_sha256
        );
    }

    #[test]
    fn dashboard_accepts_safe_local_and_control_plane_execution_ids() {
        assert!(super::presenter::validate_execution_id("local-20260807T120000-abcdef12").is_ok());
        assert!(
            super::presenter::validate_execution_id("d0be4cb7dcf8561079b673d735715060").is_ok()
        );
        assert!(super::presenter::validate_execution_id("../results").is_err());
        assert!(
            super::presenter::validate_execution_id("d0be4cb7dcf8561079b673d735715060/extra")
                .is_err()
        );
    }
}
