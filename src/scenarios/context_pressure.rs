//! `context_pressure` — maximum long-horizon context-survival case.
//!
//! The retained faithful-recall task uses 48 noise segments of about 8 KiB
//! each. Earlier lower loads were removed to keep one comparable cohort.
//! This is the only scenario that exercises the stack's context manager:
//! the subject must ingest far more text than it can afford to keep
//! verbatim, and the gates check what survives.
//!
//! The suite's own engine connection registers two run-scoped functions
//! before the prompt is sent (the `setup` hook):
//!
//!   e2etest::ctx_charter_<suffix> → the planted facts, served exactly once
//!                                   at the START of the run
//!   e2etest::ctx_segment_<suffix> → deterministic noise documents, each
//!                                   hiding one needle marker
//!
//! The subject reads the charter first, then every segment strictly in
//! order, and finally reports the charter fields verbatim plus every needle
//! in ascending order. The charter values never appear in the prompt — they
//! exist only in the first function result — so reproducing them at the end
//! is direct evidence that early context survived the pressure.

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common::{self, ObservedFunctionCall};
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "context_pressure";
const VERSION: u32 = 4;
const DELIVERABLE_ID: &str = "context_report";
const SEGMENT_CHARS: usize = 8_192;

#[derive(Debug, Clone, Copy)]
struct Rung {
    segments: u32,
}

pub const CANONICAL_SEED: u64 = 3003;
const RUNG: Rung = Rung { segments: 48 };
const MAX_SEGMENTS: u32 = 48;

const CHARTER_PRESERVED: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "charter_preserved",
    40,
    "The final report reproduces every planted charter fact verbatim after the full distractor load.",
    EvaluationDimension::Deliverable,
);
const NEEDLES_RECOVERED: AssessmentSpec = AssessmentSpec::hard_gated(
    "needles_recovered",
    30,
    "Every segment needle appears in the report exactly once each, in ascending segment order.",
);
const INGESTION_DISCIPLINE: AssessmentSpec = AssessmentSpec::hard_gated(
    "ingestion_discipline",
    20,
    "The charter is read first and each segment exactly once in ascending order, with nothing beyond function discovery and no errors.",
);
const CONCISE_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "concise_report",
    10,
    "The report carries the facts and needles without echoing document text.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CHARTER_PRESERVED,
    NEEDLES_RECOVERED,
    INGESTION_DISCIPLINE,
    CONCISE_REPORT,
];

