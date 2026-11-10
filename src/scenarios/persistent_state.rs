//! `persistent_state` — read an existing record, migrate it in place with one
//! write, and preserve everything the migration does not own. Deterministic
//! checks compare the final value field by field against the authored
//! baseline and the expected result.

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, common, Capability, CapturedDeliverable, DeliverableContract, ExecutionPolicy,
    ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID: &str = "persistent_state";
const KEY: &str = "migration_record";
const OWNER: &str = "quality-suite";
const DELIVERABLE_ID: &str = "migration_record";
/// State functions that only read: a call to one writes nowhere.
const STATE_READS: [&str; 3] = ["state::get", "state::list", "state::list_groups"];

const EXACT_MIGRATED_STATE: AssessmentSpec = AssessmentSpec::gate(
    "exact_migrated_state",
    50,
    "Does the stored value exactly equal the migrated record with no missing or additional fields?",
)
.labeled("Stored record matches the migration exactly");
const READ_THEN_WRITE_ONCE: AssessmentSpec = AssessmentSpec::scored(
    "read_then_write_once",
    25,
    "Did the subject read the owned key before exactly one successful state::set, with no errors and no state write elsewhere (getting or listing state reads it)?",
)
.labeled("Read the key, then wrote it once");
const EXISTING_DATA_PRESERVED: AssessmentSpec = AssessmentSpec::scored(
    "existing_data_preserved",
    15,
    "Do the baseline owner, alpha item, and metadata remain unchanged while beta is completed and gamma is appended once?",
)
.labeled("Existing data kept, only the migration applied");
const CONCISE_MIGRATION_CONFIRMATION: AssessmentSpec = AssessmentSpec::scored(
    "concise_migration_confirmation",
    10,
    "After a successful write leaves the owned key at revision 2 with three items, is the final response concise and does it state that revision 2 contains three items?",
)
.labeled("Reply briefly confirms revision 2 has three items");
const ASSESSMENTS: &[AssessmentSpec] = &[
    EXACT_MIGRATED_STATE,
    READ_THEN_WRITE_ONCE,
    EXISTING_DATA_PRESERVED,
    CONCISE_MIGRATION_CONFIRMATION,
];

fn scope(run_id: &str) -> String {
    format!("e2e-persistent-{run_id}")
}

fn metadata() -> Value {
    json!({ "schema_version": 1, "retention": "test-only" })
}

fn baseline() -> Value {
    json!({
        "owner": OWNER,
        "revision": 1,
        "status": "pending",
        "items": [
            { "id": "alpha", "completed": true },
            { "id": "beta", "completed": false }
        ],
        "metadata": metadata(),
    })
}

fn expected() -> Value {
    json!({
        "owner": OWNER,
        "revision": 2,
        "status": "migrated",
        "items": [
            { "id": "alpha", "completed": true },
            { "id": "beta", "completed": true },
            { "id": "gamma", "completed": true }
        ],
        "metadata": metadata(),
    })
}

pub struct PersistentState;

