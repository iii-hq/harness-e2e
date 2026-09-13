//! `sequential_pipeline` — three hidden stage contracts that must be read at
//! runtime and chained strictly in order: each stage's result copies the
//! contract it read, and the next stage may only start after the previous
//! write succeeded. The evaluator checks the stored results and the exact
//! call sequence.

use std::collections::BTreeSet;

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::CompletionState;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, CapturedDeliverable, CleanupFuture, DeliverableCaptureFuture, DeliverableContract,
    EvaluationFuture, ExecutionPolicy, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "sequential_pipeline";
const DELIVERABLE_ID: &str = "pipeline_record";

const START_TOKEN: &str = "SEQ-START-7D1A";
const INGEST_TOKEN: &str = "SEQ-INGEST-92B4";
const REFINE_TOKEN: &str = "SEQ-REFINE-C31F";
const RECEIPT_TOKEN: &str = "SEQ-RECEIPT-E84D";

const STAGE_INGEST: &str = "stage_ingest";
const STAGE_REFINE: &str = "stage_refine";
const STAGE_PUBLISH: &str = "stage_publish";
const RESULT_INGEST: &str = "result_ingest";
const RESULT_REFINE: &str = "result_refine";
const PIPELINE_RECEIPT: &str = "pipeline_receipt";
const ALL_KEYS: [&str; 6] = [
    STAGE_INGEST,
    STAGE_REFINE,
    STAGE_PUBLISH,
    RESULT_INGEST,
    RESULT_REFINE,
    PIPELINE_RECEIPT,
];

const RECEIPT_DELIVERED: AssessmentSpec = AssessmentSpec::scored(
    "receipt_delivered",
    40,
    "pipeline_receipt exactly copies the publish stage name, accepted token, and receipt, and the final response contains that receipt.",
);
const EXACT_CHAIN: AssessmentSpec = AssessmentSpec::scored(
    "exact_chain",
    35,
    "All three results chain the authored tokens exactly and the calls occurred as get/set pairs per stage, in order, with exact values.",
);
const EXECUTION_DISCIPLINE: AssessmentSpec = AssessmentSpec::scored(
    "execution_discipline",
    15,
    "Exactly six non-discovery calls, all on the owned scope and known keys, with zero function-call errors.",
);
const CONCISE_REPORT: AssessmentSpec = AssessmentSpec::scored(
    "concise_report",
    10,
    "The final response is one short line containing the exact receipt and no invented token.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    RECEIPT_DELIVERED,
    EXACT_CHAIN,
    EXECUTION_DISCIPLINE,
    CONCISE_REPORT,
];

fn scope(run_id: &str) -> String {
    format!("e2e-sequential-{run_id}")
}

