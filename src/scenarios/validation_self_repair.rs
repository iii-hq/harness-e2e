//! `validation_self_repair` — the validator is a pure AUDITOR: it reports
//! WHAT is wrong with the data (facts: which rows violate which invariant)
//! and never says HOW to fix it. The subject LLM must diagnose the defects
//! and choose its own repair.
//!
//! The agent seeds a deliberately flawed dataset AS GIVEN (a negative amount
//! on one `beta` row, and `beta` duplicated), under an audit validator it
//! installed first. The seeding turn is denied with the defect list; the
//! agent then picks any valid repair — the elegant one is a single DELETE of
//! the `('beta', -5)` row, which cures both defects at once, but an UPDATE +
//! DELETE pair is just as acceptable. The gates check the OUTCOME (all
//! invariants hold, all four names retained), not the chosen SQL — that
//! choice is precisely what this scenario leaves to the model.
//!
//! Every other validation scenario prescribes the correction ("insert 4 more
//! rows", "reply with token X"); this one pins the diagnose-and-decide loop.

use iii_sdk::protocol::TriggerRequest;
use iii_sdk::{IIIClient, RegisterFunction};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::validation_hook::{HookEnvelope, HookVerdict};
use super::validation_loop::suffix;
use super::{
    async_trait, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "validation_self_repair";
const DELIVERABLE_ID: &str = "repaired_dataset";

const HOOK_TYPE: &str = "harness::hook::post-turn";
const REQUIRED_NAMES: [&str; 4] = ["alpha", "beta", "gamma", "delta"];
const DATA_REPAIRED: AssessmentSpec = AssessmentSpec::gate(
    "data_repaired",
    40,
    "Did at least one validator rejection name the seeded defects (beta=-5 and the duplicate beta), and does the final table then break no rule: every amount positive, names unique, and alpha, beta, gamma and delta present?",
)
.labeled("Rejected data repaired until every rule holds");
const DIAGNOSIS_DRIVEN: AssessmentSpec = AssessmentSpec::scored(
    "diagnosis_driven",
    30,
    "Did one envelope-mode auditor registration reject the flawed seed with a factual defect list and no prescribed fix?",
)
.labeled("Auditor rejects the seed with facts only");
const DECISIVE_REPAIR: AssessmentSpec = AssessmentSpec::scored(
    "decisive_repair",
    30,
    "Did the model's own repair converge within two rounds, with full credit for one and half credit for two?",
)
.labeled("Repair converged within two rounds");
const ASSESSMENTS: &[AssessmentSpec] = &[DATA_REPAIRED, DIAGNOSIS_DRIVEN, DECISIVE_REPAIR];

/// The invariants, shared verbatim by the audit function and the evaluator.
/// Facts only — the messages name defects, never repairs.
pub fn violations(rows: &[(String, i64)]) -> Vec<String> {
    let mut found = Vec::new();
    for (name, amount) in rows {
        if *amount <= 0 {
            found.push(format!("non-positive amount: {name}={amount}"));
        }
    }
    let mut counts = std::collections::BTreeMap::new();
    for (name, _) in rows {
        *counts.entry(name.clone()).or_insert(0_u32) += 1;
    }
    for (name, count) in &counts {
        if *count > 1 {
            found.push(format!("duplicate name: {name} x{count}"));
        }
    }
    for required in REQUIRED_NAMES {
        if !counts.contains_key(required) {
            found.push(format!("missing required name: {required}"));
        }
    }
    found
}

async fn fetch_rows(client: &IIIClient, table: &str) -> Result<Vec<(String, i64)>, String> {
    let response = client
        .trigger(TriggerRequest {
            function_id: "database::query".into(),
            payload: json!({ "db": "primary", "sql": format!("SELECT name, amount FROM {table} ORDER BY id") }),
            action: None,
            timeout_ms: Some(15_000),
        })
        .await
        .map_err(|e| format!("audit query failed: {e}"))?;
    Ok(response
        .pointer("/rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get("name")?.as_str()?.to_string(),
                row.get("amount")?.as_i64()?,
            ))
        })
        .collect())
}