/// A charter call (the request body is ignored).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CharterRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CharterResponse {
    pub mission: String,
    pub checkpoint: String,
    pub quorum: u8,
    pub seal: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SegmentRequest {
    #[serde(default)]
    pub index: u32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SegmentResponse {
    pub index: u32,
    pub segment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

/// Run-scoped derivation shared by the charter, the needles, and the noise:
/// opaque to the subject, reproducible for the evaluator, and independent of
/// the rung so the segment function can stay rung-unaware.
fn derived_hex(run_id: &str, label: &str) -> u64 {
    super::stable_seed(&format!("{ID}:{run_id}:{label}"))
}

fn charter(run_id: &str) -> CharterResponse {
    CharterResponse {
        mission: format!("CTX-M-{:016x}", derived_hex(run_id, "mission")),
        checkpoint: format!("CTX-C-{:016x}", derived_hex(run_id, "checkpoint")),
        quorum: 3 + (derived_hex(run_id, "quorum") % 7) as u8,
        seal: format!("CTX-S-{:016x}", derived_hex(run_id, "seal")),
    }
}

fn needle(run_id: &str, index: u32) -> String {
    format!(
        "CTX-N{index}-{:012x}",
        derived_hex(run_id, &format!("needle-{index}")) & 0xffff_ffff_ffff
    )
}

/// One deterministic noise document with its needle buried at a
/// seed-selected word position. Pseudo-random hex words defeat verbatim
/// shortcuts without being compressible into anything that still contains
/// the needle by accident.
fn segment_text(run_id: &str, index: u32) -> String {
    let needle = needle(run_id, index);
    let mut state = derived_hex(run_id, &format!("segment-{index}"));
    let mut words = Vec::new();
    let mut chars = 0usize;
    while chars < SEGMENT_CHARS {
        state = state
            .wrapping_mul(0x100000001b3)
            .wrapping_add(0x9e3779b97f4a7c15);
        let word = format!("{state:016x}");
        chars += word.len() + 1;
        words.push(word);
    }
    let slot = (derived_hex(run_id, &format!("slot-{index}")) as usize) % words.len();
    words.insert(slot, needle);
    words.join(" ")
}

fn charter_function_id(run_id: &str) -> String {
    format!("e2etest::ctx_charter_{}", suffix(run_id))
}

fn segment_function_id(run_id: &str) -> String {
    format!("e2etest::ctx_segment_{}", suffix(run_id))
}

fn report_header(segments: u32) -> String {
    format!("CONTEXT-LADDER-REPORT {segments}")
}

/// The temporary document server: registered on the suite's own engine
/// connection, alive exactly as long as this process.
fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let charter_body = charter(run_id);
        context.client().register_function(
            charter_function_id(run_id),
            RegisterFunction::new_async(move |_request: CharterRequest| {
                let body = CharterResponse {
                    mission: charter_body.mission.clone(),
                    checkpoint: charter_body.checkpoint.clone(),
                    quorum: charter_body.quorum,
                    seal: charter_body.seal.clone(),
                };
                async move { Ok::<CharterResponse, iii_sdk::errors::Error>(body) }
            })
            .description(
                "E2E temporary context charter: the planted facts the final report \
                 must reproduce verbatim.",
            ),
        );
        let owner = run_id.to_string();
        context.client().register_function(
            segment_function_id(run_id),
            RegisterFunction::new_async(move |request: SegmentRequest| {
                let owner = owner.clone();
                async move {
                    let response = if request.index < MAX_SEGMENTS {
                        SegmentResponse {
                            index: request.index,
                            segment: segment_text(&owner, request.index),
                            guidance: None,
                        }
                    } else {
                        SegmentResponse {
                            index: request.index,
                            segment: String::new(),
                            guidance: Some(format!(
                                "segment index {} is outside this ladder; fetch only the \
                                 indexes your assignment names",
                                request.index
                            )),
                        }
                    };
                    Ok::<SegmentResponse, iii_sdk::errors::Error>(response)
                }
            })
            .description(
                "E2E temporary context segments: deterministic noise documents, each \
                 hiding exactly one needle marker.",
            ),
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id, RUNG)
}

pub fn materialize(namespace: &str, _seed: u64) -> anyhow::Result<MaterializedScenario> {
    let rung = RUNG;
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        json!({
            "segments": rung.segments,
            "segment_chars": SEGMENT_CHARS,
            "report_header": report_header(rung.segments),
            "token_derivation": "run-scoped",
        }),
        ComplexityProfile {
            planning_depth: 1,
            dependency_depth: 2,
            artifact_count: 1,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
        ],
        deliverable_contract(),
    )?
    // The ingest workload is charter + every segment + report.
    .with_minimum_expected_work(u64::from(rung.segments) + 2)?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace, rung),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str, rung: Rung) -> ScenarioSpec {
    let charter_function = charter_function_id(run_id);
    let segment_function = segment_function_id(run_id);
    let last = rung.segments - 1;
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            r#"Complete a context-endurance run over {segments} large documents.

Two functions are already registered:
- `{charter_function}` — call it FIRST, once, with `{{}}`. It returns the run charter:
  `mission`, `checkpoint`, `quorum`, and `seal`. You must reproduce these exactly at the end.
- `{segment_function}` — call it with `{{"index": <i>}}` for every index 0 through {last},
  strictly ascending, exactly once each. Each result is a noise document containing exactly
  one marker that starts with `CTX-N<i>-`. Note each marker; never quote any other document
  text in your replies.

Call nothing else. After the last segment, finish with a report in exactly this shape, one
item per line and nothing more:

{header}
mission=<mission from the charter>
checkpoint=<checkpoint from the charter>
quorum=<quorum from the charter>
seal=<seal from the charter>
<the marker of segment 0>
<the marker of segment 1>
... one line per segment, in ascending order."#,
            segments = rung.segments,
            header = report_header(rung.segments),
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 12 + rung.segments,
            max_output_tokens: Some(16_384),
            // Unbounded on purpose: capping total tokens would distort the
            // measurement this ladder exists for; spend shows up in the
            // Efficiency dimension instead.
            max_total_tokens: None,
            stuck_timeout_seconds: 600,
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

struct IngestionAudit {
    charter_calls: usize,
    charter_first: bool,
    segments_exact: bool,
    segment_calls: usize,
    other_calls: usize,
}

fn ingestion_audit(run_id: &str, segments: u32, calls: &[ObservedFunctionCall]) -> IngestionAudit {
    let charter_function = charter_function_id(run_id);
    let segment_function = segment_function_id(run_id);
    let charter_positions: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == charter_function)
        .map(|(position, _)| position)
        .collect();
    let segment_calls: Vec<(usize, &ObservedFunctionCall)> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.function_id == segment_function)
        .collect();
    // Function discovery is legitimate machinery, not indiscipline — the same
    // exemption every leaf-discipline audit in this registry grants.
    let other_calls = calls
        .iter()
        .filter(|call| {
            call.function_id != charter_function
                && call.function_id != segment_function
                && !call.function_id.starts_with("engine::functions::")
        })
        .count();
    let charter_first = charter_positions.len() == 1
        && segment_calls
            .iter()
            .all(|(position, _)| *position > charter_positions[0]);
    let segments_exact = segment_calls.len() == segments as usize
        && segment_calls
            .iter()
            .enumerate()
            .all(|(expected_index, (_, call))| {
                call.arguments == json!({ "index": expected_index as u32 })
            });
    IngestionAudit {
        charter_calls: charter_positions.len(),
        charter_first,
        segments_exact,
        segment_calls: segment_calls.len(),
        other_calls,
    }
}

