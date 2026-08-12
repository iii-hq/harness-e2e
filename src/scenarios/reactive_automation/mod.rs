mod cleanup;
mod evaluate;
mod evidence;
mod names;
mod prompt;
mod queries;

use crate::context::E2eContext;
use serde_json::json;

use super::assessment::{self, AssessmentSpec};
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};
use names::ScenarioNames;

pub const ID: &str = "reactive_automation";
const VERSION: u32 = 5;
const DELIVERABLE_ID: &str = "reactive_run_snapshot";
pub(super) const EXPECTED_WRITERS: usize = 3;
pub(super) const ORDERS_PER_WRITER: i64 = 5;
pub(super) const EXPECTED_ORDERS: i64 = EXPECTED_WRITERS as i64 * ORDERS_PER_WRITER;

const STUCK_WATCHDOG_SECONDS: u64 = 600;
// Shared across the long-lived root and every writer/reactor/finalizer turn.
// Discovery alone can approach one million input tokens on large-context
// models before the three writers start, so retain enough room for the actual
// workload without relaxing any behavioral gate.
const SCENARIO_MAX_TOTAL_TOKENS: u64 = 2_000_000;

const PARALLEL_WRITES: AssessmentSpec = AssessmentSpec::hard_gated(
    "parallel_writes",
    25,
    "Three parallel writer sessions produce exactly five valid orders each.",
);
const REACTIVE_AGGREGATES: AssessmentSpec = AssessmentSpec::hard_gated(
    "reactive_aggregates",
    30,
    "A mechanical trigger call maintains totals that exactly match the source rows.",
);
const TRIGGER_ORCHESTRATION: AssessmentSpec = AssessmentSpec::hard_gated(
    "trigger_orchestration",
    25,
    "The aggregate call and barrier wake are armed before writers start and proven by delivery records.",
);
const FINALIZATION_CLEANUP: AssessmentSpec = AssessmentSpec::hard_gated(
    "finalization_cleanup",
    20,
    "The barrier-woken root directly spawns one finalizer, which writes a passing report before cleanup.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    PARALLEL_WRITES,
    REACTIVE_AGGREGATES,
    TRIGGER_ORCHESTRATION,
    FINALIZATION_CLEANUP,
];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "database": "primary",
            "writer_count": EXPECTED_WRITERS,
            "orders_per_writer": ORDERS_PER_WRITER,
            "expected_orders": EXPECTED_ORDERS,
            "aggregation": "per_writer_count_and_sum",
        }),
        ComplexityProfile {
            planning_depth: 3,
            dependency_depth: 3,
            parallel_branches: EXPECTED_WRITERS as u8,
            external_systems: 2,
            state_transitions: 6,
            wake_cycles: 2,
            artifact_count: 1,
            coordination_edges: 4,
            ambiguity_level: 4,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::database".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
            "e2e::subagents".to_string(),
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
    let names = ScenarioNames::new(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt::build(&names, STUCK_WATCHDOG_SECONDS),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 64,
            max_output_tokens: None,
            max_total_tokens: SCENARIO_MAX_TOTAL_TOKENS,
            stuck_timeout_seconds: STUCK_WATCHDOG_SECONDS,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: None,
        evaluate,
        cleanup: Some(cleanup::run),
    }
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let names = ScenarioNames::new(run_id);
        if !context.function_exists("database::query").await? {
            return Ok(evaluate::missing_database());
        }
        let evidence = queries::collect(context, observation, &names).await?;
        Ok(evaluate::score(&evidence, &names))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let names = ScenarioNames::new(run_id);
        let evidence = if context.function_exists("database::query").await? {
            queries::collect(context, observation, &names).await?
        } else {
            evidence::Evidence::default()
        };
        let objective = evaluate::score(&evidence, &names);
        let invariants = objective
            .hard_gates
            .into_iter()
            .map(|gate| CapturedInvariant {
                id: gate.id,
                passed: gate.passed,
                reason: gate.reason,
            })
            .collect();
        let content = serde_json::to_value(&evidence)?;
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_run_snapshot".to_string(),
            content,
            invariants,
            provenance: vec![
                ProvenanceEvidence {
                    kind: "database_relation".to_string(),
                    source_id: format!("primary/{}", names.report),
                    relation: "captured_final_report".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session_tree".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_parallel_topology".to_string(),
                },
            ],
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_run_snapshot".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": [
                    "existing_tables", "writer_spawns", "watch", "order_summary",
                    "writers", "direct_totals", "stored_totals", "reports",
                    "aggregate_deliveries", "completion_wake_delivered",
                    "report_wake_delivered", "finalizer_spawn_count",
                    "finalizer_in_tree", "active_run_triggers"
                ],
                "additionalProperties": true
            }),
            max_size_bytes: 131_072,
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