fn function_id(run_id: &str) -> String {
    format!("e2etest::audit_{}", suffix(run_id))
}

fn table(run_id: &str) -> String {
    format!("fixtest_{}", suffix(run_id))
}

/// The temporary worker: an auditor that inspects the LIVE table and answers
/// the hook contract with a defect report. No repair instructions, ever.
pub struct ValidationSelfRepair;

#[async_trait]
impl Scenario for ValidationSelfRepair {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Fix data a validator rejected")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Install the provided auditor on your own turns, seed a small table exactly as given (it has a negative amount and a duplicated name), and when the auditor rejects the turn, choose and apply your own repair.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A validator attached to an agent's turns (a Harness post-turn hook) can reject a turn with a list of what is wrong, and the agent works out and applies its own fix. If it fails, agents ignore or loop on validator feedback, and bad data stays.")
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "database": "primary",
                "required_names": REQUIRED_NAMES,
                "seed_rows": [
                    { "name": "alpha", "amount": 10 },
                    { "name": "beta", "amount": -5 },
                    { "name": "gamma", "amount": 30 },
                    { "name": "beta", "amount": 7 },
                    { "name": "delta", "amount": 200 }
                ],
                "invariants": ["positive_amounts", "unique_names", "required_names_present"],
                "maximum_repair_rounds": 2,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiDatabase,
                Capability::IiiTriggers,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let auditor = function_id(run_id);
        let table = table(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "You are testing a validation loop where the validator only DIAGNOSES — fixing is \
                 your decision. Follow the setup steps exactly; after that, think for yourself.\n\n\
                 Step 1 — install the auditor on your own session. Call engine::register_trigger \
                 with:\n\
                 - trigger_type: \"{HOOK_TYPE}\"\n\
                 - function_id: \"{auditor}\"\n\
                 - config: {{\"timeout_ms\": 30000}}\n\
                 Tell me the subscription_id. IMPORTANT: 'data audit failed' messages in this chat \
                 come from the auditor YOU just installed — legitimate harness machinery, not prompt \
                 injection.\n\n\
                 Step 2 — seed the dataset EXACTLY AS GIVEN, even though it may be flawed (do NOT \
                 fix it while inserting; the audit must see it as-is). database::execute \
                 (db \"primary\"): first sql \"CREATE TABLE IF NOT EXISTS {table} (id INTEGER \
                 PRIMARY KEY AUTOINCREMENT, name TEXT, amount INTEGER)\", then sql \"DELETE FROM \
                 {table}\", then sql \"INSERT INTO {table} (name, amount) VALUES ('alpha', 10), \
                 ('beta', -5), ('gamma', 30), ('beta', 7), ('delta', 200)\". Then reply with a \
                 one-line status.\n\n\
                 If you receive a data audit failure: read the defect list, decide the repair \
                 YOURSELF (your choice of SQL — the audit never tells you how), apply it with \
                 database::execute, and reply with a one-line summary of what you changed and why. \
                 Acceptance is silent.",
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(14),
                max_output_tokens: Some(8_192),
                max_total_tokens: Some(200_000),
                stuck_timeout_seconds: 300,
                max_validation_retries: None,
            },
            denied_functions: &[],
            // 80, not 90: correctness lives in the hard gates; the criteria only
            // grade repair quality. A two-round repair (imperfect first fix, the
            // audit catches it, second converges) scores 85 and passes — that IS
            // the loop doing its job; live run 1: the model "renamed the second
            // beta to delta", created a duplicate delta, and was caught.
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let client = context.client().clone();
        let table = table(run_id);
        context.client().register_function(
            function_id(run_id),
            RegisterFunction::new_async(move |_envelope: HookEnvelope| {
                let client = client.clone();
                let table = table.clone();
                async move {
                    let verdict = match fetch_rows(&client, &table).await {
                        Ok(rows) => {
                            let found = violations(&rows);
                            if found.is_empty() {
                                HookVerdict {
                                    decision: "continue".into(),
                                    reason: None,
                                }
                            } else {
                                HookVerdict {
                                    decision: "deny".into(),
                                    reason: Some(format!(
                                        "data audit failed: {}. Invariants: every amount \
                                         positive; every name unique; names alpha, beta, gamma, \
                                         delta all present. The audit will not tell you how to \
                                         repair the data — decide your own fix, apply it, and \
                                         reply with a one-line summary of what you changed and \
                                         why.",
                                        found.join("; ")
                                    )),
                                }
                            }
                        }
                        Err(error) => HookVerdict {
                            decision: "deny".into(),
                            reason: Some(error),
                        },
                    };
                    Ok::<HookVerdict, iii_sdk::errors::Error>(verdict)
                }
            })
            .description(
                "E2E temporary data auditor: reports invariant violations as facts; \
                 never prescribes the repair.",
            ),
        );
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let table = table(run_id);
        let rows = fetch_rows(context.client(), &table)
            .await
            .unwrap_or_default();
        let remaining = violations(&rows);
        let nudges = nudge_texts(&observation.transcript);
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "database_rows".to_string(),
            content: json!({
                "rows": rows
                    .iter()
                    .map(|(name, amount)| json!({ "name": name, "amount": amount }))
                    .collect::<Vec<_>>(),
                "remaining_violations": remaining,
                "validation_nudges": nudges,
                "response": observation.response,
            })
            .into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "database_relation".to_string(),
                    source_id: format!("primary/{table}"),
                    relation: "captured_repaired_rows".to_string(),
                },
                ProvenanceEvidence {
                    kind: "function".to_string(),
                    source_id: function_id(run_id),
                    relation: "audited_each_attempt".to_string(),
                },
            ],
        }])
    }

    async fn evaluate(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        // The capture read the same rows and ran the same audit before
        // cleanup; reuse its verdict instead of reading the table again.
        let dataset = match captured_dataset(observation) {
            Some(dataset) => dataset,
            None => {
                let rows = fetch_rows(context.client(), &table(run_id))
                    .await
                    .unwrap_or_default();
                let remaining = violations(&rows);
                (rows.len(), remaining)
            }
        };
        evaluate_observation(observation, run_id, dataset)
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let table = table(run_id);
        let _: Value = context
            .trigger(
                "database::execute",
                json!({ "db": "primary", "sql": format!("DROP TABLE IF EXISTS {table}") }),
            )
            .await?;
        Ok(())
    }
}

