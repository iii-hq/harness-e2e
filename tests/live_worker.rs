//! Validation of a running `harness-e2e` worker through its control plane.
//!
//! These tests talk to a real iii engine that has the worker registered, so
//! they are ignored by default. Opt in with:
//!
//! ```text
//! HARNESS_E2E_LIVE_URL=ws://127.0.0.1:49134 \
//! HARNESS_E2E_LIVE_NAMESPACE=my-project \
//! cargo test --test live_worker -- --ignored
//! ```
//!
//! Seven scenarios state host paths in their prompts, so their definition
//! digests match the worker's only when the test process carries the same
//! `HARNESS_E2E_RUN_DIR`, `TMPDIR` and `HARNESS_E2E_*_FIXTURE_PATH` values as
//! the worker. The scenario run additionally needs `HARNESS_E2E_LIVE_PROVIDER`
//! and `HARNESS_E2E_LIVE_MODEL`, a subject the stack can route, and leaves one
//! labelled execution in the worker's storage.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use harness_e2e::context::E2eContext;
use harness_e2e::control::{
    ExecutionPhase, ResultsGetResponse, ResultsListResponse, RunAccepted, ScenariosListResponse,
    StatusResponse,
};
use harness_e2e::report::TechnicalState;
use harness_e2e::scenarios::ScenarioId;
use serde_json::json;

const LIVE_LABEL: &str = "live worker validation";
const PROMPT_ENVIRONMENT: [&str; 6] = [
    "HARNESS_E2E_RUN_DIR",
    "TMPDIR",
    "HARNESS_E2E_FIXTURE_PATH",
    "HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH",
    "HARNESS_E2E_INCIDENT_FIXTURE_PATH",
    "HARNESS_E2E_SECURITY_FIXTURE_PATH",
];

struct Live {
    context: E2eContext,
}