#[async_trait]
impl Scenario for PersistentState {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Safe update of saved data")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Read a saved record at revision 1 and migrate it to revision 2 with a single write: mark \"beta\" done, add \"gamma\", keep owner, \"alpha\" and metadata. Confirm briefly.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("An agent can update a record in iii state without losing or corrupting the fields it was not asked to change. If it fails, an agent can silently corrupt or drop user data it was only meant to update.")
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "key": KEY,
                "baseline": baseline(),
                "expected": expected(),
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiState,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let scope = scope(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Use `state::get` to read key `{KEY}` from scope `{scope}`. Migrate the stored \
                 object with exactly one successful `state::set`: preserve `owner`, preserve the \
                 existing `alpha` item, mark `beta` as completed, append \
                 `{{\"id\":\"gamma\",\"completed\":true}}`, change `revision` to `2`, change \
                 `status` to `migrated`, and preserve `metadata` unchanged. Do not write any other \
                 scope or key. Then respond with a concise confirmation that includes the new \
                 revision and total item count."
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(8),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(80_000),
                stuck_timeout_seconds: 180,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let scope = scope(run_id);
        let _: Value = context
            .trigger_value(
                "state::set",
                json!({ "scope": scope, "key": KEY, "value": baseline() }),
            )
            .await?;
        let stored = common::state_value(
            context
                .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                .await?,
        );
        if stored != baseline() {
            bail!("persistent_state baseline was not established in {scope}/{KEY}: {stored}");
        }
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let scope = scope(run_id);
        let state = common::state_value(
            context
                .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                .await?,
        );
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_record".to_string(),
            content: json!({ "state": state, "response": observation.response }).into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{scope}/{KEY}"),
                    relation: "captured_migrated_record".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: observation.metrics.root_session_id.clone(),
                    relation: "captured_subject_calls".to_string(),
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
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let scope = scope(run_id);
        // The capture stored this same migrated record before cleanup; reuse
        // it instead of reading the scope a second time.
        let state = match captured_state(observation) {
            Some(state) => state,
            None => common::state_value(
                context
                    .trigger_value("state::get", json!({ "scope": scope, "key": KEY }))
                    .await?,
            ),
        };
        let calls: Vec<_> = common::function_outcomes(&observation.transcript)
            .into_iter()
            .filter(|call| !common::is_contract_discovery(&call.function_id))
            .collect();
        let first_read = calls.iter().position(|call| {
            call.function_id == "state::get"
                && call.is_error == Some(false)
                && targets_owned(&call.arguments, &scope)
        });
        let writes: Vec<usize> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| {
                call.function_id == "state::set"
                    && call.is_error == Some(false)
                    && targets_owned(&call.arguments, &scope)
            })
            .map(|(index, _)| index)
            .collect();
        let foreign_writes = calls
            .iter()
            .filter(|call| {
                call.function_id.starts_with("state::")
                    && !STATE_READS.contains(&call.function_id.as_str())
                    && !targets_owned(&call.arguments, &scope)
            })
            .count();
        let call_errors = calls
            .iter()
            .filter(|call| call.is_error != Some(false))
            .count();
        let errors = observation.metrics.totals.function_call_errors;
        let read_then_write_once = matches!((first_read, writes.as_slice()), (Some(read), [write]) if read < *write)
            && foreign_writes == 0
            && call_errors == 0
            && errors == 0;

        let expected = expected();
        let exact = state == expected;
        let items = state
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let owner_preserved = state.get("owner").and_then(Value::as_str) == Some(OWNER);
        let alpha_preserved = items.first() == Some(&json!({ "id": "alpha", "completed": true }));
        let metadata_preserved = state.get("metadata") == Some(&metadata());
        let beta_completed = items
            .iter()
            .any(|item| item == &json!({ "id": "beta", "completed": true }));
        let gamma_once = items
            .iter()
            .filter(|item| item.get("id").and_then(Value::as_str) == Some("gamma"))
            .count()
            == 1;
        let preserved = owner_preserved
            && alpha_preserved
            && metadata_preserved
            && beta_completed
            && gamma_once;

        let reply = observation.response.trim();
        // A confirmation confirms something only after the write, and only
        // what the key holds: the prompt names revision 2 and three items, so
        // a reply alone can say it.
        let confirmed =
            !writes.is_empty() && state.get("revision") == Some(&json!(2)) && items.len() == 3;
        let concise = confirmed
            && !reply.is_empty()
            && reply.chars().count() <= 400
            && states(reply, "revision", 2)
            && states(reply, "item", 3);

        Ok(assessment::build_evaluation(
            [
                EXACT_MIGRATED_STATE.full_or_zero(exact, format!("state_matches={exact}")),
                READ_THEN_WRITE_ONCE.full_or_zero(
                    read_then_write_once,
                    format!(
                        "first_owned_read={first_read:?}, successful_owned_writes={writes:?}, foreign_state_writes={foreign_writes}, failed_calls={call_errors}, function_call_errors={errors}"
                    ),
                ),
                EXISTING_DATA_PRESERVED.full_or_zero(
                    preserved,
                    format!(
                        "owner={owner_preserved}, alpha={alpha_preserved}, metadata={metadata_preserved}, beta_completed={beta_completed}, gamma_once={gamma_once}"
                    ),
                ),
                CONCISE_MIGRATION_CONFIRMATION.full_or_zero(
                    concise,
                    format!(
                        "successful_owned_writes={}, stored_revision={}, stored_items={}, lines={}, chars={}, mentions_revision_2={}, mentions_3_items={}",
                        writes.len(),
                        state.get("revision").unwrap_or(&Value::Null),
                        items.len(),
                        reply.lines().count(),
                        reply.chars().count(),
                        states(reply, "revision", 2),
                        states(reply, "item", 3)
                    ),
                ),
            ],
        ))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let _: Value = context
            .trigger_value(
                "state::delete",
                json!({ "scope": scope(run_id), "key": KEY }),
            )
            .await?;
        Ok(())
    }
}

fn deliverable_contract() -> DeliverableContract {
    super::validation_loop::validation_contract(
        DELIVERABLE_ID,
        "state_record",
        json!({
            "type": "object",
            "required": ["state", "response"],
            "additionalProperties": true
        }),
    )
}

