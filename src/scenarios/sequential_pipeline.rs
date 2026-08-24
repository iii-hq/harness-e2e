//! `sequential_pipeline` — the L1Sequential rung: strict dependent chaining.
//!
//! The suite's own engine connection registers three run-scoped stage
//! functions before the prompt is sent (the `setup` hook):
//!
//!   e2etest::seq_ingest_<suffix>  → accepts the prompt token, issues token 1
//!   e2etest::seq_refine_<suffix>  → accepts token 1, issues token 2
//!   e2etest::seq_publish_<suffix> → accepts token 2, issues the receipt
//!
//! Each stage is a pure function that advances only on the exact token the
//! previous stage issued, so the chain cannot be shortcut, reordered, or
//! parallelized: correct sequencing is the deliverable. No state, timers,
//! subagents, or validators are involved — this isolates multi-step tool
//! discipline from everything the L2+ scenarios measure.

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common::{self, ObservedFunctionInvocation};
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "sequential_pipeline";
const VERSION: u32 = 2;
const DELIVERABLE_ID: &str = "pipeline_receipt";
const STAGES: [&str; 3] = ["ingest", "refine", "publish"];
/// Token indexes 0..=2 feed the stages; index 3 is the receipt the terminal
/// stage issues.
const RECEIPT_INDEX: u8 = 3;

const RECEIPT_DELIVERED: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "receipt_delivered",
    40,
    "The final response reports the exact receipt issued by the terminal stage.",
    EvaluationDimension::Deliverable,
);
const EXACT_CHAIN: AssessmentSpec = AssessmentSpec::hard_gated(
    "exact_chain",
    35,
    "Each stage is invoked exactly once, in declared order, with the exact token the previous stage issued.",
);
const EXECUTION_DISCIPLINE: AssessmentSpec = AssessmentSpec::score_only(
    "execution_discipline",
    15,
    "The chain is the whole workload: three calls, nothing beyond function discovery, no function-call errors.",
);
const CONCISE_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "concise_report",
    10,
    "The final response is a brief single report.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    RECEIPT_DELIVERED,
    EXACT_CHAIN,
    EXECUTION_DISCIPLINE,
    CONCISE_REPORT,
];

/// A stage call (harness → stage function).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct StageRequest {
    #[serde(default)]
    pub token: String,
}

/// A stage answer. Exactly one of `token`/`receipt` is present on success;
/// neither on rejection.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StageResponse {
    pub stage: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

/// What a stage issues when it accepts its input token.
#[derive(Debug, Clone)]
enum StageIssue {
    Token(String),
    Receipt(String),
}

/// Judge one stage call. Advancing requires the exact token the previous
/// stage issued; a rejection names the stage but never leaks the expected
/// token, so the chain stays unguessable. Stateless and idempotent — a
/// redelivered call gets the same answer.
fn stage_verdict(
    stage: &'static str,
    expected_token: &str,
    presented_token: &str,
    issue: &StageIssue,
) -> StageResponse {
    if presented_token != expected_token {
        return StageResponse {
            stage: stage.to_string(),
            status: "rejected".to_string(),
            token: None,
            receipt: None,
            guidance: Some(format!(
                "stage `{stage}` only accepts the exact token issued by the previous stage; \
                 call the stages strictly in declared order"
            )),
        };
    }
    let (token, receipt) = match issue {
        StageIssue::Token(token) => (Some(token.clone()), None),
        StageIssue::Receipt(receipt) => (None, Some(receipt.clone())),
    };
    StageResponse {
        stage: stage.to_string(),
        status: "ok".to_string(),
        token,
        receipt,
        guidance: None,
    }
}

fn stage_function_id(run_id: &str, stage: &str) -> String {
    format!("e2etest::seq_{stage}_{}", suffix(run_id))
}

/// Run-scoped chain tokens: opaque to the subject, reproducible for the
/// evaluator. Derived from the run namespace (not the seed) because the
/// `setup` hook — which hosts the stage functions — only knows the run id.
fn chain_token(run_id: &str, index: u8) -> String {
    format!(
        "SEQ-{:016x}",
        super::stable_seed(&format!("{ID}:{run_id}:stage-{index}"))
    )
}