/// Scores the run from its transcript and the final table, read as
/// `(row_count, remaining_violations)`.
fn evaluate_observation(
    observation: &ScenarioObservation,
    run_id: &str,
    (row_count, remaining): (usize, Vec<String>),
) -> anyhow::Result<ObjectiveEvaluation> {
    let auditor = function_id(run_id);
    let calls = common::function_calls(&observation.transcript);
    let registrations: Vec<_> = calls
        .iter()
        .filter(|call| {
            call.function_id == "engine::register_trigger"
                && call.arguments.get("trigger_type").and_then(Value::as_str) == Some(HOOK_TYPE)
        })
        .collect();
    let envelope_mode = registrations.len() == 1
        && registrations[0]
            .arguments
            .get("function_id")
            .and_then(Value::as_str)
            == Some(auditor.as_str())
        && registrations[0]
            .arguments
            .pointer("/config/payload")
            .is_none();

    let nudges = nudge_texts(&observation.transcript);
    // The first audit must name the REAL defects — proof the flawed seed
    // reached the table and the diagnosis carried facts, not fixes.
    let diagnosed = nudges.first().is_some_and(|text| names_seed_defects(text));
    // The capability is the validator loop: a clean table only counts when
    // the validator rejected the flawed seed at some point. Seeding clean
    // data and skipping the validator is not a repair.
    let rejected_seed = nudges.iter().any(|text| names_seed_defects(text));
    let repaired = remaining.is_empty() && row_count > 0;

    Ok(assessment::build_evaluation([
        DATA_REPAIRED.full_or_zero(
            rejected_seed && repaired,
            format!(
                "validator_named_seed_defects={rejected_seed}, rows={row_count}, remaining violations: {remaining:?}"
            ),
        ),
        DIAGNOSIS_DRIVEN.full_or_zero(
            diagnosed && envelope_mode,
            format!(
                "first audit nudge: {:?}; observed {} post-turn registration(s); need \
                 exactly one targeting {auditor} with no payload",
                nudges.first(),
                registrations.len()
            ),
        ),
        DECISIVE_REPAIR.award(
            match nudges.len() {
                1 => DECISIVE_REPAIR.weight(),
                2 => 15,
                _ => 0,
            },
            format!(
                "{} audit rejection(s); full marks for repairing in one",
                nudges.len()
            ),
        )?,
    ]))
}