struct ReportAudit {
    charter_preserved: bool,
    needles_ordered: bool,
    missing_needles: usize,
}

fn report_audit(response: &str, run_id: &str, segments: u32) -> ReportAudit {
    let charter = charter(run_id);
    let charter_preserved = response.trim_start().starts_with(&report_header(segments))
        && response.contains(&format!("mission={}", charter.mission))
        && response.contains(&format!("checkpoint={}", charter.checkpoint))
        && response.contains(&format!("quorum={}", charter.quorum))
        && response.contains(&format!("seal={}", charter.seal));
    let mut positions = Vec::new();
    let mut missing_needles = 0usize;
    for index in 0..segments {
        match response.find(&needle(run_id, index)) {
            Some(position) => positions.push(position),
            None => missing_needles += 1,
        }
    }
    let needles_ordered =
        missing_needles == 0 && positions.windows(2).all(|window| window[0] < window[1]);
    ReportAudit {
        charter_preserved,
        needles_ordered,
        missing_needles,
    }
}

fn case_segments(case: &ScenarioCase) -> anyhow::Result<u32> {
    case.inputs
        .get("segments")
        .and_then(Value::as_u64)
        .and_then(|segments| u32::try_from(segments).ok())
        .filter(|segments| *segments > 0)
        .ok_or_else(|| {
            anyhow::anyhow!("context_pressure case is missing a positive segments input")
        })
}

/// Report budget: the expected report plus slack, so pasting document text
/// forfeits the conciseness signal without failing a gate.
fn report_budget_chars(segments: u32) -> usize {
    1_024 + 64 * segments as usize
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { evaluate_rung(observation, run_id) })
}