/// The temporary stages: registered on the suite's own engine connection.
/// They live exactly as long as this process.
fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        for (index, stage) in STAGES.iter().enumerate() {
            let stage_name = *stage;
            let expected = chain_token(run_id, index as u8);
            let issue = if index + 1 == STAGES.len() {
                StageIssue::Receipt(chain_token(run_id, RECEIPT_INDEX))
            } else {
                StageIssue::Token(chain_token(run_id, index as u8 + 1))
            };
            context.client().register_function(
                stage_function_id(run_id, stage),
                RegisterFunction::new_async(move |request: StageRequest| {
                    let expected = expected.clone();
                    let issue = issue.clone();
                    async move {
                        Ok::<StageResponse, iii_sdk::errors::Error>(stage_verdict(
                            stage_name,
                            &expected,
                            &request.token,
                            &issue,
                        ))
                    }
                })
                .description(format!(
                    "E2E temporary pipeline stage `{stage_name}`: advances only on the exact \
                     token issued by the previous stage."
                )),
            );
        }
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "stages": STAGES,
            "argument_shape": { "token": "string" },
            "token_derivation": "run-scoped",
            "report": "single line containing the exact receipt",
        }),
        ComplexityProfile {
            planning_depth: 1,
            dependency_depth: 3,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
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
    let ingest = stage_function_id(run_id, STAGES[0]);
    let refine = stage_function_id(run_id, STAGES[1]);
    let publish = stage_function_id(run_id, STAGES[2]);
    let initial_token = chain_token(run_id, 0);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Complete a strictly sequential three-stage pipeline. The stage functions are \
             already registered:\n\
             1. `{ingest}`\n\
             2. `{refine}`\n\
             3. `{publish}`\n\n\
             Call stage 1 with exactly {{\"token\": \"{initial_token}\"}}. Every successful \
             stage returns a JSON object with a `token` field (the final stage returns \
             `receipt` instead). Call the next stage with exactly the token the previous \
             stage returned, using the same {{\"token\": ...}} argument shape. Call each \
             stage exactly once, in order, and call nothing else. Finish with a one-line \
             report that contains the receipt string exactly as returned.",
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 8,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(122_880),
            stuck_timeout_seconds: 240,
            max_validation_retries: None,
        },
        denied_functions: &["state::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup),
        evaluate,
        cleanup: None,
    }
}

struct StageAudit {
    stage: &'static str,
    calls: usize,
    exact: bool,
}

struct ChainAudit {
    stages: Vec<StageAudit>,
    /// The chain-filtered invocation sequence, in transcript order.
    invocations: Vec<ObservedFunctionInvocation>,
    /// Exactly one call per stage, in declared order, with exact arguments.
    ordered_exact: bool,
    other_calls: usize,
}

fn chain_audit(run_id: &str, transcript: &Value) -> ChainAudit {
    let expected: Vec<(String, Value)> = STAGES
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            (
                stage_function_id(run_id, stage),
                json!({ "token": chain_token(run_id, index as u8) }),
            )
        })
        .collect();
    let all = common::function_invocations(transcript);
    let invocations: Vec<ObservedFunctionInvocation> = all
        .iter()
        .filter(|invocation| {
            expected
                .iter()
                .any(|(id, _)| *id == invocation.call.function_id)
        })
        .cloned()
        .collect();
    // Function discovery is legitimate machinery, not indiscipline — the same
    // exemption every leaf-discipline audit in this registry grants.
    let other_calls = all
        .iter()
        .filter(|invocation| {
            !expected
                .iter()
                .any(|(id, _)| *id == invocation.call.function_id)
                && !invocation
                    .call
                    .function_id
                    .starts_with("engine::functions::")
        })
        .count();
    let stages = STAGES
        .iter()
        .zip(&expected)
        .map(|(stage, (id, arguments))| {
            let matching: Vec<_> = invocations
                .iter()
                .filter(|invocation| invocation.call.function_id == *id)
                .collect();
            StageAudit {
                stage,
                calls: matching.len(),
                exact: matching.len() == 1 && matching[0].call.arguments == *arguments,
            }
        })
        .collect();
    let ordered_exact = invocations.len() == expected.len()
        && invocations
            .iter()
            .zip(&expected)
            .all(|(invocation, (id, arguments))| {
                invocation.call.function_id == *id && invocation.call.arguments == *arguments
            });
    ChainAudit {
        stages,
        invocations,
        ordered_exact,
        other_calls,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let audit = chain_audit(run_id, &observation.transcript);
        let receipt = chain_token(run_id, RECEIPT_INDEX);
        let receipt_reported = observation.response.contains(&receipt);
        let errors = observation.metrics.totals.function_call_errors;
        let chain_calls: usize = audit.stages.iter().map(|stage| stage.calls).sum();
        let minimal_path = audit.ordered_exact && audit.other_calls == 0 && errors == 0;
        let response_present = !observation.response.trim().is_empty();
        let response_chars = observation.response.chars().count();
        let concise = response_present && response_chars <= 240;
        let per_stage = audit
            .stages
            .iter()
            .map(|stage| {
                format!(
                    "{}={} call(s) exact={}",
                    stage.stage, stage.calls, stage.exact
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok(assessment::build_evaluation([
            RECEIPT_DELIVERED.full_or_zero(
                receipt_reported,
                format!("final response must contain the exact receipt `{receipt}`"),
            ),
            EXACT_CHAIN.full_or_zero(
                audit.ordered_exact,
                format!("observed {chain_calls} chain call(s): {per_stage}"),
            ),
            EXECUTION_DISCIPLINE.full_or_zero(
                minimal_path,
                format!(
                    "ordered_exact={}; observed {} non-chain call(s) and {errors} \
                     function-call error(s)",
                    audit.ordered_exact, audit.other_calls
                ),
            ),
            CONCISE_REPORT.full_or_zero(
                concise,
                format!(
                    "response_present={response_present}; observed {response_chars} character(s); limit 240"
                ),
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
        let audit = chain_audit(run_id, &observation.transcript);
        let receipt = chain_token(run_id, RECEIPT_INDEX);
        let receipt_reported = observation.response.contains(&receipt);
        let errors = observation.metrics.totals.function_call_errors;
        let stages = audit
            .stages
            .iter()
            .map(|stage| {
                json!({
                    "stage": stage.stage,
                    "calls": stage.calls,
                    "exact": stage.exact,
                })
            })
            .collect::<Vec<_>>();
        let provenance = if audit.ordered_exact && errors == 0 {
            audit
                .invocations
                .iter()
                .map(|invocation| ProvenanceEvidence {
                    kind: "function_call".to_string(),
                    source_id: invocation
                        .call_id
                        .clone()
                        .unwrap_or_else(|| invocation.call.function_id.clone()),
                    relation: "executed_stage".to_string(),
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "pipeline_receipt".to_string(),
            content: json!({
                "receipt": if receipt_reported { receipt.clone() } else { String::new() },
                "stages": stages,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "receipt_reported".to_string(),
                    passed: receipt_reported,
                    reason: format!("final response must contain the exact receipt `{receipt}`"),
                },
                CapturedInvariant {
                    id: "exact_chain".to_string(),
                    passed: audit.ordered_exact,
                    reason: format!(
                        "expected one exact call per stage in declared order; observed {} \
                         chain call(s)",
                        audit.invocations.len()
                    ),
                },
            ],
            provenance,
        }])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "pipeline_receipt".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["receipt", "stages"],
                "properties": {
                    "receipt": { "type": "string" },
                    "stages": {
                        "type": "array",
                        "minItems": 3,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "required": ["stage", "calls", "exact"],
                            "properties": {
                                "stage": { "type": "string" },
                                "calls": { "type": "integer", "minimum": 0 },
                                "exact": { "type": "boolean" }
                            },
                            "additionalProperties": false
                        }
                    }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 4_096,
        }],
        invariants: vec![
            InvariantSpec {
                id: "receipt_reported".to_string(),
                description: "The final response contains the exact receipt issued by the terminal stage.".to_string(),
            },
            InvariantSpec {
                id: "exact_chain".to_string(),
                description: "Each stage was invoked exactly once, in declared order, with the exact chained token.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_tokens_are_run_scoped_and_stable() {
        assert_eq!(chain_token("attempt-a", 1), chain_token("attempt-a", 1));
        assert_ne!(chain_token("attempt-a", 1), chain_token("attempt-a", 2));
        assert_ne!(chain_token("attempt-a", 1), chain_token("attempt-b", 1));
        assert!(chain_token("attempt-a", 0).starts_with("SEQ-"));
    }

    #[test]
    fn stage_verdict_advances_only_on_the_exact_token() {
        let issue = StageIssue::Token("SEQ-next".to_string());
        let accepted = stage_verdict("ingest", "SEQ-ok", "SEQ-ok", &issue);
        assert_eq!(accepted.status, "ok");
        assert_eq!(accepted.token.as_deref(), Some("SEQ-next"));
        assert!(accepted.receipt.is_none());
        assert!(accepted.guidance.is_none());

        let rejected = stage_verdict("ingest", "SEQ-ok", "SEQ-wrong", &issue);
        assert_eq!(rejected.status, "rejected");
        assert!(rejected.token.is_none());
        assert!(rejected.receipt.is_none());
        let guidance = rejected.guidance.expect("rejection carries guidance");
        assert!(guidance.contains("`ingest`"));
        assert!(
            !guidance.contains("SEQ-ok"),
            "guidance must not leak the expected token"
        );
    }

    #[test]
    fn terminal_stage_issues_the_receipt() {
        let issue = StageIssue::Receipt("SEQ-receipt".to_string());
        let accepted = stage_verdict("publish", "SEQ-ok", "SEQ-ok", &issue);
        assert_eq!(accepted.status, "ok");
        assert!(accepted.token.is_none());
        assert_eq!(accepted.receipt.as_deref(), Some("SEQ-receipt"));
    }

    fn transcript_of(calls: &[(String, Value)]) -> Value {
        let blocks = calls
            .iter()
            .enumerate()
            .map(|(index, (function_id, arguments))| {
                json!({
                    "type": "function_call",
                    "id": format!("call-{index}"),
                    "function_id": function_id,
                    "arguments": arguments,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "messages": [
                { "message": { "role": "assistant", "content": blocks } }
            ]
        })
    }

    fn exact_calls(run_id: &str) -> Vec<(String, Value)> {
        STAGES
            .iter()
            .enumerate()
            .map(|(index, stage)| {
                (
                    stage_function_id(run_id, stage),
                    json!({ "token": chain_token(run_id, index as u8) }),
                )
            })
            .collect()
    }

    #[test]
    fn chain_audit_accepts_the_exact_ordered_chain() {
        let run_id = "audit-run";
        let audit = chain_audit(run_id, &transcript_of(&exact_calls(run_id)));

        assert!(audit.ordered_exact);
        assert_eq!(audit.other_calls, 0);
        assert!(audit
            .stages
            .iter()
            .all(|stage| stage.calls == 1 && stage.exact));
    }

    #[test]
    fn chain_audit_rejects_reordered_or_inexact_chains() {
        let run_id = "audit-run";
        let mut reordered = exact_calls(run_id);
        reordered.swap(0, 1);
        assert!(!chain_audit(run_id, &transcript_of(&reordered)).ordered_exact);

        let mut wrong_token = exact_calls(run_id);
        wrong_token[2].1 = json!({ "token": "SEQ-guessed" });
        let audit = chain_audit(run_id, &transcript_of(&wrong_token));
        assert!(!audit.ordered_exact);
        assert!(!audit.stages[2].exact);
    }

    #[test]
    fn chain_audit_counts_non_chain_calls_but_exempts_discovery() {
        let run_id = "audit-run";
        let mut calls = exact_calls(run_id);
        calls.push(("state::set".to_string(), json!({ "key": "extra" })));
        let audit = chain_audit(run_id, &transcript_of(&calls));

        assert!(
            audit.ordered_exact,
            "trailing non-chain calls do not break the chain itself"
        );
        assert_eq!(audit.other_calls, 1);

        let mut with_discovery = exact_calls(run_id);
        with_discovery.insert(
            0,
            (
                "engine::functions::list".to_string(),
                json!({ "search": "e2etest" }),
            ),
        );
        let audit = chain_audit(run_id, &transcript_of(&with_discovery));
        assert!(audit.ordered_exact);
        assert_eq!(audit.other_calls, 0, "function discovery is exempt");
    }

    #[test]
    fn materialized_case_is_l1_sequential() {
        let materialized = materialize("attempt-a", 17).unwrap();
        assert_eq!(
            materialized.case.complexity.tier,
            super::super::ComplexityTier::L1Sequential
        );
        assert_eq!(materialized.case.deliverable_contract.artifacts.len(), 1);
    }
}
