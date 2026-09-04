//! Software engineering tasks sharing one versioned service and trusted curriculum.
//! Only a selected entry snapshot reaches the subject; the journey retains its own code.

mod assets;
mod runtime;
pub(crate) mod workflow;

use anyhow::Result;
use serde_json::json;

use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, ComplexityProfile, CriterionSpec,
    DeliverableContract, ExecutionPolicy, InvariantSpec, MaterializedScenario, ObjectiveEvaluation,
    ProvenanceEvidence, ScenarioCase, ScenarioId, ScenarioSpec,
};
use crate::report::EvaluationDimension;

pub const VERSION: u32 = 1;
pub const REPORT_ID: &str = "swe_service_report";
pub const FIXTURE_REPOSITORY: &str = "iii-hq/e2e-fixture";
pub const FIXTURE_REVISION: &str = "ab373b11ae167ef853f5b5c5184cdcd431a444ea";
pub const WORKSPACE_ROOT_ENV: &str = "HARNESS_E2E_SWE_WORKSPACE_ROOT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// Zero selects the continuous journey; 1..=8 select an isolated ticket.
    pub ticket: u8,
    pub id: &'static str,
}

impl Case {
    pub fn from_scenario(scenario: ScenarioId) -> Option<Self> {
        let ticket = match scenario {
            ScenarioId::SweConfigIsolation => 1,
            ScenarioId::SweCacheInvalidation => 2,
            ScenarioId::SweBatchReplay => 3,
            ScenarioId::SweReplayRecovery => 4,
            ScenarioId::SweContractMigration => 5,
            ScenarioId::SweTenantIsolation => 6,
            ScenarioId::SweReplayPerformance => 7,
            ScenarioId::SweReleaseHandoff => 8,
            ScenarioId::SweServiceJourney => 0,
            _ => return None,
        };
        Some(Self {
            ticket,
            id: scenario.as_str(),
        })
    }

    pub fn journey(self) -> bool {
        self.ticket == 0
    }
    pub fn mode(self) -> &'static str {
        if self.journey() {
            "journey"
        } else {
            "isolated"
        }
    }
    pub fn first_ticket(self) -> u8 {
        self.ticket.max(1)
    }
    pub fn deadline_seconds(self) -> u64 {
        if self.journey() {
            5_400
        } else {
            900
        }
    }
    pub fn generations(self) -> u32 {
        if self.journey() {
            320
        } else {
            64
        }
    }
    pub fn tokens(self) -> u64 {
        if self.journey() {
            1_500_000
        } else {
            250_000
        }
    }

    pub fn description(self) -> &'static str {
        match self.ticket {
            1 => "Diagnose configuration precedence and isolate independent CLI settings.",
            2 => "Repair stale profile caches under out-of-order change notifications.",
            3 => "Implement ordered, bounded event replay with resumable cursor semantics.",
            4 => "Recover interrupted event replay without losing or duplicating durable effects.",
            5 => "Migrate a profile API and adapt to a legacy consumer revealed by a real canary.",
            6 => "Repair cross-tenant profile access while preserving authorized operations.",
            7 => "Optimize replay work while preserving paging, order and existing behavior.",
            8 => "Address configuration-removal feedback and document the software handoff.",
            _ => "Evolve one profile service through eight SWE tickets in one continuing Harness session.",
        }
    }

    pub fn profile(self) -> ComplexityProfile {
        let adaptive = self.journey() || self.ticket == 5;
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 2,
            external_systems: 2,
            state_transitions: if self.journey() { 8 } else { 1 },
            validation_loops: 1,
            artifact_count: 1,
            ambiguity_level: 4,
            agent_owned_decomposition: true,
            material_invalidation_events: u8::from(adaptive),
            replan_loops: u8::from(adaptive),
            compensable_mutations: u8::from(adaptive),
            coherent_long_horizon: self.journey(),
            ..ComplexityProfile::default()
        }
    }
}

pub fn is_swe(scenario: ScenarioId) -> bool {
    Case::from_scenario(scenario).is_some()
}

pub fn spec(scenario: ScenarioId) -> ScenarioSpec {
    let case = Case::from_scenario(scenario).expect("SWE scenario identity");
    ScenarioSpec {
        id: case.id,
        version: VERSION,
        prompt: case.description().into(),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: case.generations(),
            max_output_tokens: Some(32_768),
            max_total_tokens: Some(case.tokens()),
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &["e2e::*", "github::*", "configuration::*", "compose::*", "router::*"],
        criteria: vec![CriterionSpec::required_deterministic(
            "swe_delivery", 100,
            "Deliver the requested ticket or all eight journey tickets while preserving accepted software and protected checks.",
            EvaluationDimension::Deliverable,
        )],
        judge_reference: None,
        setup: None,
        evaluate: |_context, _observation, _run_id| Box::pin(async {
            Ok(ObjectiveEvaluation {
                completion: crate::report::CompletionState::Undetermined,
                hard_gates: Vec::new(),
                awards: Vec::new(),
            })
        }),
        cleanup: None,
    }
}

