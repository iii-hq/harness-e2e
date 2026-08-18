//! Learn a world's rules by acting in it, across three levels that permute
//! those rules.
//!
//! What is hard-gated is what the stack owns: every action came back, nothing
//! was lost or duplicated, work stayed bounded, and the final report matches
//! what the world recorded. How far the session got, and how many actions it
//! spent getting there, are advisory scores. That split is deliberate: a
//! weaker model should move the score, not fail the gate, so the same
//! scenario run against two harness revisions with the same model reads as a
//! delta rather than as noise.

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::probe;
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::world::{World, ACTIONS, LEVELS, SIZE};

pub const ID: &str = "longhorizon.hidden_rule_world";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "hidden_rule_world_run";
const ACTION_BUDGET: usize = 150;
/// Above this, the session was thrashing rather than learning.
const EFFICIENT_ACTIONS_PER_LEVEL: f64 = 25.0;

const EVERY_ACTION_ANSWERED: AssessmentSpec = AssessmentSpec::hard_gated(
    "every_action_answered",
    20,
    "Every action the session issued reached the world and returned exactly once.",
);
const WORK_BOUNDED: AssessmentSpec = AssessmentSpec::hard_gated(
    "work_bounded",
    15,
    "The session stayed inside its action budget and issued no malformed actions.",
);
const REPORT_HONEST: AssessmentSpec = AssessmentSpec::hard_gated(
    "report_honest",
    15,
    "The reported progress equals what the world recorded, not what the session believed.",
);
const LEVELS_SOLVED: AssessmentSpec = AssessmentSpec::score_only(
    "levels_solved",
    35,
    "How many levels the session actually completed. Advisory: this is where a harness change shows up as a delta.",
);
const EXPLORATION_EFFICIENCY: AssessmentSpec = AssessmentSpec::score_only(
    "exploration_efficiency",
    15,
    "Actions spent per solved level. Advisory: two harnesses that both finish are separated by this.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    EVERY_ACTION_ANSWERED,
    WORK_BOUNDED,
    REPORT_HONEST,
    LEVELS_SOLVED,
    EXPLORATION_EFFICIENCY,
];

fn world_id(run_id: &str) -> String {
    probe::id("world", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let id = world_id(run_id);
        let world = Arc::new(Mutex::new(World::new()));
        probe::register(
            context,
            id.clone(),
            "E2E temporary world: applies one action and returns what can be observed.",
            move |request: Value| {
                let id = id.clone();
                let world = Arc::clone(&world);
                async move {
                    probe::record_hit(&id);
                    let action = request
                        .get("action")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let mut world = world.lock().unwrap_or_else(PoisonError::into_inner);
                    let observation = world.act(&action);
                    let ledger = probe::ledger(&id);
                    let mut entries = ledger.lock().unwrap_or_else(PoisonError::into_inner);
                    entries.insert("levels_solved".into(), Value::from(world.solved() as u64));
                    entries.insert(
                        "actions_used".into(),
                        Value::from(world.actions_used() as u64),
                    );
                    entries.insert(
                        "invalid_actions".into(),
                        Value::from(world.invalid_actions() as u64),
                    );
                    if let Some(average) = world.actions_per_solved_level() {
                        entries.insert("actions_per_level".into(), json!(average));
                    }
                    Ok(observation)
                }
            },
        );
        Ok(())
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let world = world_id(run_id);
    let actions = ACTIONS.join("`, `");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "You are placed in a world whose rules are not documented. Learn them by acting.\n\n\
             1. Call `{world}` with payload {{\"action\": \"<label>\"}} where the label is one of \
             `{actions}`. Each call applies that action and returns what you can observe: your \
             position on a {SIZE} by {SIZE} grid, the goal, whether a key is present and whether \
             you are carrying it, the level you are on, and how many levels you have solved.\n\
             2. Each label moves you one step, but which direction a label means is not stated \
             and changes from level to level. Work it out by acting and watching the position \
             change.\n\
             3. Reaching the goal completes a level and starts the next one. When a level shows a \
             key, you must stand on the key before the goal counts.\n\
             4. There are {LEVELS} levels and a budget of {ACTION_BUDGET} actions in total. \
             Spend them on learning the mapping rather than guessing; an unknown label is wasted \
             and changes nothing.\n\
             5. Stop when you have solved all {LEVELS} levels or the budget is gone, and reply \
             with exactly one line: `LEVELS_SOLVED:<n> ACTIONS:<n>` using the numbers the world \
             last reported to you, not your own estimate."
        ),
        filesystem_root: None,
        execution: kit::policy(60, 800_000, 900),
        assessments: ASSESSMENTS,
        setup: Some(setup),
        evaluate,
        cleanup: Some(probe::cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "world_function": "e2etest::world_<run>",
            "levels": LEVELS,
            "grid": SIZE,
            "action_budget": ACTION_BUDGET,
            "scoring": {
                "hard_gated": ["every_action_answered", "work_bounded", "report_honest"],
                "advisory": ["levels_solved", "exploration_efficiency"],
                "comparison": "advisory scores carry the harness delta when the model is held fixed",
            },
        }),
        super::exploration_profile(),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["levels_solved", "actions_used", "response"],
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

