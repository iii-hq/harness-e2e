use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::common;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase, ScenarioObservation,
    ScenarioSpec,
};

pub const ID_1: &str = "coordination.1";
pub const ID_2: &str = "coordination.2";
pub const ID_3: &str = "coordination.3";
pub const ID_4: &str = "coordination.4";
pub const ID_5: &str = "coordination.5";

const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "coordination_result";
const COMPLETION_KEY: &str = "completion";
const FINAL_KEY: &str = "final";

const DELIVERABLE: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "deliverable_complete",
    45,
    "Every expected branch fragment and the root-owned final marker are durably captured.",
    EvaluationDimension::Deliverable,
);
const COORDINATION: AssessmentSpec = AssessmentSpec::hard_gated(
    "coordination_structure",
    40,
    "The expected direct children, wake registrations, and root-only finalization are observed.",
);
const COMPLETION: AssessmentSpec = AssessmentSpec::score_only(
    "completion_signal",
    15,
    "The root reports completion only after the coordination workflow has converged.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[DELIVERABLE, COORDINATION, COMPLETION];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    One,
    Two,
    Three,
    Four,
    Five,
}

impl Rung {
    fn number(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::One => ID_1,
            Self::Two => ID_2,
            Self::Three => ID_3,
            Self::Four => ID_4,
            Self::Five => ID_5,
        }
    }

    fn expected_children(self) -> u64 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 4,
            Self::Four | Self::Five => 5,
        }
    }

    fn expected_wakes(self) -> usize {
        match self {
            Self::One | Self::Two => 1,
            Self::Three | Self::Four => 2,
            Self::Five => 3,
        }
    }

    fn minimum_expected_work(self) -> u64 {
        match self {
            Self::One => 6,
            Self::Two => 10,
            Self::Three => 18,
            Self::Four | Self::Five => 22,
        }
    }

    fn complexity(self) -> ComplexityProfile {
        match self {
            Self::One => ComplexityProfile {
                planning_depth: 2,
                dependency_depth: 1,
                parallel_branches: 1,
                external_systems: 1,
                state_transitions: 3,
                wake_cycles: 1,
                artifact_count: 1,
                coordination_edges: 1,
                ambiguity_level: 1,
                ..ComplexityProfile::default()
            },
            Self::Two => ComplexityProfile {
                planning_depth: 2,
                dependency_depth: 1,
                parallel_branches: 2,
                external_systems: 1,
                state_transitions: 5,
                wake_cycles: 1,
                artifact_count: 1,
                coordination_edges: 2,
                ambiguity_level: 2,
                ..ComplexityProfile::default()
            },
            Self::Three => ComplexityProfile {
                planning_depth: 3,
                dependency_depth: 2,
                parallel_branches: 2,
                external_systems: 1,
                state_transitions: 9,
                wake_cycles: 2,
                artifact_count: 1,
                coordination_edges: 4,
                ambiguity_level: 3,
                ..ComplexityProfile::default()
            },
            Self::Four => ComplexityProfile {
                planning_depth: 4,
                dependency_depth: 3,
                parallel_branches: 4,
                external_systems: 1,
                state_transitions: 12,
                wake_cycles: 2,
                validation_loops: 1,
                artifact_count: 1,
                coordination_edges: 6,
                ambiguity_level: 5,
            },
            Self::Five => ComplexityProfile {
                planning_depth: 5,
                dependency_depth: 4,
                parallel_branches: 2,
                external_systems: 1,
                state_transitions: 14,
                wake_cycles: 3,
                validation_loops: 2,
                artifact_count: 1,
                coordination_edges: 8,
                ambiguity_level: 7,
            },
        }
    }
}

pub fn scenario(rung: Rung, run_id: &str) -> ScenarioSpec {
    scenario_for_case(rung, run_id, super::stable_seed(rung.id()))
}