fn evaluate_rung(
    observation: &ScenarioObservation,
    run_id: &str,
) -> anyhow::Result<ObjectiveEvaluation> {
    let segments = case_segments(&observation.case)?;
    let calls = common::function_calls(&observation.transcript);
    let ingestion = ingestion_audit(run_id, segments, &calls);
    let report = report_audit(&observation.response, run_id, segments);
    let errors = observation.metrics.totals.function_call_errors;
    let disciplined = ingestion.charter_first
        && ingestion.segments_exact
        && ingestion.other_calls == 0
        && errors == 0;
    let response_chars = observation.response.chars().count();
    let concise = report.charter_preserved
        && report.needles_ordered
        && response_chars <= report_budget_chars(segments);

    Ok(assessment::build_evaluation([
        CHARTER_PRESERVED.full_or_zero(
            report.charter_preserved,
            format!(
                "report must start with `{}` and reproduce mission, checkpoint, quorum, \
                 and seal verbatim",
                report_header(segments)
            ),
        ),
        NEEDLES_RECOVERED.full_or_zero(
            report.needles_ordered,
            format!(
                "missing {} of {segments} needle(s); needles must appear in ascending order",
                report.missing_needles
            ),
        ),
        INGESTION_DISCIPLINE.full_or_zero(
            disciplined,
            format!(
                "charter_calls={} (first={}), segment_calls={}/{segments} exact={}, \
                 other_calls={}, function_errors={errors}",
                ingestion.charter_calls,
                ingestion.charter_first,
                ingestion.segment_calls,
                ingestion.segments_exact,
                ingestion.other_calls
            ),
        ),
        CONCISE_REPORT.full_or_zero(
            concise,
            format!(
                "observed {response_chars} character(s); budget {}",
                report_budget_chars(segments)
            ),
        ),
    ]))
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let segments = case_segments(&observation.case)?;
        let calls = common::function_calls(&observation.transcript);
        let ingestion = ingestion_audit(run_id, segments, &calls);
        let report = report_audit(&observation.response, run_id, segments);
        let provenance = if report.charter_preserved && report.needles_ordered {
            vec![
                ProvenanceEvidence {
                    kind: "function".to_string(),
                    source_id: charter_function_id(run_id),
                    relation: "planted_charter".to_string(),
                },
                ProvenanceEvidence {
                    kind: "function".to_string(),
                    source_id: segment_function_id(run_id),
                    relation: "served_segments".to_string(),
                },
            ]
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "context_report".to_string(),
            content: json!({
                "content": observation.response,
                "segments": segments,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "charter_preserved".to_string(),
                    passed: report.charter_preserved,
                    reason: "planted charter facts compared verbatim with the report".to_string(),
                },
                CapturedInvariant {
                    id: "needles_recovered".to_string(),
                    passed: report.needles_ordered,
                    reason: format!(
                        "missing {} of {segments} needle(s), order checked",
                        report.missing_needles
                    ),
                },
                CapturedInvariant {
                    id: "ingestion_complete".to_string(),
                    passed: ingestion.charter_first && ingestion.segments_exact,
                    reason: format!(
                        "charter first={}, {}/{segments} exact segment call(s)",
                        ingestion.charter_first, ingestion.segment_calls
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
            kind: "context_report".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["content", "segments"],
                "properties": {
                    "content": { "type": "string" },
                    "segments": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 65_536,
        }],
        invariants: vec![
            InvariantSpec {
                id: "charter_preserved".to_string(),
                description:
                    "The report reproduces every planted charter fact verbatim after the full load."
                        .to_string(),
            },
            InvariantSpec {
                id: "needles_recovered".to_string(),
                description: "Every segment needle appears in the report in ascending order."
                    .to_string(),
            },
            InvariantSpec {
                id: "ingestion_complete".to_string(),
                description:
                    "The charter was read first and every segment exactly once in ascending order."
                        .to_string(),
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
    fn every_seed_request_normalizes_to_the_maximum_case() {
        assert_eq!(RUNG.segments, 48);
        assert_eq!(
            materialize("attempt", 3002).unwrap().case.seed,
            CANONICAL_SEED
        );
    }

    #[test]
    fn charter_and_needles_are_run_scoped_and_stable() {
        let first = charter("attempt-a");
        let again = charter("attempt-a");
        let other = charter("attempt-b");
        assert_eq!(first.mission, again.mission);
        assert_eq!(first.seal, again.seal);
        assert_eq!(first.quorum, again.quorum);
        assert_ne!(first.mission, other.mission);
        assert!((3..=9).contains(&first.quorum));
        assert_eq!(needle("attempt-a", 5), needle("attempt-a", 5));
        assert_ne!(needle("attempt-a", 5), needle("attempt-a", 6));
        assert_ne!(needle("attempt-a", 5), needle("attempt-b", 5));
    }

    #[test]
    fn segments_are_deterministic_noise_hiding_exactly_one_needle() {
        let text = segment_text("attempt-a", 3);
        assert_eq!(text, segment_text("attempt-a", 3));
        assert_ne!(text, segment_text("attempt-a", 4));
        assert!(text.chars().count() >= SEGMENT_CHARS);
        assert_eq!(text.matches(&needle("attempt-a", 3)).count(), 1);
        assert!(!text.contains(&needle("attempt-a", 4)));
        assert!(!text.contains(&charter("attempt-a").mission));
    }

    fn full_report(run_id: &str, segments: u32) -> String {
        let charter = charter(run_id);
        let mut lines = vec![
            report_header(segments),
            format!("mission={}", charter.mission),
            format!("checkpoint={}", charter.checkpoint),
            format!("quorum={}", charter.quorum),
            format!("seal={}", charter.seal),
        ];
        lines.extend((0..segments).map(|index| needle(run_id, index)));
        lines.join("\n")
    }

    #[test]
    fn report_audit_requires_the_charter_and_ordered_needles() {
        let run_id = "report-run";
        let complete = full_report(run_id, 4);
        let audit = report_audit(&complete, run_id, 4);
        assert!(audit.charter_preserved);
        assert!(audit.needles_ordered);

        let missing_fact = complete.replace(&format!("seal={}", charter(run_id).seal), "seal=lost");
        assert!(!report_audit(&missing_fact, run_id, 4).charter_preserved);

        let mut swapped_lines: Vec<&str> = complete.lines().collect();
        let last = swapped_lines.len() - 1;
        swapped_lines.swap(last, last - 1);
        let swapped = swapped_lines.join("\n");
        assert!(!report_audit(&swapped, run_id, 4).needles_ordered);

        let truncated = complete.replace(&needle(run_id, 2), "");
        let audit = report_audit(&truncated, run_id, 4);
        assert!(!audit.needles_ordered);
        assert_eq!(audit.missing_needles, 1);
    }

    fn call(function_id: String, arguments: Value) -> ObservedFunctionCall {
        ObservedFunctionCall {
            function_id,
            arguments,
        }
    }

    #[test]
    fn ingestion_audit_requires_charter_first_and_ascending_exact_segments() {
        let run_id = "audit-run";
        let mut calls = vec![call(charter_function_id(run_id), json!({}))];
        calls.extend(
            (0..4).map(|index| call(segment_function_id(run_id), json!({ "index": index }))),
        );
        let audit = ingestion_audit(run_id, 4, &calls);
        assert!(audit.charter_first);
        assert!(audit.segments_exact);
        assert_eq!(audit.other_calls, 0);

        let mut reordered = calls.clone();
        reordered.swap(1, 2);
        assert!(!ingestion_audit(run_id, 4, &reordered).segments_exact);

        let mut charter_late = calls.clone();
        charter_late.swap(0, 1);
        assert!(!ingestion_audit(run_id, 4, &charter_late).charter_first);

        let mut with_extra = calls.clone();
        with_extra.push(call("state::set".to_string(), json!({ "key": "extra" })));
        assert_eq!(ingestion_audit(run_id, 4, &with_extra).other_calls, 1);

        let mut with_discovery = calls;
        with_discovery.insert(
            0,
            call(
                "engine::functions::list".to_string(),
                json!({ "search": "e2etest" }),
            ),
        );
        with_discovery.push(call(
            "engine::functions::info".to_string(),
            json!({ "function_id": "x" }),
        ));
        let audit = ingestion_audit(run_id, 4, &with_discovery);
        assert_eq!(audit.other_calls, 0, "function discovery is exempt");
        assert!(audit.charter_first);
        assert!(audit.segments_exact);
    }

    #[test]
    fn retained_case_is_reproducible_and_scaled_to_the_maximum_load() {
        use super::super::ComplexityTier;

        let first = materialize("attempt-a", CANONICAL_SEED).unwrap();
        let retry = materialize("attempt-b", CANONICAL_SEED).unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.complexity.tier, ComplexityTier::L1Sequential);
        assert_eq!(
            first.case.work.minimum_expected_work,
            u64::from(RUNG.segments) + 2
        );
        assert_eq!(first.spec.execution.max_turns, 12 + RUNG.segments);
    }
}