fn contracts() -> [(&'static str, Value); 3] {
    [
        (
            STAGE_INGEST,
            json!({ "stage": "ingest", "accepts": START_TOKEN, "issues": INGEST_TOKEN }),
        ),
        (
            STAGE_REFINE,
            json!({ "stage": "refine", "accepts": INGEST_TOKEN, "issues": REFINE_TOKEN }),
        ),
        (
            STAGE_PUBLISH,
            json!({ "stage": "publish", "accepts": REFINE_TOKEN, "receipt": RECEIPT_TOKEN }),
        ),
    ]
}

fn expected_results() -> [(&'static str, Value); 3] {
    [
        (
            RESULT_INGEST,
            json!({ "stage": "ingest", "accepted": START_TOKEN, "issued": INGEST_TOKEN }),
        ),
        (
            RESULT_REFINE,
            json!({ "stage": "refine", "accepted": INGEST_TOKEN, "issued": REFINE_TOKEN }),
        ),
        (
            PIPELINE_RECEIPT,
            json!({ "stage": "publish", "accepted": REFINE_TOKEN, "receipt": RECEIPT_TOKEN }),
        ),
    ]
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        seed,
        json!({
            "contracts": contracts().iter().map(|(key, value)| json!({ "key": key, "value": value })).collect::<Vec<_>>(),
            "expected_results": expected_results().iter().map(|(key, value)| json!({ "key": key, "value": value })).collect::<Vec<_>>(),
            "receipt": RECEIPT_TOKEN,
        }),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn deliverable_contract() -> DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "state_record",
        json!({
            "type": "object",
            "required": ["contracts", "results", "response"],
            "additionalProperties": true
        }),
    )
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let scope = scope(run_id);
        let mut contracts = serde_json::Map::new();
        for key in [STAGE_INGEST, STAGE_REFINE, STAGE_PUBLISH] {
            contracts.insert(key.to_string(), read_key(context, &scope, key).await?);
        }
        let mut results = serde_json::Map::new();
        for key in [RESULT_INGEST, RESULT_REFINE, PIPELINE_RECEIPT] {
            results.insert(key.to_string(), read_key(context, &scope, key).await?);
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_record".to_string(),
            content: json!({
                "contracts": contracts,
                "results": results,
                "response": observation.response,
            })
            .into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: scope.clone(),
                    relation: "captured_pipeline_scope".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_subject_calls".to_string(),
                },
            ],
        }])
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let scope = scope(run_id);
    ScenarioSpec {
        id: ID,
        prompt: format!(
            "Complete a strictly sequential three-stage pipeline in state scope `{scope}`. The \
             stage contracts are hidden from this session and must be read at runtime.\n\n\
             1. Read key `{STAGE_INGEST}` with `state::get`. Copy its `accepts` and `issues` \
             values into one `state::set` at key `{RESULT_INGEST}` with exact value \
             `{{\"stage\":\"ingest\",\"accepted\":\"<accepts>\",\"issued\":\"<issues>\"}}`.\n\
             2. Only after that write succeeds, read key `{STAGE_REFINE}`. Verify that its \
             `accepts` equals the `issued` value from the ingest result, then write key \
             `{RESULT_REFINE}` with exact value \
             `{{\"stage\":\"refine\",\"accepted\":\"<accepts>\",\"issued\":\"<issues>\"}}`.\n\
             3. Only after that write succeeds, read key `{STAGE_PUBLISH}`. Verify that its \
             `accepts` equals the `issued` value from the refine result, then write key \
             `{PIPELINE_RECEIPT}` with exact value \
             `{{\"stage\":\"publish\",\"accepted\":\"<accepts>\",\"receipt\":\"<receipt>\"}}`.\n\n\
             Call each stage read and result write exactly once, in this order. Make no other \
             state call and do not access another scope. Finish with one short line containing \
             the exact receipt read from the publish contract."
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 12,
            max_output_tokens: Some(8_192),
            max_total_tokens: Some(160_000),
            stuck_timeout_seconds: 300,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let scope = scope(run_id);
        for (key, value) in contracts() {
            let _: Value = context
                .trigger_value(
                    "state::set",
                    json!({ "scope": scope, "key": key, "value": value }),
                )
                .await?;
            let stored = common::state_value(
                context
                    .trigger_value("state::get", json!({ "scope": scope, "key": key }))
                    .await?,
            );
            if stored != value {
                bail!("sequential_pipeline contract {scope}/{key} was not established: {stored}");
            }
        }
        Ok(())
    })
}

async fn read_key(context: &E2eContext, scope: &str, key: &str) -> anyhow::Result<Value> {
    Ok(common::state_value(
        context
            .trigger_value("state::get", json!({ "scope": scope, "key": key }))
            .await?,
    ))
}

