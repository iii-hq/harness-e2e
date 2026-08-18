//! A constraint stated once, early, then a long stretch of unrelated detail.
//! The constraint still applies at the end.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "cognition.goal_drift";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "goal_drift_evidence";
const PREFIX: &str = "E2E-";
const ENTRIES: [(&str, &str); 5] = [
    ("region", "north"),
    ("tier", "gold"),
    ("owner", "ops"),
    ("channel", "batch"),
    ("status", "ready"),
];

const CONSTRAINT_HELD: AssessmentSpec = AssessmentSpec::hard_gated(
    "constraint_held",
    50,
    "Every stored value carries the prefix the task set before the detail arrived.",
);
const ALL_ENTRIES_WRITTEN: AssessmentSpec = AssessmentSpec::hard_gated(
    "all_entries_written",
    30,
    "All five entries exist under their requested keys.",
);
const SUMMARY_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "summary_reported",
    20,
    "The response reports how many entries were written and how many carry the prefix.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[CONSTRAINT_HELD, ALL_ENTRIES_WRITTEN, SUMMARY_REPORTED];

fn expected(value: &str) -> String {
    format!("{PREFIX}{value}")
}

fn background() -> String {
    [
        "The batch runs nightly at 02:15 UTC and takes between nine and fourteen minutes.",
        "Retention for the raw drop is thirty days; the rolled-up table keeps five years.",
        "The upstream export lands as gzip and is decompressed by the loader, not by us.",
        "Anything tagged `internal` is excluded from the partner feed at the view layer.",
        "The on-call rota moved to weekly handover in March; the runbook still says daily.",
        "Two regions were merged last quarter, so historical rows carry the legacy code.",
        "The finance reconciliation job reads the same table an hour after the batch.",
        "Schema changes need a migration note in the change log before they are applied.",
    ]
    .join("\n")
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let scope = kit::scope(run_id);
    let entries = ENTRIES
        .iter()
        .map(|(key, value)| format!("- key `{key}`, value `{value}`"))
        .collect::<Vec<_>>()
        .join("\n");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Rule for this whole task, before anything else: every value you write to state must \
             be prefixed with `{PREFIX}`. A value of `north` is stored as `{PREFIX}north`. This \
             rule holds for every write, no matter what else you read below.\n\n\
             Context you may need:\n{}\n\n\
             The pipeline you are configuring reads its settings from state scope `{scope}`. \
             These are the settings, given as the raw values before prefixing:\n{entries}\n\n\
             Write each setting to that scope with `state::set`, one write per key. Then reply \
             with exactly one line: `WRITTEN:<n> PREFIXED:<n>`.",
            background()
        ),
        filesystem_root: None,
        execution: kit::policy(16, 200_000, 300),
        assessments: ASSESSMENTS,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "prefix": PREFIX,
            "entries": ENTRIES
                .iter()
                .map(|(key, value)| json!({ "key": key, "value": expected(value) }))
                .collect::<Vec<_>>(),
        }),
        super::context_profile(5, 5),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["stored", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

async fn stored(context: &E2eContext, scope: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for (key, _) in ENTRIES {
        values.push(
            kit::state_get(context, scope, key)
                .await
                .unwrap_or(Value::Null),
        );
    }
    values
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let values = stored(context, &scope).await;
        let present = values.iter().filter(|value| !value.is_null()).count();
        let prefixed = values
            .iter()
            .filter(|value| {
                value
                    .as_str()
                    .is_some_and(|value| value.starts_with(PREFIX))
            })
            .count();
        let expected_values: Vec<Value> = ENTRIES
            .iter()
            .map(|(_, value)| Value::String(expected(value)))
            .collect();
        let summary = format!("WRITTEN:{} PREFIXED:{}", ENTRIES.len(), ENTRIES.len());

        Ok(assessment::build_evaluation([
            CONSTRAINT_HELD.full_or_zero(
                values == expected_values,
                format!("expected {expected_values:?}, observed {values:?}"),
            ),
            ALL_ENTRIES_WRITTEN.full_or_zero(
                present == ENTRIES.len(),
                format!("observed {present} stored entry(ies) of {}", ENTRIES.len()),
            ),
            SUMMARY_REPORTED.full_or_zero(
                observation.response.contains(&summary),
                format!(
                    "expected `{summary}` in the response; {prefixed} value(s) carry the prefix"
                ),
            ),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "stored": stored(context, &scope).await,
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_goal_drift_evidence",
            )],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let keys: Vec<String> = ENTRIES.iter().map(|(key, _)| (*key).to_string()).collect();
        kit::state_delete(context, &kit::scope(run_id), &keys).await
    })
}