pub fn materialize(scenario: ScenarioId) -> Result<MaterializedScenario> {
    let selection = Case::from_scenario(scenario).expect("SWE scenario identity");
    let case = ScenarioCase::new(
        selection.id,
        VERSION,
        super::stable_seed(selection.id),
        json!({
            "fixture_repository": FIXTURE_REPOSITORY,
            "fixture_revision": FIXTURE_REVISION,
            "mode": selection.mode(),
            "entry_snapshot": selection.first_ticket() - 1,
            "task": selection.ticket,
            "task_count": if selection.journey() { 8 } else { 1 },
            "deadline_seconds": selection.deadline_seconds(),
            "delegation": "optional",
            "curriculum_version": 1,
        }),
        selection.profile(),
        vec![
            "iii::functions".into(),
            "e2e::control-plane-v1".into(),
            "swe::isolated-python-workspace".into(),
        ],
        DeliverableContract {
            artifacts: vec![ArtifactExpectation {
                id: REPORT_ID.into(),
                kind: "swe-service-report".into(),
                media_type: "application/json".into(),
                schema: json!({"type":"object","required":["schema","scenario_id","fixture_revision","accepted_head","accepted_tickets","terminal_status","accepted_patch","unaccepted_patch"],"properties":{"schema":{"const":"swe-service-report/v1"},"accepted_tickets":{"type":"array"},"terminal_status":{"type":"string"}}}),
                max_size_bytes: 16 * 1024 * 1024,
            }],
            invariants: vec![InvariantSpec {
                id: "delivery_complete".into(),
                description: "All requested SWE tickets are accepted within the execution limits."
                    .into(),
            }],
            provenance_required: true,
            capture_before_cleanup: true,
        },
    )?;
    Ok(MaterializedScenario {
        spec: spec(scenario),
        case,
        capture: None,
    })
}

/// Read the trusted runtime's typed termination outcome for the generic report bridge.
pub(crate) fn execution_outcome(
    output: &std::path::Path,
    attempt: &str,
) -> Option<crate::report::RunStatus> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(runtime::final_report_path(output, attempt)).ok()?)
            .ok()?;
    match value.get("terminal_status")?.as_str()? {
        "resource_limit" => Some(crate::report::RunStatus::ResourceLimit),
        "cancelled" => Some(crate::report::RunStatus::SubjectError),
        _ => None,
    }
}

/// Attach the independent pre-cleanup record even when a workflow deadline skipped its capture node.
pub(crate) fn attach_report(
    output: &std::path::Path,
    attempt: &str,
    case: &ScenarioCase,
    report: &mut crate::report::E2eRunReport,
) -> Result<()> {
    let path = runtime::final_report_path(output, attempt);
    if !path.is_file() {
        if report.failures.is_empty() {
            anyhow::bail!("SWE execution has no final checkpoint evidence");
        }
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let terminal = value
        .get("terminal_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("infrastructure_error");
    let completed = terminal == "completed";
    if let Some(session) = value.get("session_id").and_then(serde_json::Value::as_str) {
        report.session_id = session.into();
    }
    if let Some(metrics) = value.get("metrics") {
        report.metrics = Some(serde_json::from_value(metrics.clone())?);
    }
    if let Some(transcript) = value.get("transcript") {
        report.transcript = Some(transcript.clone());
    }
    if let Some(cost) = report
        .metrics
        .as_ref()
        .and_then(|metrics| metrics.totals.cost_usd)
    {
        report.cost.subject_usd = Some(cost);
        report.cost.judge_usd = Some(0.0);
        report.cost.total_usd = Some(cost);
    }
    if matches!(
        terminal,
        "resource_limit" | "cancelled" | "infrastructure_error"
    ) {
        report.push_failure(
            if terminal == "resource_limit" {
                crate::report::RunStatus::ResourceLimit
            } else if terminal == "cancelled" {
                crate::report::RunStatus::SubjectError
            } else {
                crate::report::RunStatus::InfrastructureError
            },
            crate::report::FailurePhase::Execute,
            format!(
                "SWE execution ended as {terminal}; its last accepted Git prefix was preserved"
            ),
        );
    }
    let head = value
        .get("accepted_head")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut deliverables = crate::report::evaluate_deliverables(
        case,
        vec![CapturedDeliverable {
            id: REPORT_ID.into(),
            kind: "swe-service-report".into(),
            content: value.clone().into(),
            invariants: vec![CapturedInvariant {
                id: "delivery_complete".into(),
                passed: completed,
                reason: format!("terminal state: {terminal}"),
            }],
            provenance: vec![ProvenanceEvidence {
                kind: "git-checkpoint".into(),
                source_id: head,
                relation: "immutable accepted prefix captured before workspace cleanup".into(),
            }],
        }],
    )?;
    let reference = crate::artifact::write_json(
        output,
        path.strip_prefix(output)?,
        format!("{attempt}-swe-report"),
        "swe-service-report",
        &value,
    )?;
    for deliverable in &mut deliverables {
        deliverable.artifact = Some(reference.clone());
    }
    report.evidence.push(reference);
    report.deliverables.extend(deliverables);
    Ok(())
}