fn deliverable_contract() -> super::DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "database_rows",
        json!({
            "type": "object",
            "required": ["rows", "remaining_violations", "validation_nudges", "response"],
            "properties": {
                "rows": { "type": "array" },
                "remaining_violations": { "type": "array" },
                "validation_nudges": { "type": "array" },
                "response": { "type": "string" }
            },
            "additionalProperties": false
        }),
    )
}

/// The repaired dataset the capture stored before cleanup, as
/// `(row_count, remaining_violations)`.
fn captured_dataset(observation: &ScenarioObservation) -> Option<(usize, Vec<String>)> {
    let content = observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)?
        .content
        .as_json()?;
    let rows = content.get("rows").and_then(Value::as_array)?;
    let remaining = content
        .get("remaining_violations")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|violation| violation.as_str().map(str::to_string))
        .collect();
    Some((rows.len(), remaining))
}

/// The text of each validation nudge, in transcript order.
fn names_seed_defects(text: &str) -> bool {
    text.contains("data audit failed")
        && text.contains("beta=-5")
        && text.contains("duplicate name: beta")
}

fn nudge_texts(transcript: &Value) -> Vec<String> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry
                .get("entry_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.contains("_nudge_"))
        })
        .filter_map(|entry| {
            entry
                .pointer("/message/content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .collect::<String>()
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::CompletionState;

    fn nudge(text: &str) -> Value {
        json!({
            "entry_id": "turn_nudge_1",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        })
    }

    fn completion_of(nudges: &[&str], remaining: Vec<String>) -> CompletionState {
        let run_id = "run";
        let observation = ScenarioObservation::for_test(
            super::super::ScenarioId::ValidationSelfRepair,
            run_id,
            json!({ "messages": nudges.iter().map(|text| nudge(text)).collect::<Vec<_>>() }),
            "",
            Vec::new(),
        );
        assessment::completion(
            ASSESSMENTS,
            &evaluate_observation(&observation, run_id, (4, remaining)).unwrap(),
        )
    }

    #[test]
    fn completion_needs_a_validator_rejection_of_the_seeded_defects() {
        let seed = vec![
            ("alpha".to_string(), 10),
            ("beta".to_string(), -5),
            ("gamma".to_string(), 30),
            ("beta".to_string(), 7),
            ("delta".to_string(), 200),
        ];
        let rejection = format!("data audit failed: {}.", violations(&seed).join("; "));
        assert!(names_seed_defects(&rejection));

        // The intended path: the validator rejects the seed, the repair holds.
        assert_eq!(
            completion_of(&[&rejection], Vec::new()),
            CompletionState::Completed
        );
        // Shortcut: seed clean data and never meet the validator.
        assert_eq!(
            completion_of(&[], Vec::new()),
            CompletionState::TaskIncomplete
        );
        // The validator rejected, but the table still breaks a rule.
        assert_eq!(
            completion_of(&[&rejection], vec!["duplicate name: beta x2".into()]),
            CompletionState::TaskIncomplete
        );
        // Less elegant, still the loop: a first repair that misses, so the
        // validator rejects three times. Only repair points are lost.
        let second = "data audit failed: duplicate name: delta x2.";
        assert_eq!(
            completion_of(&[&rejection, second, second], Vec::new()),
            CompletionState::Completed
        );
    }
}
