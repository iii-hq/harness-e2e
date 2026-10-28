//! `spawn_depth_cap` — the Harness refuses a spawn past its sub-agent depth
//! budget. Every level writes its row and spawns one orchestrator child that
//! follows the same contract, until the Harness refuses: with `max_depth` M,
//! one session sits at each depth 0..=M and level M's spawn comes back as
//! `harness/spawn_depth_exceeded` with `spawn depth M+1 exceeds max_depth M`.
//!
//! The prompt names no depth, so one definition proves the default cap (3:
//! root → c1 → c2 → c3, c4 refused) and the cheaper cap the regression stack
//! stamps (1: root → c1, c2 refused). The cap is read from the Harness's own
//! configuration entry, never inferred from the refusal it is checked against.

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "spawn_depth_cap";
const DELIVERABLE_ID: &str = "depth_cap_record";
const REFUSED_KEY: &str = "refused";
const DISPATCHED: &str = "DEPTH-CAP-DISPATCHED";
const REFUSAL_CODE: &str = "harness/spawn_depth_exceeded";
/// `harness/src/config.rs` `default_max_depth`, for an entry that leaves it out.
const DEFAULT_MAX_DEPTH: u32 = 3;
/// Deepest row key cleanup removes: past every cap a stack stamps (6 for
/// depth_ladder) plus the refused level's child that must never exist.
const CLEANUP_DEPTH: u32 = 8;

const CAP_ENFORCED: AssessmentSpec = AssessmentSpec::gate(
    "cap_enforced",
    50,
    "Was the spawn at the configured max_depth refused with harness/spawn_depth_exceeded and the exact `spawn depth N+1 exceeds max_depth N` message, with no session deeper than the cap?",
)
.labeled("Spawn past the depth cap refused, nothing deeper");
const CHAIN_REACHES_CAP: AssessmentSpec = AssessmentSpec::gate(
    "chain_reaches_cap",
    30,
    "Is there exactly one session at each depth from 1 to the cap, in one parent chain from the root, each spawned without a depth refusal?",
)
.labeled("Every level up to the cap was spawned");
const REFUSAL_RECORDED: AssessmentSpec = AssessmentSpec::scored(
    "refusal_recorded",
    20,
    "Did every level up to the cap store its depth row, and the refused level its level and the refusal message verbatim under the refused key, with no spawn retried after the refusal?",
)
.labeled("Refusal stored, no retry");
const ASSESSMENTS: &[AssessmentSpec] = &[CAP_ENFORCED, CHAIN_REACHES_CAP, REFUSAL_RECORDED];

fn scope(run_id: &str) -> String {
    format!("e2e-depth-cap-{run_id}")
}

fn row_key(level: u32) -> String {
    format!("depth-{level}")
}

fn refusal_message(cap: u32) -> String {
    format!("spawn depth {} exceeds max_depth {cap}", cap + 1)
}

/// One session of the tree and the outcome of every `harness::spawn` it made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Level {
    session_id: String,
    parent_session_id: Option<String>,
    depth: u32,
    spawns: Vec<SpawnOutcome>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SpawnOutcome {
    is_error: Option<bool>,
    /// `details.error` of the result: the Harness's error code.
    error: Option<String>,
    /// `details.message` of the result.
    message: Option<String>,
}

impl SpawnOutcome {
    fn refused_for_depth(&self) -> bool {
        self.is_error == Some(true) && self.error.as_deref() == Some(REFUSAL_CODE)
    }
}

/// What capture records: the cap the Harness runs with, the tree, the row
/// each depth up to the cap wrote, and the refusal row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DepthRecord {
    max_depth: u32,
    root_session_id: String,
    levels: Vec<Level>,
    /// `depth-<d>` for d in 0..=max_depth, in order.
    #[serde(default)]
    rows: Vec<Value>,
    refused: Value,
}

struct DepthAudit {
    cap_enforced: bool,
    chain_reaches_cap: bool,
    refusal_recorded: bool,
    deepest: u32,
    chain: usize,
}

