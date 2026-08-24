//! `cross_app_transaction` — converge one account across three independently
//! versioned services while recovering from a deterministic CAS conflict.
//!
//! The fixture is hosted by run-scoped functions registered on the suite's
//! engine connection. The evaluator never trusts the model's prose: it reads
//! the fixture snapshot and its append-only audit log directly.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use iii_sdk::runtime::FunctionRef;
use iii_sdk::RegisterFunction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "cross_app_transaction";
pub const CANONICAL_SEED: u64 = 0x6372_6f73_7361_7070;
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "cross_app_transaction_audit";
const TARGET_ACCOUNT: &str = "acct-42";
const SENTINEL_ACCOUNT: &str = "acct-99";
const TARGET_TICKET: &str = "ticket-42";
const SENTINEL_TICKET: &str = "ticket-99";
const INITIAL_TEAM: &str = "team-red";
const TARGET_TEAM: &str = "team-blue";

const CONSISTENT_FINAL_STATE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "consistent_final_state",
    35,
    "CRM and billing converge on team-blue and the support ticket closes only afterward.",
    EvaluationDimension::Deliverable,
);
const CONFLICT_RECOVERED: AssessmentSpec = AssessmentSpec::hard_gated(
    "conflict_recovered",
    25,
    "The injected billing CAS conflict is observed exactly once and recovered using the new revision.",
);
const NO_COLLATERAL_MUTATION: AssessmentSpec = AssessmentSpec::hard_gated(
    "no_collateral_mutation",
    25,
    "Only the three authorized mutations occur; sentinel account and ticket remain byte-for-byte unchanged.",
);
const AUTHENTIC_RECEIPTS: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "authentic_receipts",
    15,
    "The final response contains the three receipts issued by the services and no fabricated transaction receipt.",
    EvaluationDimension::Deliverable,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CONSISTENT_FINAL_STATE,
    CONFLICT_RECOVERED,
    NO_COLLATERAL_MUTATION,
    AUTHENTIC_RECEIPTS,
];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetAccountRequest {
    pub account_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TransferAccountRequest {
    pub account_id: String,
    pub new_team: String,
    pub expected_version: u64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetTicketRequest {
    pub ticket_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CloseTicketRequest {
    pub ticket_id: String,
    pub expected_version: u64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ServiceResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct VersionedTeam {
    team: String,
    version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct VersionedTicket {
    status: String,
    version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CrossAppSnapshot {
    crm_target: VersionedTeam,
    billing_target: VersionedTeam,
    support_target: VersionedTicket,
    crm_sentinel: VersionedTeam,
    billing_sentinel: VersionedTeam,
    support_sentinel: VersionedTicket,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuditEntry {
    ordinal: u64,
    service: String,
    operation: String,
    entity_id: String,
    outcome: String,
    mutated: bool,
    receipt: Option<String>,
}

#[derive(Debug)]
struct CrossAppState {
    run_id: String,
    snapshot: CrossAppSnapshot,
    audit: Vec<AuditEntry>,
    idempotency_receipts: BTreeMap<String, String>,
    billing_conflict_injected: bool,
}

impl CrossAppState {
    fn new(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            snapshot: initial_snapshot(),
            audit: Vec::new(),
            idempotency_receipts: BTreeMap::new(),
            billing_conflict_injected: false,
        }
    }

    fn record(
        &mut self,
        service: &str,
        operation: &str,
        entity_id: &str,
        outcome: &str,
        mutated: bool,
        receipt: Option<String>,
    ) {
        self.audit.push(AuditEntry {
            ordinal: self.audit.len() as u64 + 1,
            service: service.to_string(),
            operation: operation.to_string(),
            entity_id: entity_id.to_string(),
            outcome: outcome.to_string(),
            mutated,
            receipt,
        });
    }

    fn get_account(&mut self, service: &str, account_id: &str) -> ServiceResponse {
        let account = match (service, account_id) {
            ("crm", TARGET_ACCOUNT) => Some(self.snapshot.crm_target.clone()),
            ("crm", SENTINEL_ACCOUNT) => Some(self.snapshot.crm_sentinel.clone()),
            ("billing", TARGET_ACCOUNT) => Some(self.snapshot.billing_target.clone()),
            ("billing", SENTINEL_ACCOUNT) => Some(self.snapshot.billing_sentinel.clone()),
            _ => None,
        };
        let outcome = if account.is_some() { "ok" } else { "not_found" };
        self.record(service, "get", account_id, outcome, false, None);
        match account {
            Some(account) => ServiceResponse {
                status: "ok".to_string(),
                entity_id: Some(account_id.to_string()),
                team: Some(account.team),
                ticket_status: None,
                version: Some(account.version),
                receipt: None,
                message: None,
            },
            None => rejected("not_found", "unknown account"),
        }
    }

    fn transfer(&mut self, service: &str, request: TransferAccountRequest) -> ServiceResponse {
        let operation_key = format!("{service}:transfer:{}", request.idempotency_key);
        if let Some(receipt) = self.idempotency_receipts.get(&operation_key).cloned() {
            self.record(
                service,
                "transfer",
                &request.account_id,
                "idempotent_replay",
                false,
                Some(receipt.clone()),
            );
            return accepted(&request.account_id, TARGET_TEAM, None, receipt);
        }
        if request.account_id != TARGET_ACCOUNT || request.new_team != TARGET_TEAM {
            self.record(
                service,
                "transfer",
                &request.account_id,
                "rejected",
                false,
                None,
            );
            return rejected("rejected", "only acct-42 may move to team-blue");
        }

        if service == "billing" && !self.billing_conflict_injected {
            self.billing_conflict_injected = true;
            self.snapshot.billing_target.version += 1;
            let version = self.snapshot.billing_target.version;
            self.record(
                service,
                "transfer",
                &request.account_id,
                "version_conflict",
                false,
                None,
            );
            return ServiceResponse {
                status: "version_conflict".to_string(),
                entity_id: Some(request.account_id),
                team: Some(self.snapshot.billing_target.team.clone()),
                ticket_status: None,
                version: Some(version),
                receipt: None,
                message: Some("billing changed concurrently; re-read and retry".to_string()),
            };
        }

        let current = match service {
            "crm" => &mut self.snapshot.crm_target,
            "billing" => &mut self.snapshot.billing_target,
            _ => {
                self.record(
                    service,
                    "transfer",
                    &request.account_id,
                    "rejected",
                    false,
                    None,
                );
                return rejected("rejected", "unknown service");
            }
        };
        if request.expected_version != current.version {
            let version = current.version;
            let team = current.team.clone();
            self.record(
                service,
                "transfer",
                &request.account_id,
                "version_conflict",
                false,
                None,
            );
            return ServiceResponse {
                status: "version_conflict".to_string(),
                entity_id: Some(request.account_id),
                team: Some(team),
                ticket_status: None,
                version: Some(version),
                receipt: None,
                message: Some("expected_version is stale".to_string()),
            };
        }
        current.team = TARGET_TEAM.to_string();
        current.version += 1;
        let version = current.version;
        let receipt = receipt(&self.run_id, service);
        self.idempotency_receipts
            .insert(operation_key, receipt.clone());
        self.record(
            service,
            "transfer",
            &request.account_id,
            "accepted",
            true,
            Some(receipt.clone()),
        );
        accepted(&request.account_id, TARGET_TEAM, Some(version), receipt)
    }

    fn get_ticket(&mut self, ticket_id: &str) -> ServiceResponse {
        let ticket = match ticket_id {
            TARGET_TICKET => Some(self.snapshot.support_target.clone()),
            SENTINEL_TICKET => Some(self.snapshot.support_sentinel.clone()),
            _ => None,
        };
        let outcome = if ticket.is_some() { "ok" } else { "not_found" };
        self.record("support", "get", ticket_id, outcome, false, None);
        match ticket {
            Some(ticket) => ServiceResponse {
                status: "ok".to_string(),
                entity_id: Some(ticket_id.to_string()),
                team: None,
                ticket_status: Some(ticket.status),
                version: Some(ticket.version),
                receipt: None,
                message: None,
            },
            None => rejected("not_found", "unknown ticket"),
        }
    }

    fn close_ticket(&mut self, request: CloseTicketRequest) -> ServiceResponse {
        let operation_key = format!("support:close:{}", request.idempotency_key);
        if let Some(receipt) = self.idempotency_receipts.get(&operation_key).cloned() {
            self.record(
                "support",
                "close",
                &request.ticket_id,
                "idempotent_replay",
                false,
                Some(receipt.clone()),
            );
            return ticket_accepted(&request.ticket_id, None, receipt);
        }
        if request.ticket_id != TARGET_TICKET {
            self.record(
                "support",
                "close",
                &request.ticket_id,
                "rejected",
                false,
                None,
            );
            return rejected("rejected", "only ticket-42 may be closed");
        }
        if self.snapshot.crm_target.team != TARGET_TEAM
            || self.snapshot.billing_target.team != TARGET_TEAM
        {
            self.record(
                "support",
                "close",
                &request.ticket_id,
                "dependencies_not_converged",
                false,
                None,
            );
            return rejected(
                "dependencies_not_converged",
                "CRM and billing must both show team-blue",
            );
        }
        if request.expected_version != self.snapshot.support_target.version {
            let version = self.snapshot.support_target.version;
            self.record(
                "support",
                "close",
                &request.ticket_id,
                "version_conflict",
                false,
                None,
            );
            return ServiceResponse {
                status: "version_conflict".to_string(),
                entity_id: Some(request.ticket_id),
                team: None,
                ticket_status: Some(self.snapshot.support_target.status.clone()),
                version: Some(version),
                receipt: None,
                message: Some("expected_version is stale".to_string()),
            };
        }
        self.snapshot.support_target.status = "closed".to_string();
        self.snapshot.support_target.version += 1;
        let version = self.snapshot.support_target.version;
        let receipt = receipt(&self.run_id, "support");
        self.idempotency_receipts
            .insert(operation_key, receipt.clone());
        self.record(
            "support",
            "close",
            &request.ticket_id,
            "accepted",
            true,
            Some(receipt.clone()),
        );
        ticket_accepted(&request.ticket_id, Some(version), receipt)
    }
}

fn rejected(status: &str, message: &str) -> ServiceResponse {
    ServiceResponse {
        status: status.to_string(),
        entity_id: None,
        team: None,
        ticket_status: None,
        version: None,
        receipt: None,
        message: Some(message.to_string()),
    }
}

fn accepted(entity_id: &str, team: &str, version: Option<u64>, receipt: String) -> ServiceResponse {
    ServiceResponse {
        status: "accepted".to_string(),
        entity_id: Some(entity_id.to_string()),
        team: Some(team.to_string()),
        ticket_status: None,
        version,
        receipt: Some(receipt),
        message: None,
    }
}

fn ticket_accepted(entity_id: &str, version: Option<u64>, receipt: String) -> ServiceResponse {
    ServiceResponse {
        status: "accepted".to_string(),
        entity_id: Some(entity_id.to_string()),
        team: None,
        ticket_status: Some("closed".to_string()),
        version,
        receipt: Some(receipt),
        message: None,
    }
}

fn initial_snapshot() -> CrossAppSnapshot {
    CrossAppSnapshot {
        crm_target: VersionedTeam {
            team: INITIAL_TEAM.to_string(),
            version: 7,
        },
        billing_target: VersionedTeam {
            team: INITIAL_TEAM.to_string(),
            version: 11,
        },
        support_target: VersionedTicket {
            status: "open".to_string(),
            version: 3,
        },
        crm_sentinel: VersionedTeam {
            team: "team-green".to_string(),
            version: 19,
        },
        billing_sentinel: VersionedTeam {
            team: "team-green".to_string(),
            version: 23,
        },
        support_sentinel: VersionedTicket {
            status: "open".to_string(),
            version: 29,
        },
    }
}

type Fixture = Arc<Mutex<CrossAppState>>;

struct FixtureRuntime {
    functions: Vec<FunctionRef>,
    state: Fixture,
}

static FIXTURES: OnceLock<Mutex<BTreeMap<String, FixtureRuntime>>> = OnceLock::new();

fn fixtures() -> &'static Mutex<BTreeMap<String, FixtureRuntime>> {
    FIXTURES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fixture(run_id: &str) -> Option<Fixture> {
    lock_unpoisoned(fixtures())
        .get(run_id)
        .map(|runtime| Arc::clone(&runtime.state))
}

fn release_fixture(run_id: &str) {
    if let Some(runtime) = lock_unpoisoned(fixtures()).remove(run_id) {
        for function in runtime.functions {
            function.unregister();
        }
    }
}

fn receipt(run_id: &str, service: &str) -> String {
    format!(
        "XAPP-{}-{:016x}",
        service.to_ascii_uppercase(),
        super::stable_seed(&format!("{ID}:{run_id}:{service}:receipt"))
    )
}

fn run_suffix(run_id: &str) -> String {
    format!(
        "{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:namespace"))
    )
}

#[derive(Debug, Clone)]
struct FunctionIds {
    crm_get: String,
    crm_transfer: String,
    billing_get: String,
    billing_transfer: String,
    support_get: String,
    support_close: String,
}

impl FunctionIds {
    fn new(run_id: &str) -> Self {
        let suffix = run_suffix(run_id);
        Self {
            crm_get: format!("e2etest::crm_get_{suffix}"),
            crm_transfer: format!("e2etest::crm_transfer_{suffix}"),
            billing_get: format!("e2etest::billing_get_{suffix}"),
            billing_transfer: format!("e2etest::billing_transfer_{suffix}"),
            support_get: format!("e2etest::support_get_{suffix}"),
            support_close: format!("e2etest::support_close_{suffix}"),
        }
    }

    fn all(&self) -> [&str; 6] {
        [
            &self.crm_get,
            &self.crm_transfer,
            &self.billing_get,
            &self.billing_transfer,
            &self.support_get,
            &self.support_close,
        ]
    }
}

pub fn required_functions(run_id: &str) -> Vec<String> {
    FunctionIds::new(run_id)
        .all()
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub fn allowed_functions(run_id: &str) -> Vec<String> {
    let mut allowed = required_functions(run_id);
    allowed.extend([
        "engine::functions::list".to_string(),
        "engine::functions::info".to_string(),
    ]);
    allowed
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        release_fixture(run_id);
        let state = Arc::new(Mutex::new(CrossAppState::new(run_id)));
        let mut functions = Vec::with_capacity(6);
        let ids = FunctionIds::new(run_id);

        let crm = state.clone();
        functions.push(
            context.client().register_function(
                ids.crm_get.clone(),
                RegisterFunction::new_async(move |request: GetAccountRequest| {
                    let crm = crm.clone();
                    async move {
                        Ok::<ServiceResponse, iii_sdk::errors::Error>(
                            lock_unpoisoned(&crm).get_account("crm", &request.account_id),
                        )
                    }
                })
                .description("Run-scoped CRM account lookup with optimistic version metadata."),
            ),
        );

        let crm = state.clone();
        functions.push(
            context.client().register_function(
                ids.crm_transfer.clone(),
                RegisterFunction::new_async(move |request: TransferAccountRequest| {
                    let crm = crm.clone();
                    async move {
                        Ok::<ServiceResponse, iii_sdk::errors::Error>(
                            lock_unpoisoned(&crm).transfer("crm", request),
                        )
                    }
                })
                .description(
                    "Run-scoped CRM team transfer using expected_version and idempotency_key.",
                ),
            ),
        );

        let billing = state.clone();
        functions.push(
            context.client().register_function(
                ids.billing_get.clone(),
                RegisterFunction::new_async(move |request: GetAccountRequest| {
                    let billing = billing.clone();
                    async move {
                        Ok::<ServiceResponse, iii_sdk::errors::Error>(
                            lock_unpoisoned(&billing).get_account("billing", &request.account_id),
                        )
                    }
                })
                .description("Run-scoped billing account lookup with optimistic version metadata."),
            ),
        );

        let billing = state.clone();
        functions.push(context.client().register_function(
            ids.billing_transfer.clone(),
            RegisterFunction::new_async(move |request: TransferAccountRequest| {
                let billing = billing.clone();
                async move {
                    Ok::<ServiceResponse, iii_sdk::errors::Error>(
                        lock_unpoisoned(&billing).transfer("billing", request),
                    )
                }
            })
            .description(
                "Run-scoped billing team transfer. The first valid target attempt returns a deterministic CAS conflict.",
            ),
        ));

        let support = state.clone();
        functions.push(
            context.client().register_function(
                ids.support_get.clone(),
                RegisterFunction::new_async(move |request: GetTicketRequest| {
                    let support = support.clone();
                    async move {
                        Ok::<ServiceResponse, iii_sdk::errors::Error>(
                            lock_unpoisoned(&support).get_ticket(&request.ticket_id),
                        )
                    }
                })
                .description("Run-scoped support ticket lookup with optimistic version metadata."),
            ),
        );

        let support = Arc::clone(&state);
        functions.push(context.client().register_function(
            ids.support_close,
            RegisterFunction::new_async(move |request: CloseTicketRequest| {
                let support = Arc::clone(&support);
                async move {
                    Ok::<ServiceResponse, iii_sdk::errors::Error>(
                        lock_unpoisoned(&support).close_ticket(request),
                    )
                }
            })
            .description(
                "Run-scoped support close. It accepts only after CRM and billing have converged.",
            ),
        ));
        lock_unpoisoned(fixtures()).insert(run_id.to_string(), FixtureRuntime { functions, state });
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, _seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "account_id": TARGET_ACCOUNT,
            "ticket_id": TARGET_TICKET,
            "from_team": INITIAL_TEAM,
            "to_team": TARGET_TEAM,
            "services": ["crm", "billing", "support"],
            "injected_fault": "one billing version_conflict",
            "sentinels": [SENTINEL_ACCOUNT, SENTINEL_TICKET],
        }),
        ComplexityProfile {
            planning_depth: 3,
            dependency_depth: 3,
            external_systems: 3,
            state_transitions: 4,
            validation_loops: 1,
            artifact_count: 1,
            ambiguity_level: 3,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "e2e::run-scoped-fixtures".to_string(),
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
    let ids = FunctionIds::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Move account `{TARGET_ACCOUNT}` from `{INITIAL_TEAM}` to `{TARGET_TEAM}` consistently across CRM and billing, then close support ticket `{TARGET_TICKET}`.

Use only these run-scoped service functions (function discovery is allowed):
- CRM read `{crm_get}` and transfer `{crm_transfer}`
- billing read `{billing_get}` and transfer `{billing_transfer}`
- support read `{support_get}` and close `{support_close}`

Inspect each function contract before calling it. All mutations use optimistic `expected_version` and a stable, non-empty `idempotency_key`. A service may return `version_conflict`; on conflict, re-read that service, use the returned current version, and retry only the conflicted mutation. Never mutate `{sentinel_account}` or `{sentinel_ticket}`. Close the support ticket only after both account systems report `{TARGET_TEAM}`.

Finish with a short PASS/FAIL report containing the exact receipt returned by each successful CRM, billing, and support mutation. Do not invent receipts."#,
            crm_get = ids.crm_get,
            crm_transfer = ids.crm_transfer,
            billing_get = ids.billing_get,
            billing_transfer = ids.billing_transfer,
            support_get = ids.support_get,
            support_close = ids.support_close,
            sentinel_account = SENTINEL_ACCOUNT,
            sentinel_ticket = SENTINEL_TICKET,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 18,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(300_000),
            stuck_timeout_seconds: 360,
            max_validation_retries: None,
        },
        denied_functions: &["state::*", "database::*", "http::*", "shell::*", "coder::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

#[derive(Debug)]
struct FixtureAudit {
    snapshot: CrossAppSnapshot,
    entries: Vec<AuditEntry>,
}

fn fixture_audit(run_id: &str) -> Option<FixtureAudit> {
    let state = fixture(run_id)?;
    let state = lock_unpoisoned(&state);
    Some(FixtureAudit {
        snapshot: state.snapshot.clone(),
        entries: state.audit.clone(),
    })
}

fn final_state_exact(snapshot: &CrossAppSnapshot) -> bool {
    snapshot.crm_target.team == TARGET_TEAM
        && snapshot.crm_target.version == 8
        && snapshot.billing_target.team == TARGET_TEAM
        && snapshot.billing_target.version == 13
        && snapshot.support_target.status == "closed"
        && snapshot.support_target.version == 4
}

fn sentinels_unchanged(snapshot: &CrossAppSnapshot) -> bool {
    snapshot.crm_sentinel == initial_snapshot().crm_sentinel
        && snapshot.billing_sentinel == initial_snapshot().billing_sentinel
        && snapshot.support_sentinel == initial_snapshot().support_sentinel
}

fn accepted_mutations(entries: &[AuditEntry]) -> Vec<&AuditEntry> {
    entries
        .iter()
        .filter(|entry| entry.mutated && entry.outcome == "accepted")
        .collect()
}

fn conflict_recovered(entries: &[AuditEntry]) -> bool {
    let conflicts = entries
        .iter()
        .filter(|entry| {
            entry.service == "billing"
                && entry.operation == "transfer"
                && entry.outcome == "version_conflict"
        })
        .count();
    let conflict_position = entries.iter().position(|entry| {
        entry.service == "billing"
            && entry.operation == "transfer"
            && entry.outcome == "version_conflict"
    });
    let billing_success = entries.iter().position(|entry| {
        entry.service == "billing" && entry.operation == "transfer" && entry.outcome == "accepted"
    });
    conflicts == 1
        && conflict_position
            .zip(billing_success)
            .is_some_and(|(conflict, success)| conflict < success)
}

fn exact_mutation_sequence(entries: &[AuditEntry]) -> bool {
    let attempts = entries
        .iter()
        .filter(|entry| matches!(entry.operation.as_str(), "transfer" | "close"))
        .collect::<Vec<_>>();
    if attempts.len() != 4
        || attempts.last().is_none_or(|entry| {
            entry.service != "support"
                || entry.entity_id != TARGET_TICKET
                || entry.outcome != "accepted"
        })
        || attempts.iter().any(|entry| {
            !matches!(entry.outcome.as_str(), "accepted" | "version_conflict")
                || (entry.operation == "transfer" && entry.entity_id != TARGET_ACCOUNT)
                || (entry.operation == "close" && entry.entity_id != TARGET_TICKET)
        })
    {
        return false;
    }
    let mutations = accepted_mutations(entries);
    if mutations.len() != 3 {
        return false;
    }
    let services = mutations
        .iter()
        .map(|entry| entry.service.as_str())
        .collect::<Vec<_>>();
    let support_last = services.last() == Some(&"support");
    let account_services = services[..2]
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    support_last && account_services == ["billing", "crm"].into_iter().collect()
}

fn transcript_is_scoped(transcript: &Value, run_id: &str) -> bool {
    let ids = FunctionIds::new(run_id);
    let allowed = ids.all();
    common::function_calls(transcript).iter().all(|call| {
        allowed.contains(&call.function_id.as_str())
            || call.function_id.starts_with("engine::functions::")
    })
}

fn receipts_reported(response: &str, run_id: &str) -> bool {
    ["crm", "billing", "support"]
        .iter()
        .all(|service| response.contains(&receipt(run_id, service)))
        && response.matches("XAPP-").count() == 3
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let Some(audit) = fixture_audit(run_id) else {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "run_scoped_fixture_available",
                "cross-app fixture is unavailable",
            ));
        };
        let final_exact = final_state_exact(&audit.snapshot);
        let conflict_ok = conflict_recovered(&audit.entries);
        let sequence_ok = exact_mutation_sequence(&audit.entries);
        let scoped = transcript_is_scoped(&observation.transcript, run_id);
        let sentinels_ok = sentinels_unchanged(&audit.snapshot);
        let receipts_ok = receipts_reported(&observation.response, run_id);
        Ok(assessment::build_evaluation([
            CONSISTENT_FINAL_STATE.full_or_zero(
                final_exact && sequence_ok,
                format!(
                    "final_exact={final_exact}, exact_three_mutations_with_support_last={sequence_ok}"
                ),
            ),
            CONFLICT_RECOVERED.full_or_zero(
                conflict_ok,
                format!("billing conflict recovery observed={conflict_ok}"),
            ),
            NO_COLLATERAL_MUTATION.full_or_zero(
                sentinels_ok && scoped && sequence_ok,
                format!(
                    "sentinels_unchanged={sentinels_ok}, scoped_calls={scoped}, exact_mutations={sequence_ok}"
                ),
            ),
            AUTHENTIC_RECEIPTS.full_or_zero(
                receipts_ok,
                "final response must contain exactly the three run-derived service receipts",
            ),
        ]))
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let audit = fixture_audit(run_id);
        let (available, state, entries) = match audit {
            Some(audit) => (
                true,
                serde_json::to_value(audit.snapshot)?,
                serde_json::to_value(audit.entries)?,
            ),
            None => (false, Value::Null, json!([])),
        };
        let final_exact = state
            .as_object()
            .and_then(|_| fixture_audit(run_id))
            .is_some_and(|audit| final_state_exact(&audit.snapshot));
        let sentinels_ok =
            fixture_audit(run_id).is_some_and(|audit| sentinels_unchanged(&audit.snapshot));
        let conflict_ok =
            fixture_audit(run_id).is_some_and(|audit| conflict_recovered(&audit.entries));
        let sequence_ok =
            fixture_audit(run_id).is_some_and(|audit| exact_mutation_sequence(&audit.entries));
        let receipts_ok = receipts_reported(&observation.response, run_id);
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "cross_app_audit".to_string(),
            content: json!({
                "fixture_available": available,
                "state": state,
                "audit": entries,
                "receipts": {
                    "crm": receipt(run_id, "crm"),
                    "billing": receipt(run_id, "billing"),
                    "support": receipt(run_id, "support"),
                },
                "response": observation.response,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "consistent_final_state".to_string(),
                    passed: final_exact && sequence_ok,
                    reason: format!("final_exact={final_exact}, mutation_sequence={sequence_ok}"),
                },
                CapturedInvariant {
                    id: "conflict_recovered".to_string(),
                    passed: conflict_ok,
                    reason: "exactly one billing conflict precedes its accepted retry".to_string(),
                },
                CapturedInvariant {
                    id: "no_collateral_mutation".to_string(),
                    passed: sentinels_ok && sequence_ok,
                    reason: format!("sentinels_unchanged={sentinels_ok}"),
                },
                CapturedInvariant {
                    id: "authentic_receipts".to_string(),
                    passed: receipts_ok,
                    reason: "response carries the three fixture-issued receipts".to_string(),
                },
            ],
            provenance: FunctionIds::new(run_id)
                .all()
                .into_iter()
                .map(|source_id| ProvenanceEvidence {
                    kind: "function".to_string(),
                    source_id: source_id.to_string(),
                    relation: "captured_run_scoped_service_audit".to_string(),
                })
                .collect(),
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "cross_app_audit".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["fixture_available", "state", "audit", "receipts", "response"],
                "properties": {
                    "fixture_available": { "type": "boolean" },
                    "state": { "type": ["object", "null"] },
                    "audit": { "type": "array" },
                    "receipts": { "type": "object" },
                    "response": { "type": "string" }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 65_536,
        }],
        invariants: ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().to_string(),
                description: assessment.description().to_string(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        release_fixture(run_id);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer(account_id: &str, team: &str, version: u64, key: &str) -> TransferAccountRequest {
        TransferAccountRequest {
            account_id: account_id.to_string(),
            new_team: team.to_string(),
            expected_version: version,
            idempotency_key: key.to_string(),
        }
    }

    #[test]
    fn canonical_workflow_recovers_conflict_and_closes_last() {
        let mut state = CrossAppState::new("run-a");
        assert_eq!(state.get_account("crm", TARGET_ACCOUNT).version, Some(7));
        assert_eq!(
            state.get_account("billing", TARGET_ACCOUNT).version,
            Some(11)
        );
        assert_eq!(state.get_ticket(TARGET_TICKET).version, Some(3));
        assert_eq!(
            state
                .transfer("crm", transfer(TARGET_ACCOUNT, TARGET_TEAM, 7, "crm-1"))
                .status,
            "accepted"
        );
        let conflict = state.transfer(
            "billing",
            transfer(TARGET_ACCOUNT, TARGET_TEAM, 11, "billing-1"),
        );
        assert_eq!(conflict.status, "version_conflict");
        assert_eq!(conflict.version, Some(12));
        assert_eq!(
            state.get_account("billing", TARGET_ACCOUNT).version,
            Some(12)
        );
        assert_eq!(
            state
                .transfer(
                    "billing",
                    transfer(TARGET_ACCOUNT, TARGET_TEAM, 12, "billing-1"),
                )
                .status,
            "accepted"
        );
        assert_eq!(
            state
                .close_ticket(CloseTicketRequest {
                    ticket_id: TARGET_TICKET.to_string(),
                    expected_version: 3,
                    idempotency_key: "support-1".to_string(),
                })
                .status,
            "accepted"
        );
        assert!(final_state_exact(&state.snapshot));
        assert!(sentinels_unchanged(&state.snapshot));
        assert!(conflict_recovered(&state.audit));
        assert!(exact_mutation_sequence(&state.audit));
    }

    #[test]
    fn support_cannot_close_before_both_services_converge() {
        let mut state = CrossAppState::new("run-a");
        let response = state.close_ticket(CloseTicketRequest {
            ticket_id: TARGET_TICKET.to_string(),
            expected_version: 3,
            idempotency_key: "premature".to_string(),
        });
        assert_eq!(response.status, "dependencies_not_converged");
        assert_eq!(state.snapshot.support_target.status, "open");
        assert!(accepted_mutations(&state.audit).is_empty());
    }

    #[test]
    fn successful_replay_is_idempotent() {
        let mut state = CrossAppState::new("run-a");
        let request = transfer(TARGET_ACCOUNT, TARGET_TEAM, 7, "crm-1");
        let first = state.transfer("crm", request.clone());
        let replay = state.transfer("crm", request);
        assert_eq!(first.receipt, replay.receipt);
        assert_eq!(state.snapshot.crm_target.version, 8);
        assert_eq!(accepted_mutations(&state.audit).len(), 1);
        assert_eq!(state.audit.last().unwrap().outcome, "idempotent_replay");
    }

    #[test]
    fn sentinel_mutation_is_rejected_and_detectable() {
        let mut state = CrossAppState::new("run-a");
        let before = state.snapshot.clone();
        let response = state.transfer("crm", transfer(SENTINEL_ACCOUNT, TARGET_TEAM, 19, "bad"));
        assert_eq!(response.status, "rejected");
        assert_eq!(state.snapshot, before);
        assert!(sentinels_unchanged(&state.snapshot));
        assert!(!exact_mutation_sequence(&state.audit));
    }

    #[test]
    fn names_and_receipts_are_stable_and_run_scoped() {
        let ids_a = FunctionIds::new("attempt-a");
        let ids_b = FunctionIds::new("attempt-b");
        assert_ne!(ids_a.crm_get, ids_b.crm_get);
        assert_eq!(receipt("attempt-a", "crm"), receipt("attempt-a", "crm"));
        assert_ne!(receipt("attempt-a", "crm"), receipt("attempt-b", "crm"));
        assert_ne!(receipt("attempt-a", "crm"), receipt("attempt-a", "billing"));
    }

    #[test]
    fn cleanup_of_an_absent_fixture_is_an_idempotent_noop() {
        release_fixture("never-installed-cross-app-fixture");
        release_fixture("never-installed-cross-app-fixture");
        assert!(fixture("never-installed-cross-app-fixture").is_none());
    }

    #[test]
    fn materialized_case_is_canonical_and_valid() {
        let first = materialize("attempt-a", 41).unwrap();
        let retry = materialize("attempt-b", 41).unwrap();
        first.validate().unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(first.case.seed, CANONICAL_SEED);
        assert_ne!(first.spec.prompt, retry.spec.prompt);
        assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
        assert!(first.case.deliverable_contract.capture_before_cleanup);
    }
}