pub fn materialize(rung: Rung, namespace: &str, seed: u64) -> Result<MaterializedScenario> {
    let expected = expected_artifact(rung, seed);
    let case = ScenarioCase::new(
        rung.id(),
        VERSION,
        seed,
        json!({
            "rung": rung.number(),
            "expected_artifact": expected,
        }),
        rung.complexity(),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::state".to_string(),
            "iii::triggers".to_string(),
            "e2e::subagents".to_string(),
        ],
        deliverable_contract(expected_artifact(rung, seed)),
    )?
    .with_minimum_expected_work(rung.minimum_expected_work())?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(rung, namespace, seed),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(rung: Rung, run_id: &str, seed: u64) -> ScenarioSpec {
    let names = Names::new(rung, run_id);
    ScenarioSpec {
        id: rung.id(),
        version: VERSION,
        prompt: prompt(rung, &names, seed),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 32,
            max_output_tokens: Some(8_192),
            max_total_tokens: 600_000,
            stuck_timeout_seconds: 180,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn prompt(rung: Rung, names: &Names, seed: u64) -> String {
    let preamble = format!(
        r#"Execute the reproducible `{id}` coordination case in isolated state scope `{scope}`.

You are the coordinator. Children are leaf workers: they may call only `state::set`, must never
spawn, read state, register triggers, or coordinate. Every wake is parent-owned and wake-only:
use `engine::register_trigger`, omit every function target, set `once: true`, and end the current
turn immediately after spawning. Never poll children or state. Use case seed `{seed}` in every
value exactly as shown. The root must write no branch fragment; it may write only `{final_key}`
after the required wake sequence converges.

When functions are exposed through the generic `agent_trigger` wrapper, always put the target
function's arguments under `payload`; never use a `parameters` field.

For every named-set barrier below, register a state trigger on scope `{scope}`, key
`{completion_key}`, with the stated label and:
`conditions: [{{ function_id: "state::barrier", config: {{ id: "<label>-set",
expect: [<writers>], key_from: "/new_value/writer", carry: "/new_value" }} }}]`.

Each normal child X must first write `{scope}` / `branch_x` as
`{{"branch":"x","status":"complete","case_seed":{seed}}}`, then write `{scope}` /
`{completion_key}` as `{{"writer":"x"}}`, and stop."#,
        id = rung.id(),
        scope = names.scope,
        seed = seed,
        final_key = FINAL_KEY,
        completion_key = COMPLETION_KEY,
    );

    let body = match rung {
        Rung::One => format!(
            r#"

1. Register barrier label `{label}` expecting exactly `"a"`.
2. Spawn child A as session `{a}` with only `state::set`, following the normal-child contract.
3. End the turn. When `{label}` wakes you, write exactly `{final}` to `{scope}` / `{final_key}`
   and reply `COORDINATION 1 COMPLETE`."#,
            label = names.label("complete"),
            a = names.child("a"),
            final = final_literal(rung, seed),
            scope = names.scope,
            final_key = FINAL_KEY,
        ),
        Rung::Two => format!(
            r#"

1. Register barrier label `{label}` expecting exactly `"a"` and `"b"`.
2. In one assistant message, spawn children A and B as sessions `{a}` and `{b}`, each with only
   `state::set` and the normal-child contract.
3. End the turn. When `{label}` wakes you, write exactly `{final}` to `{scope}` / `{final_key}`
   and reply `COORDINATION 2 COMPLETE`."#,
            label = names.label("complete"),
            a = names.child("a"),
            b = names.child("b"),
            final = final_literal(rung, seed),
            scope = names.scope,
            final_key = FINAL_KEY,
        ),
        Rung::Three => format!(
            r#"

1. Register barrier label `{wave1}` expecting exactly `"a"` and `"b"`.
2. In one assistant message spawn normal children A and B as `{a}` and `{b}`, then end the turn.
3. Only when `{wave1}` wakes you, register a new barrier label `{wave2}` expecting exactly `"c"`
   and `"d"`. In that same turn spawn normal children C and D as `{c}` and `{d}`, then end it.
4. Only when `{wave2}` wakes you, write exactly `{final}` to `{scope}` / `{final_key}` and reply
   `COORDINATION 3 COMPLETE`."#,
            wave1 = names.label("wave-1"),
            wave2 = names.label("wave-2"),
            a = names.child("a"),
            b = names.child("b"),
            c = names.child("c"),
            d = names.child("d"),
            final = final_literal(rung, seed),
            scope = names.scope,
            final_key = FINAL_KEY,
        ),
        Rung::Four => format!(
            r#"

1. Register barrier label `{initial}` expecting exactly `"a"`, `"b"`, `"c_failed"`, and `"d"`.
2. In one assistant message spawn A, B, and D as normal children (`{a}`, `{b}`, `{d}`). Also spawn
   initial child C as `{c}` with only `state::set`; C must write `{scope}` / `branch_c_failed` as
   `{{"branch":"c","status":"failed","reason":"injected_recoverable","case_seed":{seed}}}`,
   then write `{scope}` / `{completion_key}` as `{{"writer":"c_failed"}}`, and stop.
3. End the turn. Only when `{initial}` wakes you, register a one-shot state wake for key
   `branch_c`, label `{recovered}`, then spawn recovery child C as `{c_recovery}` with only
   `state::set`. It must write `{scope}` / `branch_c` as
   `{{"branch":"c","status":"recovered","recovered_from":"injected_recoverable","case_seed":{seed}}}`
   and stop. End your turn.
4. Only when `{recovered}` wakes you, write exactly `{final}` to `{scope}` / `{final_key}` and
   reply `COORDINATION 4 COMPLETE`."#,
            initial = names.label("initial-settled"),
            recovered = names.label("recovered"),
            a = names.child("a"),
            b = names.child("b"),
            c = names.child("c-initial"),
            d = names.child("d"),
            c_recovery = names.child("c-recovery"),
            scope = names.scope,
            completion_key = COMPLETION_KEY,
            seed = seed,
            final = final_literal(rung, seed),
            final_key = FINAL_KEY,
        ),
        Rung::Five => format!(
            r#"

1. Register barrier label `{wave1}` expecting exactly `"a"` and `"b"`.
2. In one assistant message spawn normal children A and B as `{a}` and `{b}`, then end the turn.
3. Only when `{wave1}` wakes you, register two wake-only triggers BEFORE spawning anything:
   - a one-shot state wake for key `branch_c_invalid`, label `{invalid}`;
   - a one-shot timer with `in_ms: 30000`, label `{deadline}`.
   Then in one assistant message spawn initial C as `{c}` and partial D as `{d}`, both with only
   `state::set`. C must write `{scope}` / `branch_c_invalid` as
   `{{"branch":"c","status":"invalid","missing":"validated","case_seed":{seed}}}` and stop.
   D must write `{scope}` / `branch_d_partial` as
   `{{"branch":"d","status":"partial_timeout","completed_units":1,"expected_units":2,"case_seed":{seed}}}`
   and stop without a completion event. End your turn.
4. Only when `{invalid}` wakes you, validate that its payload has status `invalid`, then spawn
   repair child C as `{c_repair}` with only `state::set`. It must write `{scope}` / `branch_c` as
   `{{"branch":"c","status":"validated","repaired":true,"case_seed":{seed}}}` and stop.
   End your turn without polling.
5. Only when `{deadline}` wakes you, treat D's persisted partial result as the bounded timeout
   outcome. Write exactly `{final}` to `{scope}` / `{final_key}` and reply
   `COORDINATION 5 COMPLETE`."#,
            wave1 = names.label("wave-1"),
            invalid = names.label("validation-required"),
            deadline = names.label("partial-deadline"),
            a = names.child("a"),
            b = names.child("b"),
            c = names.child("c-invalid"),
            d = names.child("d-partial"),
            c_repair = names.child("c-repair"),
            scope = names.scope,
            seed = seed,
            final = final_literal(rung, seed),
            final_key = FINAL_KEY,
        ),
    };
    format!("{preamble}{body}")
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    _run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let rung = rung_from_case(&observation.case)?;
        let deliverable = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID);
        let artifact_complete = deliverable.is_some_and(|deliverable| {
            deliverable
                .invariants
                .iter()
                .all(|invariant| invariant.passed)
        });
        let calls = common::function_calls(&observation.transcript);
        let spawns = calls
            .iter()
            .filter(|call| call.function_id == "harness::spawn")
            .count() as u64;
        let registrations = calls
            .iter()
            .filter(|call| call.function_id == "engine::register_trigger")
            .count();
        let root_writes = calls
            .iter()
            .filter(|call| call.function_id == "state::set")
            .collect::<Vec<_>>();
        let expected_final = final_value(rung, observation.case.seed);
        let root_final_only = root_writes.len() == 1
            && root_writes[0].arguments.get("key").and_then(Value::as_str) == Some(FINAL_KEY)
            && root_writes[0].arguments.get("value") == Some(&expected_final);
        let child_sessions = observation
            .metrics
            .by_session
            .iter()
            .filter(|session| session.depth == 1)
            .count() as u64;
        let deeper_sessions = observation
            .metrics
            .by_session
            .iter()
            .filter(|session| session.depth > 1)
            .count();
        let structure_valid = spawns == rung.expected_children()
            && child_sessions == rung.expected_children()
            && deeper_sessions == 0
            && registrations == rung.expected_wakes()
            && root_final_only
            && observation.metrics.totals.function_call_errors == 0;
        let response = observation.response.trim();
        let expected_signal = format!("COORDINATION {} COMPLETE", rung.number());
        let completion_signal = response.contains(&expected_signal);

        Ok(assessment::build_evaluation([
            DELIVERABLE.full_or_zero(
                artifact_complete,
                format!("captured exact durable artifact={artifact_complete}"),
            ),
            COORDINATION.full_or_zero(
                structure_valid,
                format!(
                    "spawns={spawns}/{}, child_sessions={child_sessions}/{}, deeper_sessions={deeper_sessions}, registrations={registrations}/{}, root_final_only={root_final_only}, function_errors={}",
                    rung.expected_children(),
                    rung.expected_children(),
                    rung.expected_wakes(),
                    observation.metrics.totals.function_call_errors,
                ),
            ),
            COMPLETION.award(
                if completion_signal { COMPLETION.weight() } else { 0 },
                format!("expected completion signal '{expected_signal}'"),
            )?,
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let rung = rung_from_case(&observation.case)?;
        let names = Names::new(rung, run_id);
        let mut branches = Map::new();
        for (name, key) in branch_keys(rung) {
            branches.insert(
                name.to_string(),
                read_state(context, &names.scope, key).await?,
            );
        }
        let final_marker = read_state(context, &names.scope, FINAL_KEY).await?;
        let content = json!({
            "rung": rung.number(),
            "branches": Value::Object(branches),
            "final": final_marker,
        });
        let expected = expected_artifact(rung, observation.case.seed);

        let calls = common::function_invocations(&observation.transcript);
        let spawns = calls
            .iter()
            .filter(|invocation| invocation.call.function_id == "harness::spawn")
            .collect::<Vec<_>>();
        let root_writes = calls
            .iter()
            .filter(|invocation| invocation.call.function_id == "state::set")
            .collect::<Vec<_>>();
        let child_sessions = observation
            .metrics
            .by_session
            .iter()
            .filter(|session| session.depth == 1)
            .count() as u64;
        let expected_children = rung.expected_children();
        let workflow_evidence = spawns.len() as u64 == expected_children
            && child_sessions == expected_children
            && root_writes.len() == 1
            && root_writes[0]
                .call
                .arguments
                .get("key")
                .and_then(Value::as_str)
                == Some(FINAL_KEY);

        let mut provenance = spawns
            .iter()
            .map(|invocation| ProvenanceEvidence {
                kind: "function_call".to_string(),
                source_id: invocation
                    .call_id
                    .clone()
                    .unwrap_or_else(|| "harness::spawn".to_string()),
                relation: "delegated_branch".to_string(),
            })
            .collect::<Vec<_>>();
        provenance.extend(
            branch_keys(rung)
                .into_iter()
                .map(|(_, key)| ProvenanceEvidence {
                    kind: "state_location".to_string(),
                    source_id: format!("{}/{}", names.scope, key),
                    relation: "captured_fragment".to_string(),
                }),
        );

        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_bundle".to_string(),
            content: content.clone().into(),
            invariants: vec![
                CapturedInvariant {
                    id: "matches_materialized_case".to_string(),
                    passed: content == expected,
                    reason: "captured branch bundle compared with the materialized case"
                        .to_string(),
                },
                CapturedInvariant {
                    id: "all_fragments_present".to_string(),
                    passed: content
                        .get("branches")
                        .and_then(Value::as_object)
                        .is_some_and(|branches| {
                            branches.len() == branch_keys(rung).len()
                                && branches.values().all(|value| !value.is_null())
                        }),
                    reason: format!(
                        "expected {} durable branch fragment(s)",
                        branch_keys(rung).len()
                    ),
                },
                CapturedInvariant {
                    id: "workflow_provenance".to_string(),
                    passed: workflow_evidence,
                    reason: format!(
                        "spawns={}, direct_children={child_sessions}, root_writes={}",
                        spawns.len(),
                        root_writes.len()
                    ),
                },
            ],
            provenance,
        }])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        for rung in [Rung::One, Rung::Two, Rung::Three, Rung::Four, Rung::Five] {
            let names = Names::new(rung, run_id);
            for key in all_state_keys() {
                let _: Value = context
                    .trigger("state::delete", json!({ "scope": names.scope, "key": key }))
                    .await?;
            }
        }
        Ok(())
    })
}