/// Every `SEQ-…` token in the response, trimmed of surrounding punctuation.
fn sequence_tokens(text: &str) -> BTreeSet<String> {
    text.split(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '*'
            )
    })
    .map(|token| token.trim_end_matches('.'))
    .filter(|token| token.starts_with("SEQ-"))
    .map(str::to_string)
    .collect()
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let scope = scope(run_id);
        let expected = expected_results();
        let mut results_match = [false; 3];
        for (index, (key, value)) in expected.iter().enumerate() {
            results_match[index] = read_key(context, &scope, key).await? == *value;
        }
        let receipt_stored = results_match[2];

        let calls: Vec<_> = common::function_outcomes(&observation.transcript)
            .into_iter()
            .filter(|call| !common::is_contract_discovery(&call.function_id))
            .collect();
        let expected_sequence: [(&str, &str, Option<&Value>); 6] = [
            ("state::get", STAGE_INGEST, None),
            ("state::set", RESULT_INGEST, Some(&expected[0].1)),
            ("state::get", STAGE_REFINE, None),
            ("state::set", RESULT_REFINE, Some(&expected[1].1)),
            ("state::get", STAGE_PUBLISH, None),
            ("state::set", PIPELINE_RECEIPT, Some(&expected[2].1)),
        ];
        let exact_sequence = calls.len() == expected_sequence.len()
            && calls.iter().zip(expected_sequence.iter()).all(
                |(call, (function_id, key, value))| {
                    call.function_id == *function_id
                        && call.is_error == Some(false)
                        && call.arguments.get("scope").and_then(Value::as_str)
                            == Some(scope.as_str())
                        && call.arguments.get("key").and_then(Value::as_str) == Some(*key)
                        && value.is_none_or(|value| call.arguments.get("value") == Some(value))
                },
            );
        let owned_calls = calls.iter().all(|call| {
            matches!(call.function_id.as_str(), "state::get" | "state::set")
                && call.arguments.get("scope").and_then(Value::as_str) == Some(scope.as_str())
                && call
                    .arguments
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|key| ALL_KEYS.contains(&key))
        });
        let errors = observation.metrics.totals.function_call_errors;
        let failed_calls = calls
            .iter()
            .filter(|call| call.is_error != Some(false))
            .count();

        let reply = observation.response.trim();
        let tokens = sequence_tokens(reply);
        let receipt_reported = reply.contains(RECEIPT_TOKEN);
        let concise = !reply.is_empty()
            && reply.lines().count() == 1
            && tokens.len() == 1
            && tokens.contains(RECEIPT_TOKEN);

        Ok(assessment::build_evaluation(
            if receipt_stored {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            [
                RECEIPT_DELIVERED.full_or_zero(
                    receipt_stored && receipt_reported,
                    format!("receipt_stored={receipt_stored}, receipt_reported={receipt_reported}"),
                ),
                EXACT_CHAIN.full_or_zero(
                    results_match.iter().all(|matched| *matched) && exact_sequence,
                    format!(
                        "result_ingest={}, result_refine={}, pipeline_receipt={}, exact_call_sequence={exact_sequence}",
                        results_match[0], results_match[1], results_match[2]
                    ),
                ),
                EXECUTION_DISCIPLINE.full_or_zero(
                    calls.len() == 6 && owned_calls && errors == 0 && failed_calls == 0,
                    format!(
                        "task_calls={}, owned_calls={owned_calls}, failed_calls={failed_calls}, function_call_errors={errors}",
                        calls.len()
                    ),
                ),
                CONCISE_REPORT.full_or_zero(
                    concise,
                    format!(
                        "lines={}, sequence_tokens={tokens:?}",
                        reply.lines().count()
                    ),
                ),
            ],
        ))
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let scope = scope(run_id);
        for key in ALL_KEYS {
            let _: Value = context
                .trigger_value("state::delete", json!({ "scope": scope, "key": key }))
                .await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_chain_into_the_expected_results() {
        let contracts = contracts();
        let results = expected_results();
        assert_eq!(contracts[0].1["issues"], contracts[1].1["accepts"]);
        assert_eq!(contracts[1].1["issues"], contracts[2].1["accepts"]);
        assert_eq!(results[0].1["issued"], results[1].1["accepted"]);
        assert_eq!(results[1].1["issued"], results[2].1["accepted"]);
        assert_eq!(results[2].1["receipt"], contracts[2].1["receipt"]);
        scenario("run").validate().unwrap();
        materialize("case", 11).unwrap();
    }

    #[test]
    fn sequence_tokens_ignore_punctuation_and_catch_invented_tokens() {
        assert_eq!(
            sequence_tokens("Receipt: `SEQ-RECEIPT-E84D`."),
            BTreeSet::from([RECEIPT_TOKEN.to_string()])
        );
        assert_eq!(
            sequence_tokens("Done with SEQ-RECEIPT-E84D after SEQ-REFINE-C31F.").len(),
            2
        );
    }
}