/// One engine registration per test process: the SDK keeps process-wide
/// state, so a second registration never reaches the connected state. Tests
/// share the runtime that owns the connection and run one at a time.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn live() -> &'static Live {
    static LIVE: OnceLock<Live> = OnceLock::new();
    LIVE.get_or_init(|| runtime().block_on(connect()))
}

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn connect() -> Live {
    let url = std::env::var("HARNESS_E2E_LIVE_URL")
        .expect("HARNESS_E2E_LIVE_URL names the engine the worker is registered in");
    let namespace =
        std::env::var("HARNESS_E2E_LIVE_NAMESPACE").unwrap_or_else(|_| "my-project".into());
    let client = iii_sdk::register_worker(
        &url,
        iii_sdk::InitOptions {
            namespace: Some(namespace),
            ..iii_sdk::InitOptions::default()
        },
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the engine accepted the test client within 10s");
    Live {
        context: E2eContext::from_client(client),
    }
}

fn is_full_git_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The worker answering the control plane must publish exactly the catalog
/// this revision materializes: same ids, seeds, cases and definition digests.
/// A stale worker binary fails here before anything runs.
#[test]
#[ignore = "requires HARNESS_E2E_LIVE_URL pointing at an engine with the harness-e2e worker"]
fn the_live_worker_publishes_the_catalog_of_this_revision() {
    let _serial = one_at_a_time();
    let live = live();
    let listed: ScenariosListResponse = runtime()
        .block_on(live.context.trigger("e2e::scenarios-list", json!({})))
        .expect("e2e::scenarios-list answers");

    assert_eq!(listed.schema, "e2e-scenario-catalog");
    assert_eq!(listed.runner.name, "harness-e2e");
    assert!(
        is_full_git_sha(&listed.runner.revision),
        "runner revision is a full git sha, got {:?}",
        listed.runner.revision
    );
    let ids: Vec<ScenarioId> = listed
        .scenarios
        .iter()
        .map(|descriptor| descriptor.scenario_id)
        .collect();
    assert_eq!(
        ids,
        ScenarioId::ALL.to_vec(),
        "registry order and membership"
    );

    for descriptor in &listed.scenarios {
        let materialized = descriptor
            .scenario_id
            .materialize("catalog", descriptor.seed)
            .unwrap();
        let id = descriptor.scenario_id;
        assert_eq!(
            descriptor.seed,
            id.canonical_seed(),
            "{id:?} canonical seed"
        );
        assert_eq!(
            descriptor.case_id, materialized.case.case_id,
            "{id:?} case id"
        );
        assert_eq!(
            descriptor.inputs_sha256, materialized.case.inputs_sha256,
            "{id:?} inputs"
        );
        assert_eq!(
            descriptor.behavior_sha256, materialized.case.behavior_sha256,
            "{id:?} definition digest differs: either the worker was built from other \
             definitions, or its prompt states a host path and this process does not \
             carry the worker's {PROMPT_ENVIRONMENT:?}"
        );
        assert_eq!(
            descriptor.required_capabilities, materialized.case.required_capabilities,
            "{id:?} capabilities"
        );
        assert_eq!(
            serde_json::to_value(&descriptor.deliverable_contract).unwrap(),
            serde_json::to_value(&materialized.case.deliverable_contract).unwrap(),
            "{id:?} deliverable contract"
        );
    }
}

/// The control plane rejects a malformed request without admitting anything
/// and reports an unknown execution as an error, not as a transport failure.
#[test]
#[ignore = "requires HARNESS_E2E_LIVE_URL pointing at an engine with the harness-e2e worker"]
fn the_live_worker_validates_requests_before_admitting_them() {
    let _serial = one_at_a_time();
    let live = live();
    runtime().block_on(validate_requests(live));
}

async fn validate_requests(live: &Live) {
    let before: ResultsListResponse = live
        .context
        .trigger("e2e::results-list", json!({ "limit": 200 }))
        .await
        .expect("e2e::results-list answers");

    let rejected = live
        .context
        .trigger_value(
            "e2e::run",
            json!({
                "idempotency_key": "",
                "label": LIVE_LABEL,
                "model": "no-such-model",
                "provider": "no-such-provider",
                "scenarios": ["minimal_path"],
                "runs": 1,
            }),
        )
        .await
        .expect_err("an empty idempotency key is refused");
    assert!(
        format!("{rejected:#}").contains("idempotency_key"),
        "the refusal names the field: {rejected:#}"
    );

    let unknown = live
        .context
        .trigger_value(
            "e2e::status",
            json!({ "execution_id": "0123456789abcdef0123456789abcdef" }),
        )
        .await
        .expect_err("an unknown execution is an error");
    assert!(
        !format!("{unknown:#}").contains("function_not_found"),
        "the worker answered, the function exists: {unknown:#}"
    );

    let after: ResultsListResponse = live
        .context
        .trigger("e2e::results-list", json!({ "limit": 200 }))
        .await
        .unwrap();
    assert_eq!(
        before.executions.len(),
        after.executions.len(),
        "a refused request admits nothing"
    );
}

/// One real scenario goes through the whole worker: admission, materialization,
/// setup, the subject turn, capture, evaluation over the captured deliverable,
/// persistence and a report readable back through the control plane.
#[test]
#[ignore = "requires HARNESS_E2E_LIVE_URL plus HARNESS_E2E_LIVE_PROVIDER and HARNESS_E2E_LIVE_MODEL"]
fn the_live_worker_runs_minimal_path_to_a_technically_valid_report() {
    let _serial = one_at_a_time();
    let live = live();
    runtime().block_on(run_minimal_path(live));
}

async fn run_minimal_path(live: &Live) {
    let provider = std::env::var("HARNESS_E2E_LIVE_PROVIDER").expect("HARNESS_E2E_LIVE_PROVIDER");
    let model = std::env::var("HARNESS_E2E_LIVE_MODEL").expect("HARNESS_E2E_LIVE_MODEL");
    let timeout_seconds: u64 = std::env::var("HARNESS_E2E_LIVE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(900);
    let listed: ScenariosListResponse = live
        .context
        .trigger("e2e::scenarios-list", json!({}))
        .await
        .unwrap();

    let idempotency_key = format!("live-worker-validation:{}", uuid::Uuid::new_v4().simple());
    let accepted: RunAccepted = live
        .context
        .trigger(
            "e2e::run",
            json!({
                "idempotency_key": idempotency_key,
                "label": LIVE_LABEL,
                "lane": "local",
                "model": model,
                "provider": provider,
                "scenarios": ["minimal_path"],
                "runs": 1,
                "technical_retries": 0,
                "progress_interval_seconds": 5,
            }),
        )
        .await
        .expect("e2e::run admits minimal_path");
    assert!(
        !accepted.duplicate,
        "a fresh idempotency key is not a duplicate"
    );
    assert!(accepted.request_sha256.starts_with("sha256:"));

    let started = Instant::now();
    let status = loop {
        let status: StatusResponse = live
            .context
            .trigger(
                "e2e::status",
                json!({ "execution_id": accepted.execution_id }),
            )
            .await
            .expect("e2e::status answers while the execution runs");
        if status.terminal {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(timeout_seconds),
            "execution {} still in phase {:?} after {timeout_seconds}s",
            accepted.execution_id,
            status.phase
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    assert_eq!(
        status.phase,
        ExecutionPhase::Completed,
        "execution {} ended in {:?}: {}",
        accepted.execution_id,
        status.phase,
        status.error
    );
    assert!(status.error.is_empty(), "{}", status.error);

    let results: ResultsGetResponse = live
        .context
        .trigger(
            "e2e::results-get",
            json!({ "execution_id": accepted.execution_id }),
        )
        .await
        .expect("e2e::results-get answers for a completed execution");
    let report = results
        .report
        .expect("a completed execution retains its report");
    assert_eq!(
        report.result_contract_sha256,
        harness_e2e::result_contract::RESULT_CONTRACT_SHA256,
        "the worker writes this revision's result contract"
    );
    assert_eq!(report.scenarios.len(), 1);
    let scenario = &report.scenarios[0];
    assert_eq!(scenario.scenario_id, "minimal_path");
    assert_eq!(
        scenario.behavior_sha256.as_deref(),
        Some(
            listed
                .scenarios
                .iter()
                .find(|descriptor| descriptor.scenario_id == ScenarioId::MinimalPath)
                .unwrap()
                .behavior_sha256
                .as_str()
        )
    );
    assert_eq!(scenario.runs.len(), 1, "one run was requested");
    let run = &scenario.runs[0];
    assert_eq!(
        run.technical,
        TechnicalState::Valid,
        "the run is technically valid, status {:?}",
        run.status
    );
    assert!(
        !run.deliverables.is_empty(),
        "minimal_path captures its deliverable before cleanup"
    );
    assert!(
        run.score.is_some(),
        "a technically valid run carries the sum of its evaluated criteria"
    );

    let listed_after: ResultsListResponse = live
        .context
        .trigger(
            "e2e::results-list",
            json!({ "lane": "local", "scenario_id": "minimal_path", "limit": 200 }),
        )
        .await
        .unwrap();
    assert!(
        listed_after
            .executions
            .iter()
            .any(|execution| execution.execution_id == accepted.execution_id),
        "the execution is listed for its lane and scenario"
    );
}
