//! `browser_cross_site` — exercise a real browser across three isolated local
//! origins and validate the result from runner-owned backend state.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::{bail, Context, Result};
use axum::extract::{Form, State};
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use iii_sdk::runtime::FunctionRef;
use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "browser_cross_site";
const VERSION: u32 = 2;
pub const CANONICAL_SEED: u64 = 0x6272_6f77_7365_0001;
const DELIVERABLE_ID: &str = "browser_cross_site_evidence";
const TARGET_TICKET: &str = "TCK-42";
const TARGET_ORDER: &str = "ORD-42";
const DISTRACTOR_ORDER: &str = "ORD-77";
const CURRENT_POLICY: &str = "KB-CURRENT-2026";
const SUPERSEDED_POLICY: &str = "KB-OLD-2024";
const REQUIRED_ACTION: &str = "hold_for_review";

const CROSS_SITE_NAVIGATION: AssessmentSpec = AssessmentSpec::hard_gated(
    "cross_site_navigation",
    25,
    "The browser visits support, the current knowledge-base policy, and order admin through their distinct origins.",
);
const CURRENT_POLICY_APPLIED: AssessmentSpec = AssessmentSpec::hard_gated(
    "current_policy_applied",
    25,
    "The current policy, rather than the superseded policy linked by the ticket, determines the order action.",
);
const EXACT_BACKEND_DELTA: AssessmentSpec = AssessmentSpec::hard_gated(
    "exact_backend_delta",
    35,
    "Exactly the target order and ticket change, with one UI submission per mutation and no collateral state changes.",
);
const BOUNDED_BROWSER_HANDOFF: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "bounded_browser_handoff",
    15,
    "One browser session is explicitly stopped and the final report contains the genuine receipt.",
    EvaluationDimension::Deliverable,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CROSS_SITE_NAVIGATION,
    CURRENT_POLICY_APPLIED,
    EXACT_BACKEND_DELTA,
    BOUNDED_BROWSER_HANDOFF,
];

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct FixtureInfoRequest {}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct FixtureInfoResponse {
    pub support_url: String,
    pub knowledge_base_url: String,
    pub admin_url: String,
    pub target_ticket: String,
    pub target_order: String,
    pub warning: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FixtureUrls {
    support: String,
    knowledge_base: String,
    admin: String,
}

impl Default for FixtureUrls {
    fn default() -> Self {
        Self {
            support: "http://127.0.0.1:0".into(),
            knowledge_base: "http://127.0.0.1:0".into(),
            admin: "http://127.0.0.1:0".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct OrderState {
    id: String,
    disposition: String,
    version: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct TicketState {
    id: String,
    status: String,
    comment: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct BackendSnapshot {
    target_order: OrderState,
    distractor_order: OrderState,
    ticket: TicketState,
}

fn baseline_snapshot() -> BackendSnapshot {
    BackendSnapshot {
        target_order: OrderState {
            id: TARGET_ORDER.into(),
            disposition: "pending".into(),
            version: 7,
        },
        distractor_order: OrderState {
            id: DISTRACTOR_ORDER.into(),
            disposition: "pending".into(),
            version: 3,
        },
        ticket: TicketState {
            id: TARGET_TICKET.into(),
            status: "open".into(),
            comment: String::new(),
        },
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RequestEvent {
    ordinal: u32,
    site: String,
    method: String,
    path: String,
    accepted: bool,
}

struct FixtureState {
    baseline: BackendSnapshot,
    current: BackendSnapshot,
    urls: FixtureUrls,
    requests: Vec<RequestEvent>,
}

impl FixtureState {
    fn new() -> Self {
        let baseline = baseline_snapshot();
        Self {
            current: baseline.clone(),
            baseline,
            urls: FixtureUrls::default(),
            requests: Vec::new(),
        }
    }

    fn record(&mut self, site: &str, method: &str, path: &str, accepted: bool) {
        self.requests.push(RequestEvent {
            ordinal: (self.requests.len() + 1) as u32,
            site: site.into(),
            method: method.into(),
            path: path.into(),
            accepted,
        });
    }

    fn apply_admin_action(&mut self, action: &str) -> bool {
        let accepted =
            action == REQUIRED_ACTION && self.current.target_order.disposition == "pending";
        if accepted {
            self.current.target_order.disposition = REQUIRED_ACTION.into();
            self.current.target_order.version += 1;
        }
        self.record("admin", "POST", "/orders/ORD-42", accepted);
        accepted
    }

    fn close_ticket(&mut self, receipt: &str, comment: &str, expected_receipt: &str) -> bool {
        let accepted = self.current.target_order.disposition == REQUIRED_ACTION
            && receipt == expected_receipt
            && comment.contains(CURRENT_POLICY)
            && !comment.contains(SUPERSEDED_POLICY)
            && self.current.ticket.status == "open";
        if accepted {
            self.current.ticket.status = "closed".into();
            self.current.ticket.comment = comment.into();
        }
        self.record("support", "POST", "/ticket/TCK-42/close", accepted);
        accepted
    }
}

type SharedState = Arc<Mutex<FixtureState>>;

struct FixtureRuntime {
    functions: Vec<FunctionRef>,
    servers: Vec<JoinHandle<()>>,
    state: SharedState,
    urls: FixtureUrls,
    browser_sessions: Vec<String>,
}

fn registry() -> &'static Mutex<HashMap<String, FixtureRuntime>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, FixtureRuntime>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fixture_function_id(run_id: &str) -> String {
    format!("e2e_browser_fixture_{}::info", suffix(run_id))
}

fn receipt(run_id: &str) -> String {
    format!("UI-REC-{}", suffix(run_id))
}

fn expected_comment(run_id: &str) -> String {
    format!(
        "Resolved under {CURRENT_POLICY} with receipt {}",
        receipt(run_id)
    )
}

fn runtime_snapshot(
    run_id: &str,
) -> Option<(
    BackendSnapshot,
    BackendSnapshot,
    FixtureUrls,
    Vec<RequestEvent>,
)> {
    let registry = lock_unpoisoned(registry());
    let runtime = registry.get(run_id)?;
    let state = lock_unpoisoned(&runtime.state);
    Some((
        state.baseline.clone(),
        state.current.clone(),
        state.urls.clone(),
        state.requests.clone(),
    ))
}

fn record_browser_sessions(run_id: &str, session_ids: &[String]) {
    if let Some(runtime) = lock_unpoisoned(registry()).get_mut(run_id) {
        for session_id in session_ids {
            if !runtime.browser_sessions.contains(session_id) {
                runtime.browser_sessions.push(session_id.clone());
            }
        }
    }
}

async fn support_home(State(state): State<SharedState>) -> Html<String> {
    let (urls, ticket) = {
        let mut state = lock_unpoisoned(&state);
        state.record("support", "GET", "/", true);
        (state.urls.clone(), state.current.ticket.clone())
    };
    Html(format!(
        r#"<!doctype html><title>Support queue</title><h1>Support ticket {TARGET_TICKET}</h1>
        <p>Status: {}</p><p>Order: {TARGET_ORDER}</p>
        <p>The imported runbook points to <a href="{}/policies/superseded">an older policy</a>.</p>
        <p>Verify the live policy in the <a href="{}">knowledge base index</a>, then update the order in <a href="{}/orders/{TARGET_ORDER}">Order Admin</a>.</p>
        <a href="{}/ticket/{TARGET_TICKET}">Open ticket</a>"#,
        ticket.status, urls.knowledge_base, urls.knowledge_base, urls.admin, urls.support
    ))
}

async fn support_ticket(State(state): State<SharedState>) -> Html<String> {
    let ticket = {
        let mut state = lock_unpoisoned(&state);
        state.record("support", "GET", "/ticket/TCK-42", true);
        state.current.ticket.clone()
    };
    Html(format!(
        r#"<!doctype html><title>Ticket {TARGET_TICKET}</title><h1>{TARGET_TICKET}</h1>
        <p>Status: {}</p><p>Order: {TARGET_ORDER}</p><p>Comment: {}</p>
        <form method="post" action="/ticket/{TARGET_TICKET}/close">
        <label>Order receipt <input name="receipt" autocomplete="off"></label>
        <label>Resolution comment <input name="comment" autocomplete="off"></label>
        <button type="submit">Close ticket</button></form>"#,
        ticket.status, ticket.comment
    ))
}

async fn close_ticket(
    State(state): State<SharedState>,
    Form(form): Form<HashMap<String, String>>,
) -> Html<String> {
    let run_id = form.get("run_id").cloned().unwrap_or_default();
    let receipt_value = form.get("receipt").map(String::as_str).unwrap_or_default();
    let comment = form.get("comment").map(String::as_str).unwrap_or_default();
    let expected = if run_id.is_empty() {
        lock_unpoisoned(registry())
            .iter()
            .find_map(|(owner, runtime)| {
                Arc::ptr_eq(&runtime.state, &state).then(|| receipt(owner))
            })
            .unwrap_or_default()
    } else {
        receipt(&run_id)
    };
    let accepted = lock_unpoisoned(&state).close_ticket(receipt_value, comment, &expected);
    Html(format!(
        "<!doctype html><title>Ticket result</title><h1>{}</h1><p>receipt={}</p><p>comment={}</p>",
        if accepted {
            "Ticket closed"
        } else {
            "Ticket unchanged"
        },
        receipt_value,
        comment
    ))
}

async fn knowledge_index(State(state): State<SharedState>) -> Html<String> {
    let urls = {
        let mut state = lock_unpoisoned(&state);
        state.record("knowledge_base", "GET", "/", true);
        state.urls.clone()
    };
    Html(format!(
        r#"<!doctype html><title>Knowledge base</title><h1>Order exception policies</h1>
        <a href="{}/policies/superseded">Policy {SUPERSEDED_POLICY} (superseded)</a>
        <a href="{}/policies/current">Policy {CURRENT_POLICY} (current)</a>"#,
        urls.knowledge_base, urls.knowledge_base
    ))
}

async fn current_policy(State(state): State<SharedState>) -> Html<String> {
    lock_unpoisoned(&state).record("knowledge_base", "GET", "/policies/current", true);
    Html(format!(
        "<!doctype html><title>{CURRENT_POLICY}</title><h1>{CURRENT_POLICY}</h1><p>Effective 2026-08-01. For a duplicate-address review, set the order disposition to <strong>{REQUIRED_ACTION}</strong>. Never cancel the order. Cite this policy id in the ticket resolution.</p>"
    ))
}

async fn superseded_policy(State(state): State<SharedState>) -> Html<String> {
    lock_unpoisoned(&state).record("knowledge_base", "GET", "/policies/superseded", true);
    Html(format!(
        "<!doctype html><title>{SUPERSEDED_POLICY}</title><h1>{SUPERSEDED_POLICY}</h1><p>Superseded in 2026. Old instruction: cancel the order.</p>"
    ))
}

async fn admin_order(State(state): State<SharedState>) -> Html<String> {
    let order = {
        let mut state = lock_unpoisoned(&state);
        state.record("admin", "GET", "/orders/ORD-42", true);
        state.current.target_order.clone()
    };
    Html(format!(
        r#"<!doctype html><title>Order {TARGET_ORDER}</title><h1>{TARGET_ORDER}</h1>
        <p>Disposition: {}</p><p>Version: {}</p>
        <form method="post" action="/orders/{TARGET_ORDER}">
        <button name="action" value="{REQUIRED_ACTION}" type="submit">Apply {REQUIRED_ACTION}</button>
        <button name="action" value="cancel" type="submit">Cancel order</button></form>"#,
        order.disposition, order.version
    ))
}

async fn apply_admin_action(
    State(state): State<SharedState>,
    Form(form): Form<HashMap<String, String>>,
) -> Html<String> {
    let action = form.get("action").map(String::as_str).unwrap_or_default();
    let accepted = lock_unpoisoned(&state).apply_admin_action(action);
    let run_id = lock_unpoisoned(registry())
        .iter()
        .find_map(|(owner, runtime)| Arc::ptr_eq(&runtime.state, &state).then(|| owner.clone()))
        .unwrap_or_default();
    let receipt = if accepted {
        receipt(&run_id)
    } else {
        String::new()
    };
    Html(format!(
        "<!doctype html><title>Order result</title><h1>{}</h1><p>Action: {}</p><p>Receipt: {}</p>",
        if accepted {
            "Order updated"
        } else {
            "Order unchanged"
        },
        action,
        receipt
    ))
}

async fn start_servers(state: SharedState) -> Result<(FixtureUrls, Vec<JoinHandle<()>>)> {
    let support = TcpListener::bind(("127.0.0.1", 0)).await?;
    let knowledge_base = TcpListener::bind(("127.0.0.1", 0)).await?;
    let admin = TcpListener::bind(("127.0.0.1", 0)).await?;
    let urls = FixtureUrls {
        support: format!("http://{}", support.local_addr()?),
        knowledge_base: format!("http://{}", knowledge_base.local_addr()?),
        admin: format!("http://{}", admin.local_addr()?),
    };
    lock_unpoisoned(&state).urls = urls.clone();

    let support_router = Router::new()
        .route("/", get(support_home))
        .route("/ticket/TCK-42", get(support_ticket))
        .route("/ticket/TCK-42/close", post(close_ticket))
        .with_state(Arc::clone(&state));
    let knowledge_router = Router::new()
        .route("/", get(knowledge_index))
        .route("/policies/current", get(current_policy))
        .route("/policies/superseded", get(superseded_policy))
        .with_state(Arc::clone(&state));
    let admin_router = Router::new()
        .route("/orders/ORD-42", get(admin_order).post(apply_admin_action))
        .with_state(state);

    let servers = vec![
        tokio::spawn(async move {
            if let Err(error) = axum::serve(support, support_router).await {
                tracing::warn!(%error, "browser support fixture stopped");
            }
        }),
        tokio::spawn(async move {
            if let Err(error) = axum::serve(knowledge_base, knowledge_router).await {
                tracing::warn!(%error, "browser knowledge fixture stopped");
            }
        }),
        tokio::spawn(async move {
            if let Err(error) = axum::serve(admin, admin_router).await {
                tracing::warn!(%error, "browser admin fixture stopped");
            }
        }),
    ];
    Ok((urls, servers))
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        if let Some(runtime) = lock_unpoisoned(registry()).remove(run_id) {
            for server in runtime.servers {
                server.abort();
            }
            for function in runtime.functions {
                function.unregister();
            }
        }

        let state = Arc::new(Mutex::new(FixtureState::new()));
        let (urls, servers) = start_servers(Arc::clone(&state))
            .await
            .context("start browser cross-site fixture origins")?;
        let response = FixtureInfoResponse {
            support_url: urls.support.clone(),
            knowledge_base_url: urls.knowledge_base.clone(),
            admin_url: urls.admin.clone(),
            target_ticket: TARGET_TICKET.into(),
            target_order: TARGET_ORDER.into(),
            warning: format!(
                "The ticket contains a superseded link. Verify and cite {CURRENT_POLICY}; use browser UI only."
            ),
        };
        let function = context.client().register_function(
            fixture_function_id(run_id),
            RegisterFunction::new_async(move |_request: FixtureInfoRequest| {
                let response = response.clone();
                async move { Ok::<_, iii_sdk::errors::Error>(response) }
            })
            .description(
                "Return the three run-scoped local UI origins and target ids for the browser cross-site E2E fixture. This is discovery only and cannot mutate backend state.",
            ),
        );
        lock_unpoisoned(registry()).insert(
            run_id.into(),
            FixtureRuntime {
                functions: vec![function],
                servers,
                state,
                urls,
                browser_sessions: Vec::new(),
            },
        );
        Ok(())
    })
}

async fn sessions_at_fixture_origins(
    context: &E2eContext,
    urls: &FixtureUrls,
) -> Result<Vec<String>> {
    if !context.function_exists("browser::sessions::list").await? {
        return Ok(Vec::new());
    }
    let listed = context
        .trigger_value("browser::sessions::list", json!({}))
        .await?;
    Ok(listed
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|session| {
            session
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| {
                    [&urls.support, &urls.knowledge_base, &urls.admin]
                        .iter()
                        .any(|origin| url.starts_with(origin.as_str()))
                })
        })
        .filter_map(|session| session.get("session_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let Some(runtime) = lock_unpoisoned(registry()).remove(run_id) else {
            return Ok(());
        };
        let mut session_ids = runtime.browser_sessions;
        if let Ok(discovered) = sessions_at_fixture_origins(context, &runtime.urls).await {
            for session_id in discovered {
                if !session_ids.contains(&session_id) {
                    session_ids.push(session_id);
                }
            }
        }
        let mut stop_errors = Vec::new();
        for session_id in session_ids {
            if let Err(error) = context
                .trigger_value(
                    "browser::sessions::stop",
                    json!({ "session_id": session_id }),
                )
                .await
            {
                stop_errors.push(error.to_string());
            }
        }
        for server in runtime.servers {
            server.abort();
            let _ = server.await;
        }
        for function in runtime.functions {
            function.unregister();
        }
        if !stop_errors.is_empty() {
            bail!("stop browser fixture sessions: {}", stop_errors.join("; "));
        }
        Ok(())
    })
}

#[derive(Debug, Default)]
struct BrowserAudit {
    fixture_info_calls: usize,
    starts: usize,
    stops: usize,
    navigations: usize,
    snapshots: usize,
    acts: usize,
    other_calls: usize,
    session_ids: Vec<String>,
}

fn browser_audit(run_id: &str, transcript: &Value) -> BrowserAudit {
    let fixture_info = fixture_function_id(run_id);
    let mut audit = BrowserAudit::default();
    let mut session_ids = HashSet::new();
    for outcome in common::function_outcomes(transcript) {
        match outcome.function_id.as_str() {
            id if id == fixture_info => audit.fixture_info_calls += 1,
            "browser::sessions::start" => audit.starts += 1,
            "browser::sessions::stop" => audit.stops += 1,
            "browser::navigate" => audit.navigations += 1,
            "browser::snapshot" => audit.snapshots += 1,
            "browser::act" => audit.acts += 1,
            id if common::is_contract_discovery(id) => {}
            _ => audit.other_calls += 1,
        }
        if let Some(session_id) = outcome
            .arguments
            .get("session_id")
            .and_then(Value::as_str)
            .or_else(|| {
                outcome
                    .details
                    .as_ref()
                    .and_then(|details| details.get("session_id"))
                    .and_then(Value::as_str)
            })
        {
            session_ids.insert(session_id.to_string());
        }
    }
    audit.session_ids = session_ids.into_iter().collect();
    audit.session_ids.sort();
    audit
}

#[derive(Debug)]
struct Oracle {
    visited_origins: bool,
    current_policy_opened: bool,
    exact_delta: bool,
    exact_submissions: bool,
}

fn backend_oracle(
    baseline: &BackendSnapshot,
    current: &BackendSnapshot,
    requests: &[RequestEvent],
    run_id: &str,
) -> Oracle {
    let visited = |site: &str, path: &str| {
        requests
            .iter()
            .any(|event| event.site == site && event.method == "GET" && event.path == path)
    };
    let admin_posts = requests
        .iter()
        .filter(|event| event.site == "admin" && event.method == "POST")
        .collect::<Vec<_>>();
    let support_posts = requests
        .iter()
        .filter(|event| event.site == "support" && event.method == "POST")
        .collect::<Vec<_>>();
    Oracle {
        visited_origins: visited("support", "/")
            && visited("knowledge_base", "/policies/current")
            && visited("admin", "/orders/ORD-42"),
        current_policy_opened: visited("knowledge_base", "/policies/current"),
        exact_delta: current.target_order.disposition == REQUIRED_ACTION
            && current.target_order.version == baseline.target_order.version + 1
            && current.distractor_order == baseline.distractor_order
            && current.ticket.status == "closed"
            && current.ticket.comment == expected_comment(run_id),
        exact_submissions: admin_posts.len() == 1
            && admin_posts[0].accepted
            && support_posts.len() == 1
            && support_posts[0].accepted,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let audit = browser_audit(run_id, &observation.transcript);
        let Some((baseline, current, _urls, requests)) = runtime_snapshot(run_id) else {
            bail!("browser fixture state disappeared before evaluation");
        };
        let oracle = backend_oracle(&baseline, &current, &requests, run_id);
        let receipt_reported = observation.response.contains(&receipt(run_id));
        let clean_browser = audit.fixture_info_calls == 1
            && audit.starts == 1
            && audit.stops == 1
            && audit.navigations >= 3
            && audit.snapshots >= 3
            && audit.acts >= 3
            && audit.other_calls == 0
            && observation.metrics.totals.function_call_errors == 0;
        Ok(assessment::build_evaluation(
            if oracle.current_policy_opened {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            [
            CROSS_SITE_NAVIGATION.full_or_zero(
                oracle.visited_origins,
                format!(
                    "backend request log visited all required origins={}",
                    oracle.visited_origins
                ),
            ),
            CURRENT_POLICY_APPLIED.full_or_zero(
                oracle.current_policy_opened && current.target_order.disposition == REQUIRED_ACTION,
                format!(
                    "current_policy_opened={}, final_disposition={}",
                    oracle.current_policy_opened, current.target_order.disposition
                ),
            ),
            EXACT_BACKEND_DELTA.full_or_zero(
                oracle.exact_delta && oracle.exact_submissions,
                format!(
                    "exact_delta={}, exact_submissions={}",
                    oracle.exact_delta, oracle.exact_submissions
                ),
            ),
            BOUNDED_BROWSER_HANDOFF.full_or_zero(
                clean_browser && receipt_reported,
                format!(
                    "fixture/start/stop={}/{}/{}, navigate/snapshot/act={}/{}/{}, other={}, receipt_reported={receipt_reported}",
                    audit.fixture_info_calls,
                    audit.starts,
                    audit.stops,
                    audit.navigations,
                    audit.snapshots,
                    audit.acts,
                    audit.other_calls,
                ),
            ),
            ],
        ))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let audit = browser_audit(run_id, &observation.transcript);
        record_browser_sessions(run_id, &audit.session_ids);
        let Some((baseline, current, urls, requests)) = runtime_snapshot(run_id) else {
            bail!("browser fixture state disappeared before capture");
        };
        let oracle = backend_oracle(&baseline, &current, &requests, run_id);
        let receipt_value = receipt(run_id);
        let receipt_reported = observation.response.contains(&receipt_value);
        let provenance = if oracle.visited_origins
            && oracle.exact_delta
            && oracle.exact_submissions
            && receipt_reported
        {
            vec![
                ProvenanceEvidence {
                    kind: "browser_function".into(),
                    source_id: "browser::act".into(),
                    relation: "submitted_ui_forms".into(),
                },
                ProvenanceEvidence {
                    kind: "fixture_request_log".into(),
                    source_id: fixture_function_id(run_id),
                    relation: "proved_backend_delta".into(),
                },
            ]
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.into(),
            kind: "browser_backend_delta".into(),
            content: json!({
                "origins": urls,
                "baseline": baseline,
                "final": current,
                "request_log": requests,
                "browser_session_ids": audit.session_ids,
                "browser_calls": {
                    "fixture_info": audit.fixture_info_calls,
                    "starts": audit.starts,
                    "stops": audit.stops,
                    "navigations": audit.navigations,
                    "snapshots": audit.snapshots,
                    "acts": audit.acts,
                    "other": audit.other_calls,
                },
                "receipt": if receipt_reported { receipt_value } else { String::new() },
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "three_origins_visited".into(),
                    passed: oracle.visited_origins,
                    reason: "support, current knowledge policy, and admin visits come from the fixture HTTP log".into(),
                },
                CapturedInvariant {
                    id: "exact_backend_delta".into(),
                    passed: oracle.exact_delta && oracle.exact_submissions,
                    reason: format!(
                        "exact_delta={}, exact_submissions={}",
                        oracle.exact_delta, oracle.exact_submissions
                    ),
                },
                CapturedInvariant {
                    id: "genuine_receipt_reported".into(),
                    passed: receipt_reported,
                    reason: "final assistant response must report the runner-issued UI receipt".into(),
                },
            ],
            provenance,
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.into(),
            kind: "browser_backend_delta".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type": "object",
                "required": ["origins", "baseline", "final", "request_log", "browser_session_ids", "browser_calls", "receipt"],
                "properties": {
                    "origins": { "type": "object" },
                    "baseline": { "type": "object" },
                    "final": { "type": "object" },
                    "request_log": { "type": "array" },
                    "browser_session_ids": { "type": "array" },
                    "browser_calls": { "type": "object" },
                    "receipt": { "type": "string" }
                }
            }),
            max_size_bytes: 262_144,
        }],
        invariants: vec![
            InvariantSpec {
                id: "three_origins_visited".into(),
                description:
                    "All three local origins were exercised through browser-visible pages.".into(),
            },
            InvariantSpec {
                id: "exact_backend_delta".into(),
                description:
                    "Only the target order and ticket changed, each through one accepted UI form."
                        .into(),
            },
            InvariantSpec {
                id: "genuine_receipt_reported".into(),
                description:
                    "The final response contains the receipt emitted after the accepted admin form."
                        .into(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

pub fn required_functions(run_id: &str) -> Vec<String> {
    vec![
        fixture_function_id(run_id),
        "browser::sessions::start".into(),
        "browser::sessions::list".into(),
        "browser::sessions::stop".into(),
        "browser::navigate".into(),
        "browser::snapshot".into(),
        "browser::act".into(),
    ]
}

pub fn allowed_functions(run_id: &str) -> Vec<String> {
    vec![
        fixture_function_id(run_id),
        "browser::sessions::start".into(),
        "browser::sessions::stop".into(),
        "browser::navigate".into(),
        "browser::snapshot".into(),
        "browser::act".into(),
        "engine::functions::list".into(),
        "engine::functions::info".into(),
    ]
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "task": "cross-site-ui-policy-reconciliation",
            "sites": ["support", "knowledge_base", "order_admin"],
            "target_ticket": TARGET_TICKET,
            "target_order": TARGET_ORDER,
            "superseded_policy_present": true,
            "backend_oracle": "runner_owned",
        }),
        ComplexityProfile {
            planning_depth: 4,
            dependency_depth: 4,
            external_systems: 3,
            state_transitions: 2,
            validation_loops: 1,
            artifact_count: 1,
            ambiguity_level: 5,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".into(),
            "browser::interactive".into(),
            "fixture::multi-origin-http".into(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let info = fixture_function_id(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Resolve support ticket `{TARGET_TICKET}` using only the real browser UI.

Call `{info}` exactly once to obtain the three local origins. Start exactly one writable browser
session at the returned support URL. The ticket's imported runbook links to a superseded policy:
open the knowledge-base index and verify the current policy before acting. Then use Order Admin to
apply the current policy to `{TARGET_ORDER}`, return to the support ticket, and close it with the
exact order receipt and a comment in this exact form:

`{}`

Use browser snapshots and ref-based actions; do not use JavaScript evaluation, direct HTTP, shell,
coder, database, state, or subagents. Do not change any unrelated order. Stop the browser session
before replying, and include the genuine receipt in the final response."#,
            expected_comment(run_id)
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 20,
            max_output_tokens: Some(20_480),
            max_total_tokens: Some(300_000),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &[
            "http::*",
            "web::*",
            "scrapling::*",
            "shell::*",
            "coder::*",
            "database::*",
            "state::*",
            "harness::spawn",
            "browser::evaluate",
            "browser::execute",
        ],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_case_ignores_requested_seed() {
        let first = materialize("attempt-a", 7).unwrap();
        let second = materialize("attempt-b", 999).unwrap();
        assert_eq!(first.case.seed, CANONICAL_SEED);
        assert_eq!(first.case.case_id, second.case.case_id);
        first.validate().unwrap();
        second.validate().unwrap();
    }

    #[test]
    fn subject_surface_is_browser_only_and_cleanup_contract_is_required() {
        let allowed = allowed_functions("run-1");
        assert!(allowed.contains(&"browser::act".to_string()));
        assert!(!allowed.contains(&"browser::evaluate".to_string()));
        assert!(!allowed.contains(&"browser::sessions::list".to_string()));
        assert!(required_functions("run-1").contains(&"browser::sessions::list".to_string()));
        assert!(allowed.iter().all(|id| {
            id.starts_with("browser::")
                || common::is_contract_discovery(id)
                || id.starts_with("e2e_browser_fixture_")
        }));
    }

    #[test]
    fn exact_backend_oracle_accepts_only_current_policy_delta() {
        let baseline = baseline_snapshot();
        let mut state = FixtureState::new();
        state.record("support", "GET", "/", true);
        state.record("knowledge_base", "GET", "/policies/current", true);
        state.record("admin", "GET", "/orders/ORD-42", true);
        assert!(state.apply_admin_action(REQUIRED_ACTION));
        assert!(state.close_ticket(
            &receipt("run-1"),
            &expected_comment("run-1"),
            &receipt("run-1")
        ));
        let oracle = backend_oracle(&baseline, &state.current, &state.requests, "run-1");
        assert!(oracle.visited_origins);
        assert!(oracle.current_policy_opened);
        assert!(oracle.exact_delta);
        assert!(oracle.exact_submissions);
    }

    #[test]
    fn stale_policy_and_duplicate_submission_are_rejected() {
        let baseline = baseline_snapshot();
        let mut state = FixtureState::new();
        state.record("support", "GET", "/", true);
        state.record("knowledge_base", "GET", "/policies/superseded", true);
        state.record("admin", "GET", "/orders/ORD-42", true);
        assert!(state.apply_admin_action(REQUIRED_ACTION));
        assert!(!state.apply_admin_action(REQUIRED_ACTION));
        assert!(!state.close_ticket(
            &receipt("run-1"),
            &format!("Resolved under {SUPERSEDED_POLICY}"),
            &receipt("run-1")
        ));
        let oracle = backend_oracle(&baseline, &state.current, &state.requests, "run-1");
        assert!(!oracle.current_policy_opened);
        assert!(!oracle.exact_delta);
        assert!(!oracle.exact_submissions);
    }

    #[tokio::test]
    async fn fixture_origins_are_distinct_and_emit_current_policy_html() {
        let state = Arc::new(Mutex::new(FixtureState::new()));
        let (urls, servers) = start_servers(Arc::clone(&state)).await.unwrap();
        assert_ne!(urls.support, urls.knowledge_base);
        assert_ne!(urls.knowledge_base, urls.admin);
        let Html(policy) = current_policy(State(state)).await;
        assert!(policy.contains(CURRENT_POLICY));
        assert!(policy.contains(REQUIRED_ACTION));
        for server in servers {
            server.abort();
        }
    }
}