struct Progress {
    solved: usize,
    actions: usize,
    invalid: usize,
    per_level: Option<f64>,
    issued: usize,
    delivered: usize,
}

fn progress(observation: &ScenarioObservation, run_id: &str) -> Progress {
    let world = world_id(run_id);
    Progress {
        solved: probe::ledger_u64(&world, "levels_solved") as usize,
        actions: probe::ledger_u64(&world, "actions_used") as usize,
        invalid: probe::ledger_u64(&world, "invalid_actions") as usize,
        per_level: probe::ledger_value(&world, "actions_per_level")
            .as_ref()
            .and_then(Value::as_f64),
        issued: kit::calls_of(&common::function_calls(&observation.transcript), &world).len(),
        delivered: probe::hits(&world) as usize,
    }
}

/// Advisory points scale with progress rather than gating on it.
fn solved_points(solved: usize) -> u8 {
    let share = solved.min(LEVELS) as f64 / LEVELS as f64;
    (f64::from(LEVELS_SOLVED.weight()) * share).round() as u8
}

fn efficiency_points(progress: &Progress) -> u8 {
    match progress.per_level {
        Some(average) if progress.solved > 0 => {
            let ratio = (EFFICIENT_ACTIONS_PER_LEVEL / average.max(1.0)).min(1.0);
            (f64::from(EXPLORATION_EFFICIENCY.weight()) * ratio).round() as u8
        }
        _ => 0,
    }
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let progress = progress(observation, run_id);
        let reported = format!(
            "LEVELS_SOLVED:{} ACTIONS:{}",
            progress.solved, progress.actions
        );

        Ok(assessment::build_evaluation([
            EVERY_ACTION_ANSWERED.full_or_zero(
                progress.issued > 0
                    && progress.issued == progress.delivered
                    && observation.metrics.totals.function_call_errors == 0,
                format!(
                    "issued {} action(s), the world saw {}; {} function-call error(s)",
                    progress.issued,
                    progress.delivered,
                    observation.metrics.totals.function_call_errors
                ),
            ),
            WORK_BOUNDED.full_or_zero(
                progress.actions <= ACTION_BUDGET && progress.invalid == 0,
                format!(
                    "spent {} of {ACTION_BUDGET} action(s) with {} malformed",
                    progress.actions, progress.invalid
                ),
            ),
            REPORT_HONEST.full_or_zero(
                observation.response.contains(&reported),
                format!("expected `{reported}` in the response"),
            ),
            LEVELS_SOLVED.award(
                solved_points(progress.solved),
                format!("solved {} of {LEVELS} level(s)", progress.solved),
            )?,
            EXPLORATION_EFFICIENCY.award(
                efficiency_points(&progress),
                format!(
                    "{:?} action(s) per solved level, full marks at {EFFICIENT_ACTIONS_PER_LEVEL} or fewer",
                    progress.per_level
                ),
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
        let progress = progress(observation, run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "levels_solved": progress.solved,
                "actions_used": progress.actions,
                "actions_per_solved_level": progress.per_level,
                "invalid_actions": progress.invalid,
                "output_tokens": observation.metrics.totals.output_tokens,
                "cost_usd": observation.metrics.totals.cost_usd,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&world_id(run_id), "owned_the_world_state"),
                kit::session_provenance(observation, "captured_world_progress"),
            ],
        )])
    })
}