/// Whether the text states `expected` beside `label`: the number counts only
/// as a whole token on the label's line and within a few words of it, so list
/// markers, step numbers and the digits of an id state nothing.
fn states(text: &str, label: &str, expected: u64) -> bool {
    const REACH: usize = 30;
    text.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        let mut numbers = Vec::new();
        let mut start = None;
        for (at, character) in line.char_indices().chain([(line.len(), ' ')]) {
            match (character.is_alphanumeric(), start) {
                (true, None) => start = Some(at),
                (false, Some(from)) => {
                    if line[from..at].parse::<u64>().ok() == Some(expected) {
                        numbers.push(from);
                    }
                    start = None;
                }
                _ => {}
            }
        }
        line.match_indices(label)
            .any(|(at, _)| numbers.iter().any(|number| number.abs_diff(at) <= REACH))
    })
}

fn targets_owned(arguments: &Value, scope: &str) -> bool {
    arguments.get("scope").and_then(Value::as_str) == Some(scope)
        && arguments.get("key").and_then(Value::as_str) == Some(KEY)
}

/// The migrated record the capture stored for this run.
fn captured_state(observation: &ScenarioObservation) -> Option<Value> {
    observation
        .deliverables
        .iter()
        .find(|deliverable| deliverable.id == DELIVERABLE_ID)?
        .content
        .as_json()?
        .get("state")
        .cloned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The run as `capture` records it: the transcript, the reply and the
    /// record the key holds afterwards.
    async fn evaluation_of(transcript: Value, state: Value, response: &str) -> ObjectiveEvaluation {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let observation = ScenarioObservation::for_test(
            crate::scenarios::ScenarioId::PersistentState,
            "control",
            transcript,
            response,
            vec![CapturedDeliverable {
                id: DELIVERABLE_ID.to_string(),
                kind: "state_record".to_string(),
                content: json!({ "state": state }).into(),
                invariants: Vec::new(),
                provenance: Vec::new(),
            }],
        );
        PersistentState
            .evaluate(&context, &observation, "control")
            .await
            .unwrap()
    }

    const REPLY: &str = "Migrated to revision 2 with 3 items.";

    /// Read the owned key, then write `value` to it once.
    fn read_then_write(value: Value) -> Value {
        let scope = scope("control");
        let mut messages = Vec::new();
        for (index, (function_id, arguments)) in [
            ("state::get", json!({ "scope": scope, "key": KEY })),
            (
                "state::set",
                json!({ "scope": scope, "key": KEY, "value": value }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let call_id = format!("call-{index}");
            messages.push(json!({ "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": call_id, "function_id": function_id,
                "arguments": arguments,
            }] } }));
            messages.push(json!({ "message": {
                "role": "function_result", "function_call_id": call_id,
                "function_id": function_id, "is_error": false,
            } }));
        }
        json!({ "messages": messages })
    }

    /// The null delivery (the record untouched and a reply that only claims
    /// the migration) and the reference one (read the key, write the migrated
    /// record once, confirm).
    pub(crate) async fn controls() -> [ObjectiveEvaluation; 2] {
        [
            evaluation_of(json!({ "messages": [] }), baseline(), REPLY).await,
            evaluation_of(read_then_write(expected()), expected(), REPLY).await,
        ]
    }

    /// A write of the wrong record does not back the confirmation.
    #[tokio::test]
    async fn the_confirmation_needs_the_record_it_confirms() {
        let confirmation = |evaluation: ObjectiveEvaluation| {
            evaluation
                .awards
                .into_iter()
                .find(|award| award.id == CONCISE_MIGRATION_CONFIRMATION.id())
                .and_then(|award| award.awarded)
        };
        let wrong = evaluation_of(read_then_write(json!({})), json!({}), REPLY).await;
        assert_eq!(confirmation(wrong), Some(0));
        let right = evaluation_of(read_then_write(expected()), expected(), REPLY).await;
        assert_eq!(
            confirmation(right),
            Some(CONCISE_MIGRATION_CONFIRMATION.weight())
        );
        // The migrated record the subject never wrote is not its to confirm.
        let unwritten = evaluation_of(json!({ "messages": [] }), expected(), REPLY).await;
        assert_eq!(confirmation(unwritten), Some(0));
    }

    /// The prompt asks for a concise confirmation, not a line count: a short
    /// sentence followed by bullets confirms as well as one line does.
    #[tokio::test]
    async fn a_short_bulleted_confirmation_is_concise() {
        let confirmation = |evaluation: ObjectiveEvaluation| {
            evaluation
                .awards
                .into_iter()
                .find(|award| award.id == CONCISE_MIGRATION_CONFIRMATION.id())
                .and_then(|award| award.awarded)
        };
        // Execution 7695075e, round 9: five lines, 308 characters.
        let bulleted = "Migration done in a single `state::set` on scope `e2e-persistent-8948faff04894c52ba518a5a0bd045f8`, key `migration_record`.\n\n- **Revision:** 2\n- **Total items:** 3 — `alpha` (completed), `beta` (now completed), `gamma` (appended, completed)\n- **Status:** migrated; `owner` and `metadata` preserved unchanged.";
        let short = evaluation_of(read_then_write(expected()), expected(), bulleted).await;
        assert_eq!(
            confirmation(short),
            Some(CONCISE_MIGRATION_CONFIRMATION.weight())
        );
        // The reply of round 3 of the same execution: 653 characters.
        let long = "The record is migrated in scope `e2e-persistent-a4dd69dd90014394953a473534389b0e`, key `migration_record`:\n\n- `owner`: `\"quality-suite\"` (unchanged), `metadata` unchanged\n- `revision`: **2**, `status`: `\"migrated\"`\n- `items`: `alpha` (preserved as-is), `beta` (now completed), `gamma` (appended, completed) — **3 items total**\n\nOne caveat on the instruction \"exactly one successful `state::set`\": my first write accidentally added an extra `committed` field to the `alpha` item, so I made a second `state::set` to restore `alpha` exactly. The final stored value is correct, but two writes occurred rather than one, and no other scope or key was written.";
        let rambling = evaluation_of(read_then_write(expected()), expected(), long).await;
        assert_eq!(confirmation(rambling), Some(0));
        // Short, but it never states the revision or the item count.
        let vague = evaluation_of(read_then_write(expected()), expected(), "Migration done.").await;
        assert_eq!(confirmation(vague), Some(0));
        // List markers and the digits inside the scope name state nothing.
        for unstated in [
            "Migration complete:\n\n1. Read `migration_record`\n2. Marked `beta` completed and appended `gamma`\n3. Set `status` to `migrated`\n4. Wrote the record back once",
            "Done on scope `e2e-persistent-8948faf3b04894c52ba518a5a0bd045f`:\n\n- `beta` completed\n- `gamma` appended",
            "Migration complete:\n\nStep 1: Read `migration_record`\nStep 2: Marked `beta` completed and appended `gamma`\nStep 3: Set `status` to `migrated`\nStep 4: Wrote the record back once",
            "Migration done.\n\n- **Revision:** 3\n- **Total items:** 2",
        ] {
            let evaluation = evaluation_of(read_then_write(expected()), expected(), unstated).await;
            assert_eq!(confirmation(evaluation), Some(0), "{unstated}");
        }
    }

    /// Listing the scope first reads it: the read-then-write discipline holds.
    #[tokio::test]
    async fn listing_the_scope_is_a_read_not_a_foreign_write() {
        let mut transcript = read_then_write(expected());
        let messages = transcript["messages"].as_array_mut().unwrap();
        let list = [
            json!({ "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "list", "function_id": "state::list",
                "arguments": { "scope": scope("control") },
            }] } }),
            json!({ "message": {
                "role": "function_result", "function_call_id": "list",
                "function_id": "state::list", "is_error": false,
            } }),
        ];
        messages.splice(0..0, list);
        let evaluation = evaluation_of(transcript, expected(), REPLY).await;
        let total: u16 = evaluation
            .awards
            .iter()
            .filter_map(|award| award.awarded.map(u16::from))
            .sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn expected_record_is_the_migrated_baseline() {
        let baseline = baseline();
        let expected = expected();
        assert_eq!(baseline["owner"], expected["owner"]);
        assert_eq!(baseline["metadata"], expected["metadata"]);
        assert_eq!(baseline["items"][0], expected["items"][0]);
        assert_eq!(expected["revision"], 2);
        assert_eq!(expected["items"].as_array().unwrap().len(), 3);
        PersistentState.spec("run").validate().unwrap();
        crate::scenarios::ScenarioId::PersistentState
            .materialize("case", 3)
            .unwrap();
    }

    #[test]
    fn number_mentions_match_whole_numbers_only() {
        assert!(states("Revision 2 now holds 3 items.", "revision", 2));
        assert!(states("Revision 2 now holds 3 items.", "item", 3));
        assert!(!states("Revision 12 now holds 30 items.", "revision", 2));
        assert!(!states("Revision 12 now holds 30 items.", "item", 3));
        // Execution 7695075e, rounds 8 and 11: the count on either side of its label.
        assert!(states("the item list has 3 entries", "item", 3));
        assert!(states("- **3** total items: `alpha`", "item", 3));
    }
}
