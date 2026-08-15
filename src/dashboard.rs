mod api;
mod assets;
mod bus;
mod controller;
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

const LOCAL_SCHEMA_VERSION: u32 = 1;

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
struct RunRequest {
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
struct RunMetadata {
    schema_version: u32,
    id: String,
    label: String,
    status: JobStatus,
    started_at: String,
    completed_at: String,
    returncode: Option<i32>,
    error: String,
    request: RunRequest,
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
    use super::store::{read_metadata, write_metadata};
    use super::*;
    use crate::identity::{ExecutionIdentity, StackIdentity, SystemUnderTestIdentity};
    use crate::report::{
        CostReport, E2eManifestV2, E2eReport, E2eRunReport, E2eScenarioReport, ModelArtifact,
        RunStatus, MANIFEST_SCHEMA_VERSION,
    };
    use crate::scenarios::ExecutionPolicy;
    use crate::wire::{
        ControlPlaneEvidence, FunctionContractEvidence, CONTROL_PLANE_CONTRACT_NAME,
        CONTROL_PLANE_CONTRACT_VERSION,
    };

    const TEST_DIGEST: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn request() -> RunRequest {
        RunRequest {
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
                },
                vec![run],
            )],
        )
    }

    fn manifest(report: &E2eReport) -> E2eManifestV2 {
        E2eManifestV2 {
            schema_version: MANIFEST_SCHEMA_VERSION,
            execution: report.execution.clone().unwrap(),
            system_under_test: report.system_under_test.clone().unwrap(),
            subject: report.subject.clone(),
            judge: report.judge.clone(),
            control_plane: ControlPlaneEvidence {
                name: CONTROL_PLANE_CONTRACT_NAME.into(),
                version: CONTROL_PLANE_CONTRACT_VERSION,
                functions: vec![FunctionContractEvidence {
                    function_id: "harness::status".into(),
                    contract: json!({"name": CONTROL_PLANE_CONTRACT_NAME, "version": 1}),
                    request_schema: json!({"type": "object"}),
                    response_schema: json!({"type": "object"}),
                    sha256: TEST_DIGEST.into(),
                }],
            },
        }
    }

    fn write_report(output: &Path) {
        let mut report = report();
        let manifest = manifest(&report);
        report.write_to(output, &manifest).unwrap();
    }

    fn metadata() -> RunMetadata {
        RunMetadata {
            schema_version: LOCAL_SCHEMA_VERSION,
            id: "local-20260807T120000-abcdef12".into(),
            label: "first run".into(),
            status: JobStatus::Completed,
            started_at: "2026-08-07T12:00:00Z".into(),
            completed_at: "2026-08-07T12:00:02Z".into(),
            returncode: Some(0),
            error: String::new(),
            request: request(),
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
    }

    #[test]
    fn legacy_results_keep_unknown_identity_fields_null() {
        let mut value = serde_json::to_value(report()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("schema_version");
        object.remove("execution");
        object.remove("system_under_test");
        object.remove("manifest");
        object.remove("engine_revision");
        let legacy: E2eReport = serde_json::from_value(value).unwrap();

        let summary = execution_summary(&metadata(), Some(&legacy)).unwrap();
        assert_eq!(legacy.schema_version, 1);
        assert!(summary["source"]["sha"].is_null());
        assert!(summary["source"]["repository"].is_null());
        assert!(summary["release"]["stack_versions"].is_null());
        assert!(summary["stack"]["mode"].is_null());
        assert!(summary["subjects"][0]["engine_revision"].is_null());
        assert!(summary["execution"]["head_sha"].is_null());
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
            let execution = value.execution.as_mut().unwrap();
            execution.execution_id = format!("execution-{index}");
            execution.completed_at = format!("2026-08-0{}T12:00:02Z", index + 7);
            let completed_at = execution.completed_at.clone();
            if let StackIdentity::Source {
                workers_revision, ..
            } = &mut value.system_under_test.as_mut().unwrap().stack
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
        assert!(model
            .tests_list(TestsListRequest {
                cursor: Some("stale:1".into()),
                ..TestsListRequest::default()
            })
            .unwrap_err()
            .to_string()
            .contains("stale"));
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

    #[test]
    fn local_store_rejects_unknown_schema_versions() {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("local-20260807T120000-abcdef12");
        let mut metadata = metadata();
        metadata.schema_version = LOCAL_SCHEMA_VERSION + 1;
        write_metadata(&run, &metadata).unwrap();
        let error = read_metadata(&run).unwrap_err();
        assert!(error.to_string().contains("unsupported local run schema"));
    }
}