async fn read_state(context: &E2eContext, scope: &str, key: &str) -> Result<Value> {
    Ok(common::state_value(
        context
            .trigger("state::get", json!({ "scope": scope, "key": key }))
            .await?,
    ))
}

fn deliverable_contract(expected: Value) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "state_bundle".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({ "const": expected }),
            max_size_bytes: 32_768,
        }],
        invariants: vec![
            InvariantSpec {
                id: "matches_materialized_case".to_string(),
                description: "The captured bundle exactly matches the seeded case.".to_string(),
            },
            InvariantSpec {
                id: "all_fragments_present".to_string(),
                description: "Every expected durable branch fragment is present.".to_string(),
            },
            InvariantSpec {
                id: "workflow_provenance".to_string(),
                description: "Direct child spawns and root-only finalization are observed."
                    .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn expected_artifact(rung: Rung, seed: u64) -> Value {
    let branches = branch_values(rung, seed)
        .into_iter()
        .collect::<Map<String, Value>>();
    json!({
        "rung": rung.number(),
        "branches": branches,
        "final": final_value(rung, seed),
    })
}

fn branch_values(rung: Rung, seed: u64) -> BTreeMap<String, Value> {
    let normal = |branch: &str| {
        json!({
            "branch": branch,
            "status": "complete",
            "case_seed": seed,
        })
    };
    let mut values = BTreeMap::new();
    match rung {
        Rung::One => {
            values.insert("a".to_string(), normal("a"));
        }
        Rung::Two => {
            values.insert("a".to_string(), normal("a"));
            values.insert("b".to_string(), normal("b"));
        }
        Rung::Three => {
            for branch in ["a", "b", "c", "d"] {
                values.insert(branch.to_string(), normal(branch));
            }
        }
        Rung::Four => {
            for branch in ["a", "b", "d"] {
                values.insert(branch.to_string(), normal(branch));
            }
            values.insert(
                "c_failed".to_string(),
                json!({
                    "branch": "c",
                    "status": "failed",
                    "reason": "injected_recoverable",
                    "case_seed": seed,
                }),
            );
            values.insert(
                "c_recovered".to_string(),
                json!({
                    "branch": "c",
                    "status": "recovered",
                    "recovered_from": "injected_recoverable",
                    "case_seed": seed,
                }),
            );
        }
        Rung::Five => {
            for branch in ["a", "b"] {
                values.insert(branch.to_string(), normal(branch));
            }
            values.insert(
                "c_invalid".to_string(),
                json!({
                    "branch": "c",
                    "status": "invalid",
                    "missing": "validated",
                    "case_seed": seed,
                }),
            );
            values.insert(
                "c_validated".to_string(),
                json!({
                    "branch": "c",
                    "status": "validated",
                    "repaired": true,
                    "case_seed": seed,
                }),
            );
            values.insert(
                "d_partial".to_string(),
                json!({
                    "branch": "d",
                    "status": "partial_timeout",
                    "completed_units": 1,
                    "expected_units": 2,
                    "case_seed": seed,
                }),
            );
        }
    }
    values
}

fn branch_keys(rung: Rung) -> Vec<(&'static str, &'static str)> {
    match rung {
        Rung::One => vec![("a", "branch_a")],
        Rung::Two => vec![("a", "branch_a"), ("b", "branch_b")],
        Rung::Three => vec![
            ("a", "branch_a"),
            ("b", "branch_b"),
            ("c", "branch_c"),
            ("d", "branch_d"),
        ],
        Rung::Four => vec![
            ("a", "branch_a"),
            ("b", "branch_b"),
            ("c_failed", "branch_c_failed"),
            ("c_recovered", "branch_c"),
            ("d", "branch_d"),
        ],
        Rung::Five => vec![
            ("a", "branch_a"),
            ("b", "branch_b"),
            ("c_invalid", "branch_c_invalid"),
            ("c_validated", "branch_c"),
            ("d_partial", "branch_d_partial"),
        ],
    }
}

fn all_state_keys() -> &'static [&'static str] {
    &[
        COMPLETION_KEY,
        FINAL_KEY,
        "branch_a",
        "branch_b",
        "branch_c",
        "branch_c_failed",
        "branch_c_invalid",
        "branch_d",
        "branch_d_partial",
    ]
}

fn final_value(rung: Rung, seed: u64) -> Value {
    let mode = match rung {
        Rung::One => "single_child",
        Rung::Two => "parallel_merge",
        Rung::Three => "dependency_dag",
        Rung::Four => "partial_recovery",
        Rung::Five => "validated_partial_timeout",
    };
    json!({
        "scenario": rung.id(),
        "status": "complete",
        "mode": mode,
        "case_seed": seed,
    })
}

fn final_literal(rung: Rung, seed: u64) -> String {
    serde_json::to_string(&final_value(rung, seed)).expect("serialize static coordination marker")
}

fn rung_from_case(case: &ScenarioCase) -> Result<Rung> {
    match case.scenario_id.as_str() {
        ID_1 => Ok(Rung::One),
        ID_2 => Ok(Rung::Two),
        ID_3 => Ok(Rung::Three),
        ID_4 => Ok(Rung::Four),
        ID_5 => Ok(Rung::Five),
        id => anyhow::bail!("unknown coordination case '{id}'"),
    }
}

struct Names {
    rung: Rung,
    run_id: String,
    scope: String,
}

impl Names {
    fn new(rung: Rung, run_id: &str) -> Self {
        Self {
            rung,
            run_id: run_id.to_string(),
            scope: format!("e2e:{run_id}:coordination:{}", rung.number()),
        }
    }

    fn child(&self, role: &str) -> String {
        format!("{}-coord-{}-{role}", self.run_id, self.rung.number())
    }

    fn label(&self, phase: &str) -> String {
        format!("{}-coord-{}-{phase}", self.run_id, self.rung.number())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rung_materializes_a_distinct_versioned_case() {
        let cases = [Rung::One, Rung::Two, Rung::Three, Rung::Four, Rung::Five]
            .into_iter()
            .map(|rung| materialize(rung, "attempt", 7).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(cases.len(), 5);
        for (index, case) in cases.iter().enumerate() {
            assert_eq!(case.spec.version, VERSION);
            assert_eq!(case.case.inputs["rung"], json!(index + 1));
            assert_eq!(
                case.case.work.minimum_expected_work,
                [6, 10, 18, 22, 22][index]
            );
            assert!(case.capture.is_some());
        }
    }

    #[test]
    fn advanced_rungs_publish_recovery_and_timeout_evidence() {
        let recovery = expected_artifact(Rung::Four, 9);
        let adaptive = expected_artifact(Rung::Five, 9);

        assert_eq!(recovery["branches"]["c_failed"]["status"], "failed");
        assert_eq!(recovery["branches"]["c_recovered"]["status"], "recovered");
        assert_eq!(adaptive["branches"]["c_validated"]["repaired"], true);
        assert_eq!(
            adaptive["branches"]["d_partial"]["status"],
            "partial_timeout"
        );
    }

    #[test]
    fn ladder_increases_derived_complexity() {
        let tiers = [Rung::One, Rung::Two, Rung::Three, Rung::Four, Rung::Five]
            .into_iter()
            .map(|rung| {
                materialize(rung, "attempt", 7)
                    .unwrap()
                    .case
                    .complexity
                    .tier
            })
            .collect::<Vec<_>>();

        assert_ne!(tiers[0], tiers[1]);
        assert_eq!(tiers[2], tiers[3]);
        assert_ne!(tiers[3], tiers[4]);
    }
}