fn audit(record: &DepthRecord) -> DepthAudit {
    let cap = record.max_depth;
    let expected = refusal_message(cap);
    let deepest = record
        .levels
        .iter()
        .map(|level| level.depth)
        .max()
        .unwrap_or(0);
    let at_cap: Vec<&Level> = record.levels.iter().filter(|l| l.depth == cap).collect();
    let refused_at_cap = match at_cap.as_slice() {
        [level] => level.spawns.iter().any(|spawn| {
            spawn.refused_for_depth() && spawn.message.as_deref() == Some(expected.as_str())
        }),
        _ => false,
    };

    // One session per depth, each the child of the one above, and each
    // level below the cap spawned its child without a depth refusal.
    let mut chain = 0usize;
    let mut parent = record.root_session_id.clone();
    for depth in 1..=cap {
        let [level] = record
            .levels
            .iter()
            .filter(|level| level.depth == depth)
            .collect::<Vec<_>>()[..]
        else {
            break;
        };
        if level.parent_session_id.as_deref() != Some(parent.as_str()) {
            break;
        }
        parent = level.session_id.clone();
        chain += 1;
    }
    let early_refusal = record
        .levels
        .iter()
        .filter(|level| level.depth < cap)
        .any(|level| level.spawns.iter().any(SpawnOutcome::refused_for_depth));

    let rows_written = record.rows.len() == cap as usize + 1
        && record
            .rows
            .iter()
            .zip(0..)
            .all(|(row, depth)| *row == json!({ "level": depth }));
    let refusal_recorded = refused_at_cap
        && rows_written
        && at_cap[0].spawns.len() == 1
        && record.refused.get("level").and_then(Value::as_u64) == Some(u64::from(cap))
        && record
            .refused
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains(&expected));

    DepthAudit {
        cap_enforced: refused_at_cap && deepest <= cap,
        chain_reaches_cap: chain == cap as usize && !early_refusal,
        refusal_recorded,
        deepest,
        chain,
    }
}

fn evaluation(record: &DepthRecord) -> ObjectiveEvaluation {
    let audit = audit(record);
    let cap = record.max_depth;
    assessment::build_evaluation([
        CAP_ENFORCED.full_or_zero(
            audit.cap_enforced,
            format!(
                "max_depth={cap}, deepest_session={}, expected `{REFUSAL_CODE}: {}` at depth {cap}",
                audit.deepest,
                refusal_message(cap)
            ),
        ),
        CHAIN_REACHES_CAP.full_or_zero(
            audit.chain_reaches_cap,
            format!("chained_levels={}/{cap}", audit.chain),
        ),
        REFUSAL_RECORDED.full_or_zero(
            audit.refusal_recorded,
            format!(
                "depth_rows={}, refused_row={}",
                json!(record.rows),
                record.refused
            ),
        ),
    ])
}

