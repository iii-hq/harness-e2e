//! `moving_target` — the L5Adaptive rung for mid-run requirement change.
//!
//! The suite's own engine connection registers two run-scoped functions
//! before the prompt is sent (the `setup` hook):
//!
//!   e2etest::brief_<suffix>  → the ORIGINAL requirement A, carrying STAMP-A
//!                              (which exists nowhere else)
//!   e2etest::submit_<suffix> → judges one manifest: an exact requirement-A
//!                              manifest is answered with a REVISION carrying
//!                              STAMP-B (which exists nowhere else); an exact
//!                              revision-B manifest is accepted with the
//!                              receipt; anything else is rejected without
//!                              leaking either stamp
//!
//! The requirement changes mid-run by design: the subject must detect the
//! revision, discard the invalidated work, and converge on the revised goal.
//! Because STAMP-B is delivered only inside the revision payload, phase A
//! cannot be skipped; because every verdict is a pure function of its input,
//! redelivered calls are idempotent. The pure function would accept a lucky
//! B-first submission, so the transcript audit — not the handler — enforces
//! the A-then-B order. This is the registry's only single-turn-loop
//! (HarnessTurn) L5 scenario.

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

pub const ID: &str = "moving_target";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "adaptation_receipt";
const REPORT_BUDGET_CHARS: usize = 300;

const ADAPTED_DELIVERABLE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "adapted_deliverable",
    40,
    "The final response reports the exact receipt, obtainable only through a revision-conforming submission.",
    EvaluationDimension::Deliverable,
);
const REVISION_HONORED: AssessmentSpec = AssessmentSpec::hard_gated(
    "revision_honored",
    30,
    "Exactly one brief-conforming submission is followed by exactly one revision-conforming submission.",
);
const BOUNDED_REWORK: AssessmentSpec = AssessmentSpec::hard_gated(
    "bounded_rework",
    15,
    "One brief call and two submit calls are the whole workload: nothing beyond function discovery, no function-call errors.",
);
const ADAPTATION_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "adaptation_report",
    15,
    "The final report is brief, carries the receipt, and mentions that the requirement was revised.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    ADAPTED_DELIVERABLE,
    REVISION_HONORED,
    BOUNDED_REWORK,
    ADAPTATION_REPORT,
];

const BRIEF_REQUIREMENT: &str =
    "Produce a manifest object exactly matching `manifest` and submit it. Requirements may be \
     revised while you work; only the latest revision counts.";

/// A brief call (the request body is ignored).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct BriefRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct BriefResponse {
    pub requirement: String,
    pub manifest: Value,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SubmitRequest {
    #[serde(default)]
    pub manifest: Value,
}

/// A submission verdict. `revision` accompanies status `revised`, `receipt`
/// accompanies status `accepted`, `guidance` accompanies status `rejected`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SubmitResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

/// Run-scoped derivation shared by both stamps and the receipt: opaque to
/// the subject, reproducible for the evaluator. Derived from the run
/// namespace (not the seed) because the `setup` hook — which hosts the
/// functions — only knows the run id.
fn derived_hex(run_id: &str, label: &str) -> u64 {
    super::stable_seed(&format!("{ID}:{run_id}:{label}"))
}

/// STAMP-A: delivered only inside the brief result.
fn stamp_a(run_id: &str) -> String {
    format!("MOVA-{:016x}", derived_hex(run_id, "stamp-a"))
}

/// STAMP-B: delivered only inside the revision payload, so phase A cannot
/// be skipped by guessing.
fn stamp_b(run_id: &str) -> String {
    format!("MOVB-{:016x}", derived_hex(run_id, "stamp-b"))
}

fn receipt(run_id: &str) -> String {
    format!("MOV-{:016x}", derived_hex(run_id, "receipt"))
}

/// The exact requirement-A manifest. Shared by the submit handler, the
/// evaluator, and the deliverable capture so the matchers cannot drift.
fn manifest_a(run_id: &str) -> Value {
    json!({
        "format": "csv",
        "columns": ["id", "qty"],
        "separator": ";",
        "stamp": stamp_a(run_id),
    })
}

