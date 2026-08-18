//! A fully specified grid game. The agent implements the rules and plays a
//! scripted run; the runner replays the same script and compares every step.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.game_simulation";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "game_simulation_artifact";
const TRACE_FILE: &str = "game/trace.json";
const RULES_FILE: &str = "game/rules.md";
const GRID: i64 = 5;
const COIN_VALUE: i64 = 10;
const BUMP_PENALTY: i64 = 1;
const COINS: [(i64, i64); 4] = [(1, 0), (2, 2), (4, 1), (3, 4)];
const MOVES: [char; 16] = [
    'U', 'R', 'R', 'D', 'D', 'L', 'U', 'R', 'D', 'D', 'R', 'R', 'U', 'L', 'D', 'D',
];

const TRACE_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "trace_exact",
    50,
    "Every step of the produced trace matches the rules replayed by the runner.",
);
const RULES_DOCUMENTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "rules_documented",
    20,
    "The rules file records how coins and out-of-grid moves are scored.",
);
const FINAL_STATE_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "final_state_reported",
    30,
    "The response reports the final position and score the rules produce.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[TRACE_EXACT, RULES_DOCUMENTED, FINAL_STATE_REPORTED];

fn reference_trace() -> Vec<Value> {
    let mut coins: Vec<(i64, i64)> = COINS.to_vec();
    let (mut x, mut y, mut score) = (0_i64, 0_i64, 0_i64);
    let mut trace = Vec::new();
    for (index, movement) in MOVES.iter().enumerate() {
        let (dx, dy) = match movement {
            'U' => (0, -1),
            'D' => (0, 1),
            'L' => (-1, 0),
            _ => (1, 0),
        };
        let (next_x, next_y) = (x + dx, y + dy);
        if (0..GRID).contains(&next_x) && (0..GRID).contains(&next_y) {
            x = next_x;
            y = next_y;
            if let Some(position) = coins.iter().position(|coin| *coin == (x, y)) {
                coins.remove(position);
                score += COIN_VALUE;
            }
        } else {
            score -= BUMP_PENALTY;
        }
        trace.push(json!({ "step": index + 1, "x": x, "y": y, "score": score }));
    }
    trace
}

fn final_state() -> (i64, i64, i64) {
    let trace = reference_trace();
    let last = trace.last().cloned().unwrap_or(Value::Null);
    (
        last.get("x").and_then(Value::as_i64).unwrap_or_default(),
        last.get("y").and_then(Value::as_i64).unwrap_or_default(),
        last.get("score")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
    )
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let moves: String = MOVES.iter().collect();
    let coins = COINS
        .iter()
        .map(|(x, y)| format!("({x},{y})"))
        .collect::<Vec<_>>()
        .join(", ");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Implement a small grid game and play one scripted run in this workspace.\n\n\
             Rules, in full:\n\
             - The grid is {GRID} by {GRID}. `x` runs 0 to {last} left to right, `y` runs 0 to \
             {last} top to bottom.\n\
             - The player starts at x=0, y=0 with score 0.\n\
             - Coins sit at {coins}. Each coin can be collected once.\n\
             - `U` decreases y by 1, `D` increases y by 1, `L` decreases x by 1, `R` increases x \
             by 1.\n\
             - A move that would leave the grid is a bump: the player does not move and the \
             score decreases by {BUMP_PENALTY}.\n\
             - Landing on a square that still holds a coin collects it and increases the score \
             by {COIN_VALUE}.\n\n\
             The scripted run is exactly this sequence of {count} moves: `{moves}`.\n\n\
             1. Write `{RULES_FILE}` documenting the rules you implemented, including how a coin \
             and a bump change the score.\n\
             2. Play the scripted run and write `{TRACE_FILE}` in the shape \
             {{\"steps\": [{{\"step\": 1, \"x\": 0, \"y\": 0, \"score\": 0}}, ...]}} with one \
             entry per move, in order, recording the state after that move.\n\
             3. Reply with exactly one line: `FINAL:x=<x>,y=<y>,score=<score>`.",
            last = GRID - 1,
            count = MOVES.len(),
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(24, 320_000, 900),
        assessments: ASSESSMENTS,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let (x, y, score) = final_state();
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "grid": GRID,
            "coins": COINS.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
            "moves": MOVES.iter().collect::<String>(),
            "coin_value": COIN_VALUE,
            "bump_penalty": BUMP_PENALTY,
            "final": { "x": x, "y": y, "score": score },
        }),
        super::build_profile(2, 2),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["steps", "rules_present", "response"],
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

fn observed_trace(run_id: &str) -> Vec<Value> {
    workspace::read_json(&workspace::root(ID, run_id), TRACE_FILE)
        .and_then(|trace| trace.get("steps").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        let observed = observed_trace(run_id);
        let expected = reference_trace();
        let rules = workspace::read(&root, RULES_FILE).unwrap_or_default();
        let rules_documented = rules.to_lowercase().contains("coin")
            && (rules.to_lowercase().contains("bump")
                || rules.to_lowercase().contains("outside")
                || rules.to_lowercase().contains("off the grid"));
        let (x, y, score) = final_state();
        let summary = format!("FINAL:x={x},y={y},score={score}");
        let first_divergence = observed
            .iter()
            .zip(expected.iter())
            .position(|(left, right)| left != right);

        Ok(assessment::build_evaluation([
            TRACE_EXACT.full_or_zero(
                observed == expected,
                format!(
                    "observed {} step(s) of {}; first divergence at index {first_divergence:?}",
                    observed.len(),
                    expected.len()
                ),
            ),
            RULES_DOCUMENTED.full_or_zero(
                rules_documented,
                format!("`{RULES_FILE}` holds {} character(s)", rules.trim().len()),
            ),
            FINAL_STATE_REPORTED.full_or_zero(
                observation.response.contains(&summary),
                format!("expected `{summary}` in the response"),
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
        let root = workspace::root(ID, run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "steps": observed_trace(run_id),
                "rules_present": workspace::read(&root, RULES_FILE).is_some(),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_game_trace_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_run_bumps_once_and_collects_three_coins() {
        let (x, y, score) = final_state();
        assert_eq!((x, y), (3, 4));
        assert_eq!(score, 3 * COIN_VALUE - BUMP_PENALTY);
    }

    #[test]
    fn the_trace_records_one_entry_per_move() {
        assert_eq!(reference_trace().len(), MOVES.len());
        assert_eq!(
            reference_trace()[0],
            json!({ "step": 1, "x": 0, "y": 0, "score": -1 })
        );
    }

    #[test]
    fn the_player_never_leaves_the_grid() {
        for step in reference_trace() {
            let x = step.get("x").and_then(Value::as_i64).unwrap();
            let y = step.get("y").and_then(Value::as_i64).unwrap();
            assert!((0..GRID).contains(&x) && (0..GRID).contains(&y));
        }
    }
}