/// The entry the Harness registered for itself (`metadata.ui_form`
/// "harness"); its id carries the stack's namespace prefix.
async fn harness_max_depth(context: &E2eContext) -> anyhow::Result<u32> {
    let listed = context
        .trigger_value("configuration::list", json!({}))
        .await
        .context("list configuration entries")?;
    let id = listed
        .get("configurations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.pointer("/metadata/ui_form").and_then(Value::as_str) == Some("harness"))
        .and_then(|entry| entry.get("id").and_then(Value::as_str))
        .context("no Harness configuration entry is registered")?
        .to_string();
    let entry = context
        .trigger_value("configuration::get", json!({ "id": id }))
        .await
        .with_context(|| format!("read configuration {id}"))?;
    max_depth_of(entry.get("value").unwrap_or(&Value::Null))
}

fn max_depth_of(value: &Value) -> anyhow::Result<u32> {
    match value.get("max_depth") {
        None | Some(Value::Null) => Ok(DEFAULT_MAX_DEPTH),
        Some(depth) => depth
            .as_u64()
            .and_then(|depth| u32::try_from(depth).ok())
            .filter(|depth| *depth > 0)
            .with_context(|| format!("Harness max_depth leaves no relay to run: {depth}")),
    }
}

fn spawn_outcomes(transcript: &Value) -> Vec<SpawnOutcome> {
    common::function_outcomes(transcript)
        .into_iter()
        .filter(|outcome| outcome.function_id == "harness::spawn")
        .map(|outcome| {
            let detail = |field: &str| {
                outcome
                    .details
                    .as_ref()
                    .and_then(|details| details.get(field))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            SpawnOutcome {
                is_error: outcome.is_error,
                error: detail("error"),
                message: detail("message"),
            }
        })
        .collect()
}

pub struct SpawnDepthCap;

#[async_trait]
impl Scenario for SpawnDepthCap {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Sub-agent depth cap")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Each agent spawns one orchestrator child that does the same, until the Harness refuses a spawn past its configured depth; the refused level stores the refusal.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("The Harness's sub-agent depth budget: delegation goes exactly as deep as max_depth allows and the next spawn is refused with an error the agent can act on. If it fails, a delegation chain can grow without bound and burn the budget, or stop short of the depth operators configured.")
    }

    fn case(&self, seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "refused_key": REFUSED_KEY,
                "row_key": "depth-<level>",
                "refusal_code": REFUSAL_CODE,
                "dispatched_marker": DISPATCHED,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiState,
                Capability::E2eSubagents,
            ],
            super::validation_loop::validation_contract(
                DELIVERABLE_ID,
                "state_record",
                json!({
                    "type": "object",
                    "required": ["max_depth", "root_session_id", "levels", "refused"],
                    "additionalProperties": true
                }),
            ),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let scope = scope(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                r#"Probe the sub-agent depth limit in state scope `{scope}`. You are level 0.

Every level follows this relay contract, and hands it down verbatim as its child's `task`, with
the child's level number filled in:

1. At level `d`, write exactly `{{"level": d}}` with `state::set` to scope `{scope}`, key
   `depth-<d>` (you write `depth-0`).
2. Call `harness::spawn` exactly once to start level `d`+1 with this same contract as its task,
   and set `options.orchestrator` to `true` so the child may spawn in turn.
3. If that spawn returns an error, write `{{"level": d, "error": "<the error message, verbatim>"}}`
   to key `{REFUSED_KEY}` in the same scope and stop. Do not retry the spawn or spawn anything
   else.
4. Otherwise end the turn with one short line. Do not wait for, poll or report on the child.

The relay ends on its own when a spawn is refused. When your own steps are done, reply with one
line containing `{DISPATCHED}`."#
            ),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(12),
                max_output_tokens: Some(4_096),
                max_total_tokens: Some(400_000),
                stuck_timeout_seconds: 300,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let max_depth = harness_max_depth(context).await?;
        let root = &observation.metrics.root_session_id;
        let mut levels = Vec::new();
        for session in &observation.metrics.by_session {
            let transcript = if &session.session_id == root {
                observation.transcript.clone()
            } else {
                context.transcript(&session.session_id).await?
            };
            levels.push(Level {
                session_id: session.session_id.clone(),
                parent_session_id: session.parent_session_id.clone(),
                depth: session.depth,
                spawns: spawn_outcomes(&transcript),
            });
        }
        let scope = scope(run_id);
        let mut rows = Vec::new();
        for depth in 0..=max_depth {
            rows.push(common::get_state(context, &scope, &row_key(depth)).await?);
        }
        let record = DepthRecord {
            max_depth,
            root_session_id: root.clone(),
            levels,
            rows,
            refused: common::get_state(context, &scope, REFUSED_KEY).await?,
        };
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_record".to_string(),
            content: serde_json::to_value(&record)?.into(),
            invariants: Vec::new(),
            provenance: vec![
                ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{scope}/{REFUSED_KEY}"),
                    relation: "captured_refusal_row".to_string(),
                },
                ProvenanceEvidence {
                    kind: "session".to_string(),
                    source_id: root.clone(),
                    relation: "captured_session_tree".to_string(),
                },
            ],
        }])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "metrics_complete",
                "subject metrics are incomplete",
            ));
        }
        let record = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .and_then(|deliverable| deliverable.content.as_json())
            .context("spawn_depth_cap captured no depth record")?;
        Ok(evaluation(&serde_json::from_value(record.clone())?))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        let scope = scope(run_id);
        let keys = (0..=CLEANUP_DEPTH)
            .map(row_key)
            .chain([REFUSED_KEY.to_string()]);
        for key in keys {
            let _: Value = context
                .trigger_value("state::delete", json!({ "scope": scope, "key": key }))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `deliverable` as `capture` records it, scored by `evaluate`.
    async fn evaluated(
        scenario: &dyn Scenario,
        id: crate::scenarios::ScenarioId,
        deliverable: Value,
        response: &str,
    ) -> ObjectiveEvaluation {
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let observation = ScenarioObservation::for_test(
            id,
            "control",
            json!({ "messages": [] }),
            response,
            vec![CapturedDeliverable {
                id: DELIVERABLE_ID.to_string(),
                kind: "record".to_string(),
                content: deliverable.into(),
                invariants: Vec::new(),
                provenance: Vec::new(),
            }],
        );
        scenario
            .evaluate(&context, &observation, "control")
            .await
            .unwrap()
    }

    /// The null delivery (the root replies and spawns nothing) and the
    /// reference one, on the regression stack's cap of 1.
    pub(crate) async fn controls() -> [ObjectiveEvaluation; 2] {
        let null = DepthRecord {
            max_depth: 1,
            root_session_id: "e2e_control".into(),
            levels: vec![level("e2e_control", None, 0, Vec::new())],
            rows: Vec::new(),
            refused: Value::Null,
        };
        let mut reference = reference(1);
        reference.root_session_id = "e2e_control".into();
        reference.levels[0].session_id = "e2e_control".into();
        reference.levels[1].parent_session_id = Some("e2e_control".into());
        let id = crate::scenarios::ScenarioId::SpawnDepthCap;
        [
            evaluated(
                &SpawnDepthCap,
                id,
                serde_json::to_value(null).unwrap(),
                "done",
            )
            .await,
            evaluated(
                &SpawnDepthCap,
                id,
                serde_json::to_value(reference).unwrap(),
                DISPATCHED,
            )
            .await,
        ]
    }

    fn level(id: &str, parent: Option<&str>, depth: u32, spawns: Vec<SpawnOutcome>) -> Level {
        Level {
            session_id: id.into(),
            parent_session_id: parent.map(str::to_owned),
            depth,
            spawns,
        }
    }

    fn spawned() -> SpawnOutcome {
        SpawnOutcome {
            is_error: Some(false),
            error: None,
            message: None,
        }
    }

    fn refused(cap: u32) -> SpawnOutcome {
        SpawnOutcome {
            is_error: Some(true),
            error: Some(REFUSAL_CODE.into()),
            message: Some(refusal_message(cap)),
        }
    }

    /// The tree a compliant relay leaves under `cap`: one session per depth,
    /// each spawning its child, the deepest refused once.
    fn reference(cap: u32) -> DepthRecord {
        let mut levels = vec![level("root", None, 0, vec![spawned()])];
        for depth in 1..=cap {
            let parent = if depth == 1 {
                "root".to_string()
            } else {
                format!("c{}", depth - 1)
            };
            levels.push(level(
                &format!("c{depth}"),
                Some(&parent),
                depth,
                vec![spawned()],
            ));
        }
        levels.last_mut().unwrap().spawns = vec![refused(cap)];
        DepthRecord {
            max_depth: cap,
            root_session_id: "root".into(),
            levels,
            rows: (0..=cap).map(|depth| json!({ "level": depth })).collect(),
            refused: json!({
                "level": cap,
                "error": format!("{REFUSAL_CODE}: {}", refusal_message(cap)),
            }),
        }
    }

    fn awarded(evaluation: &ObjectiveEvaluation) -> Vec<Option<u8>> {
        evaluation
            .awards
            .iter()
            .map(|award| award.awarded)
            .collect()
    }

    #[test]
    fn the_reference_relay_earns_every_point_at_the_default_and_the_cheap_cap() {
        for cap in [1, 3] {
            let evaluation = evaluation(&reference(cap));
            assert_eq!(
                awarded(&evaluation),
                [Some(50), Some(30), Some(20)],
                "cap {cap}"
            );
        }
    }

    #[test]
    fn a_null_agent_earns_nothing() {
        let record = DepthRecord {
            max_depth: 1,
            root_session_id: "root".into(),
            levels: vec![level("root", None, 0, Vec::new())],
            rows: Vec::new(),
            refused: Value::Null,
        };
        assert_eq!(awarded(&evaluation(&record)), [Some(0), Some(0), Some(0)]);
    }

    /// The regression this test exists for: the cap not enforced, so a
    /// child appears past it.
    #[test]
    fn a_session_past_the_cap_fails_the_cap_gate() {
        let mut record = reference(3);
        record.levels[3].spawns = vec![spawned()];
        record.levels.push(level("c4", Some("c3"), 4, Vec::new()));
        record.refused = Value::Null;
        let audit = audit(&record);
        assert!(!audit.cap_enforced);
        assert_eq!(audit.deepest, 4);
    }

    #[test]
    fn a_refusal_naming_another_cap_or_code_is_not_the_cap() {
        let mut off_by_one = reference(3);
        off_by_one.levels[3].spawns = vec![refused(2)];
        assert!(!audit(&off_by_one).cap_enforced);

        let mut other_code = reference(3);
        other_code.levels[3].spawns[0].error = Some("harness/spawn_fanout_exceeded".into());
        assert!(!audit(&other_code).cap_enforced);
    }

    #[test]
    fn a_refusal_before_the_cap_breaks_the_chain() {
        let mut record = reference(3);
        record.levels.truncate(3);
        record.levels[2].spawns = vec![refused(3)];
        let audit = audit(&record);
        assert!(!audit.chain_reaches_cap);
        assert!(!audit.cap_enforced);
    }

    #[test]
    fn a_mis_parented_level_breaks_the_chain() {
        let mut record = reference(3);
        record.levels[2].parent_session_id = Some("root".into());
        assert!(!audit(&record).chain_reaches_cap);
    }

    #[test]
    fn a_retried_spawn_after_the_refusal_loses_the_record_points() {
        let mut record = reference(3);
        record.levels[3].spawns.push(refused(3));
        let audit = audit(&record);
        assert!(audit.cap_enforced);
        assert!(!audit.refusal_recorded);
    }

    #[test]
    fn a_level_that_skipped_its_row_loses_the_record_points() {
        let mut record = reference(3);
        record.rows[2] = Value::Null;
        let audit = audit(&record);
        assert!(audit.cap_enforced && audit.chain_reaches_cap);
        assert!(!audit.refusal_recorded);
    }

    #[test]
    fn the_cap_comes_from_the_entry_or_the_harness_default() {
        assert_eq!(max_depth_of(&json!({"max_depth": 1})).unwrap(), 1);
        // No spawn at all is allowed: there is no relay to run.
        assert!(max_depth_of(&json!({"max_depth": 0})).is_err());
        assert_eq!(max_depth_of(&json!({})).unwrap(), DEFAULT_MAX_DEPTH);
        assert_eq!(max_depth_of(&Value::Null).unwrap(), DEFAULT_MAX_DEPTH);
        assert!(max_depth_of(&json!({"max_depth": "3"})).is_err());
    }

    #[test]
    fn spawn_outcomes_read_the_harness_error_details() {
        let transcript = json!({ "messages": [
            { "message": { "role": "assistant", "content": [{
                "type": "function_call", "id": "call-0", "function_id": "harness::spawn",
                "arguments": { "task": "go", "options": { "orchestrator": true } },
            }] } },
            { "message": {
                "role": "function_result", "function_call_id": "call-0",
                "function_id": "harness::spawn", "is_error": true,
                "details": { "error": REFUSAL_CODE, "message": refusal_message(1) },
            } },
        ] });
        assert_eq!(spawn_outcomes(&transcript), [refused(1)]);
    }

    #[test]
    fn the_prompt_names_no_depth() {
        let prompt = SpawnDepthCap.spec("run").prompt;
        assert!(prompt.contains("options.orchestrator"));
        assert!(!prompt.contains("max_depth"));
        SpawnDepthCap.spec("run").validate().unwrap();
    }
}