/// The exact revision-B manifest — also the revision payload the submit
/// function delivers, so the expectation and the delivery cannot drift.
fn manifest_b(run_id: &str) -> Value {
    json!({
        "format": "json-lines",
        "fields": ["id", "qty", "site"],
        "stamp": stamp_b(run_id),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmissionPhase {
    A,
    B,
    Other,
}

impl SubmissionPhase {
    fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::Other => "other",
        }
    }
}

/// Classify one submitted manifest against the two expected manifests. This
/// single matcher backs the submit handler AND the transcript audit.
fn classify(run_id: &str, manifest: &Value) -> SubmissionPhase {
    if *manifest == manifest_a(run_id) {
        SubmissionPhase::A
    } else if *manifest == manifest_b(run_id) {
        SubmissionPhase::B
    } else {
        SubmissionPhase::Other
    }
}

/// Judge one submission. Pure and idempotent: history plays no part here —
/// the transcript audit, not the handler, enforces the A-then-B order. A
/// rejection never names either expected stamp.
fn submit_verdict(run_id: &str, manifest: &Value) -> SubmitResponse {
    match classify(run_id, manifest) {
        SubmissionPhase::A => SubmitResponse {
            status: "revised".to_string(),
            notice: Some("requirement changed while you worked".to_string()),
            revision: Some(manifest_b(run_id)),
            instruction: Some("resubmit one manifest conforming to the revision".to_string()),
            receipt: None,
            guidance: None,
        },
        SubmissionPhase::B => SubmitResponse {
            status: "accepted".to_string(),
            notice: None,
            revision: None,
            instruction: None,
            receipt: Some(receipt(run_id)),
            guidance: None,
        },
        SubmissionPhase::Other => SubmitResponse {
            status: "rejected".to_string(),
            notice: None,
            revision: None,
            instruction: None,
            receipt: None,
            guidance: Some(
                "manifest matches neither the brief nor a delivered revision".to_string(),
            ),
        },
    }
}

fn brief_function_id(run_id: &str) -> String {
    format!("e2etest::brief_{}", suffix(run_id))
}

fn submit_function_id(run_id: &str) -> String {
    format!("e2etest::submit_{}", suffix(run_id))
}

/// The temporary brief and submission desks: registered on the suite's own
/// engine connection. They live exactly as long as this process.
fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let brief_manifest = manifest_a(run_id);
        context.client().register_function(
            brief_function_id(run_id),
            RegisterFunction::new_async(move |_request: BriefRequest| {
                let manifest = brief_manifest.clone();
                async move {
                    Ok::<BriefResponse, iii_sdk::errors::Error>(BriefResponse {
                        requirement: BRIEF_REQUIREMENT.to_string(),
                        manifest,
                    })
                }
            })
            .description(
                "E2E temporary requirement brief: the original manifest requirement, \
                 carrying the only copy of its stamp.",
            ),
        );
        let owner = run_id.to_string();
        context.client().register_function(
            submit_function_id(run_id),
            RegisterFunction::new_async(move |request: SubmitRequest| {
                let owner = owner.clone();
                async move {
                    Ok::<SubmitResponse, iii_sdk::errors::Error>(submit_verdict(
                        &owner,
                        &request.manifest,
                    ))
                }
            })
            .description(
                "E2E temporary submission desk: answers a brief-conforming manifest with \
                 a revision, accepts a revision-conforming manifest with a receipt, and \
                 rejects everything else.",
            ),
        );
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
            "task": "manifest-with-revision",
            "phases": ["brief", "revision"],
            "max_submissions": 2,
            "token_derivation": "run-scoped",
        }),
        ComplexityProfile {
            planning_depth: 3,
            dependency_depth: 2,
            validation_loops: 2,
            ambiguity_level: 7,
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
    let brief_function = brief_function_id(run_id);
    let submit_function = submit_function_id(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Deliver one manifest to a submission service whose requirements may be revised
while you work. Two functions are already registered:
- `{brief_function}` — call it FIRST, once, with `{{}}`. It returns the current
  requirement, including the `manifest` object you must reproduce exactly.
- `{submit_function}` — call it with exactly `{{"manifest": <your manifest>}}` to submit.

Build the manifest the brief demands — copy every field exactly, including its
`stamp` — and submit it. If a submission result reports status `revised`, the
requirement has changed: discard the invalidated work, build a manifest conforming
exactly to the delivered `revision` (again every field, including its `stamp`),
and resubmit exactly once. Only the latest revision counts. A result with status
`accepted` carries a `receipt`.

Call nothing else, and never invent or guess a stamp. Finish with a one-line
report of at most 300 characters that contains the receipt string exactly as
returned and states whether the requirement was revised."#,
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 12,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(200_000),
            stuck_timeout_seconds: 300,
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

struct SubmissionAudit {
    /// The classified submissions, in transcript order.
    phases: Vec<SubmissionPhase>,
    brief_calls: usize,
    other_calls: usize,
}

impl SubmissionAudit {
    /// Exactly one A-conforming submission followed by exactly one
    /// B-conforming submission. Order matters: the pure submit function
    /// would accept a lucky B-first manifest; this audit does not.
    fn revision_honored(&self) -> bool {
        let positions = |wanted: SubmissionPhase| -> Vec<usize> {
            self.phases
                .iter()
                .enumerate()
                .filter(|(_, phase)| **phase == wanted)
                .map(|(position, _)| position)
                .collect()
        };
        let a = positions(SubmissionPhase::A);
        let b = positions(SubmissionPhase::B);
        a.len() == 1 && b.len() == 1 && a[0] < b[0]
    }

    /// Exactly two submissions and one brief call, nothing else. The
    /// evaluator layers the function-error count on top of this.
    fn bounded_rework_counts(&self) -> bool {
        self.phases.len() == 2 && self.brief_calls == 1 && self.other_calls == 0
    }
}

fn submission_audit(run_id: &str, transcript: &Value) -> SubmissionAudit {
    let brief_function = brief_function_id(run_id);
    let submit_function = submit_function_id(run_id);
    let mut phases = Vec::new();
    let mut brief_calls = 0usize;
    let mut other_calls = 0usize;
    for call in common::function_calls(transcript) {
        if call.function_id == submit_function {
            let manifest = call
                .arguments
                .get("manifest")
                .cloned()
                .unwrap_or(Value::Null);
            phases.push(classify(run_id, &manifest));
        } else if call.function_id == brief_function {
            brief_calls += 1;
        } else if !call.function_id.starts_with("engine::functions::") {
            // Function discovery is legitimate machinery, not indiscipline —
            // the same exemption every leaf-discipline audit in this registry
            // grants.
            other_calls += 1;
        }
    }
    SubmissionAudit {
        phases,
        brief_calls,
        other_calls,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let audit = submission_audit(run_id, &observation.transcript);
        let receipt = receipt(run_id);
        let receipt_reported = observation.response.contains(&receipt);
        let revision_honored = audit.revision_honored();
        let errors = observation.metrics.totals.function_call_errors;
        let bounded = audit.bounded_rework_counts() && errors == 0;
        let response_chars = observation.response.chars().count();
        let lowercase_response = observation.response.to_lowercase();
        let mentions_revision =
            lowercase_response.contains("revised") || lowercase_response.contains("revision");
        let report_ok =
            receipt_reported && mentions_revision && response_chars <= REPORT_BUDGET_CHARS;
        let phase_trace = audit
            .phases
            .iter()
            .map(|phase| phase.label())
            .collect::<Vec<_>>()
            .join(",");
        Ok(assessment::build_evaluation([
            ADAPTED_DELIVERABLE.full_or_zero(
                receipt_reported,
                format!("final response must contain the exact receipt `{receipt}`"),
            ),
            REVISION_HONORED.full_or_zero(
                revision_honored,
                format!(
                    "observed submission phases [{phase_trace}]; expected exactly one \
                     brief-conforming submission followed by exactly one \
                     revision-conforming submission"
                ),
            ),
            BOUNDED_REWORK.full_or_zero(
                bounded,
                format!(
                    "observed {} submit call(s), {} brief call(s), {} other call(s), and \
                     {errors} function-call error(s); expected 2, 1, 0, and 0",
                    audit.phases.len(),
                    audit.brief_calls,
                    audit.other_calls
                ),
            ),
            ADAPTATION_REPORT.full_or_zero(
                report_ok,
                format!(
                    "receipt_reported={receipt_reported}, mentions_revision={mentions_revision}, \
                     observed {response_chars} character(s); limit {REPORT_BUDGET_CHARS}"
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
        let audit = submission_audit(run_id, &observation.transcript);
        let receipt = receipt(run_id);
        let receipt_reported = observation.response.contains(&receipt);
        let revision_honored = audit.revision_honored();
        let submissions = audit
            .phases
            .iter()
            .map(|phase| {
                json!({
                    "phase": phase.label(),
                    "exact": *phase != SubmissionPhase::Other,
                })
            })
            .collect::<Vec<_>>();
        let provenance = if receipt_reported && revision_honored {
            vec![
                ProvenanceEvidence {
                    kind: "function".to_string(),
                    source_id: brief_function_id(run_id),
                    relation: "issued_brief".to_string(),
                },
                ProvenanceEvidence {
                    kind: "function".to_string(),
                    source_id: submit_function_id(run_id),
                    relation: "judged_submissions".to_string(),
                },
            ]
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "adaptation_receipt".to_string(),
            content: json!({
                "receipt": if receipt_reported { receipt.clone() } else { String::new() },
                "submissions": submissions,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "receipt_reported".to_string(),
                    passed: receipt_reported,
                    reason: format!("final response must contain the exact receipt `{receipt}`"),
                },
                CapturedInvariant {
                    id: "revision_honored".to_string(),
                    passed: revision_honored,
                    reason: format!(
                        "submissions audited in transcript order against both expected \
                         manifests; observed {} submission(s)",
                        audit.phases.len()
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
            kind: "adaptation_receipt".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["receipt", "submissions"],
                "properties": {
                    "receipt": { "type": "string" },
                    "submissions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["phase", "exact"],
                            "properties": {
                                "phase": { "type": "string", "enum": ["A", "B", "other"] },
                                "exact": { "type": "boolean" }
                            },
                            "additionalProperties": false
                        }
                    }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 8_192,
        }],
        invariants: vec![
            InvariantSpec {
                id: "receipt_reported".to_string(),
                description: "The final response contains the exact receipt issued for the revision-conforming submission.".to_string(),
            },
            InvariantSpec {
                id: "revision_honored".to_string(),
                description: "Exactly one brief-conforming submission was followed by exactly one revision-conforming submission.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SubmissionPhase = SubmissionPhase::A;
    const B: SubmissionPhase = SubmissionPhase::B;

    #[test]
    fn stamps_and_receipt_are_run_scoped_and_stable() {
        assert_eq!(stamp_a("attempt-a"), stamp_a("attempt-a"));
        assert_ne!(stamp_a("attempt-a"), stamp_a("attempt-b"));
        assert_eq!(stamp_b("attempt-a"), stamp_b("attempt-a"));
        assert_ne!(stamp_b("attempt-a"), stamp_b("attempt-b"));
        assert_ne!(stamp_a("attempt-a"), stamp_b("attempt-a"));
        assert_eq!(receipt("attempt-a"), receipt("attempt-a"));
        assert_ne!(receipt("attempt-a"), receipt("attempt-b"));
        assert!(receipt("attempt-a").starts_with("MOV-"));
        assert_eq!(receipt("attempt-a").len(), "MOV-".len() + 16);
    }

    #[test]
    fn submitting_the_brief_manifest_delivers_the_revision() {
        let run_id = "verdict-run";
        let verdict = submit_verdict(run_id, &manifest_a(run_id));
        assert_eq!(verdict.status, "revised");
        assert!(verdict.notice.is_some());
        assert!(verdict.instruction.is_some());
        assert!(verdict.receipt.is_none());
        let revision = verdict.revision.expect("revised verdict carries the revision");
        assert_eq!(revision, manifest_b(run_id));
        assert_eq!(revision["stamp"], json!(stamp_b(run_id)));
    }

    #[test]
    fn submitting_the_revised_manifest_is_accepted_with_the_receipt() {
        let run_id = "verdict-run";
        let verdict = submit_verdict(run_id, &manifest_b(run_id));
        assert_eq!(verdict.status, "accepted");
        assert_eq!(verdict.receipt.as_deref(), Some(receipt(run_id).as_str()));
        assert!(verdict.revision.is_none());
        assert!(verdict.guidance.is_none());
    }

    #[test]
    fn other_manifests_are_rejected_without_leaking_stamps() {
        let run_id = "verdict-run";
        let verdict = submit_verdict(run_id, &json!({ "format": "xml", "stamp": "guessed" }));
        assert_eq!(verdict.status, "rejected");
        assert!(verdict.revision.is_none());
        assert!(verdict.receipt.is_none());
        assert!(verdict.guidance.is_some());
        let serialized = serde_json::to_string(&verdict).unwrap();
        assert!(!serialized.contains(&stamp_a(run_id)));
        assert!(!serialized.contains(&stamp_b(run_id)));
        assert!(!serialized.contains(&receipt(run_id)));
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

    fn manifest_for(run_id: &str, phase: SubmissionPhase) -> Value {
        match phase {
            SubmissionPhase::A => manifest_a(run_id),
            SubmissionPhase::B => manifest_b(run_id),
            SubmissionPhase::Other => json!({ "format": "freeform" }),
        }
    }

    /// One brief call followed by the given submissions.
    fn run_calls(run_id: &str, submissions: &[SubmissionPhase]) -> Vec<(String, Value)> {
        let mut calls = vec![(brief_function_id(run_id), json!({}))];
        calls.extend(submissions.iter().map(|phase| {
            (
                submit_function_id(run_id),
                json!({ "manifest": manifest_for(run_id, *phase) }),
            )
        }));
        calls
    }

    fn audit_of(run_id: &str, submissions: &[SubmissionPhase]) -> SubmissionAudit {
        submission_audit(run_id, &transcript_of(&run_calls(run_id, submissions)))
    }

    #[test]
    fn a_lucky_b_first_submission_passes_the_pure_function_but_fails_the_order_audit() {
        let run_id = "order-run";
        // The pure function cannot know history; the transcript audit can.
        assert_eq!(submit_verdict(run_id, &manifest_b(run_id)).status, "accepted");
        assert!(!audit_of(run_id, &[B]).revision_honored());
        assert!(!audit_of(run_id, &[B, A]).revision_honored());

        let honored = audit_of(run_id, &[A, B]);
        assert!(honored.revision_honored());
        assert!(honored.bounded_rework_counts());
    }

    #[test]
    fn repeated_brief_phase_submissions_exceed_the_rework_budget() {
        let run_id = "budget-run";
        let audit = audit_of(run_id, &[A, A, B]);
        assert!(
            !audit.bounded_rework_counts(),
            "three submissions exceed the budget of two"
        );
        assert!(
            !audit.revision_honored(),
            "two brief-conforming submissions are not exactly one"
        );
    }

    #[test]
    fn the_audit_exempts_function_discovery_but_counts_other_calls() {
        let run_id = "audit-run";
        let mut calls = run_calls(run_id, &[A, B]);
        calls.insert(
            0,
            (
                "engine::functions::list".to_string(),
                json!({ "search": "e2etest" }),
            ),
        );
        let audit = submission_audit(run_id, &transcript_of(&calls));
        assert_eq!(audit.other_calls, 0, "function discovery is exempt");
        assert!(audit.bounded_rework_counts());

        calls.push(("state::set".to_string(), json!({ "key": "extra" })));
        let audit = submission_audit(run_id, &transcript_of(&calls));
        assert_eq!(audit.other_calls, 1);
        assert!(!audit.bounded_rework_counts());
    }

    #[test]
    fn materialized_case_is_l5_adaptive_and_reproducible_across_namespaces() {
        let first = materialize("attempt-a", 29).unwrap();
        let retry = materialize("attempt-b", 29).unwrap();
        first.validate().unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(
            first.case.complexity.tier,
            super::super::ComplexityTier::L5Adaptive
        );
        assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
        assert!(first.case.deliverable_contract.capture_before_cleanup);
        assert!(first.capture.is_some());
    }
}
