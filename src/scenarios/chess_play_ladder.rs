//! `chess_play_ladder` — competitive strength against a deterministic engine.
//!
//! The subject plays a full game of chess as White from the initial position
//! against a deterministic negamax opponent at the retained maximum search
//! depth 3. Earlier easier opponents were removed to keep one comparable
//! cohort. Version over version, the win rate is the visible evolution curve.
//!
//! Per run the game lives in a `static` registry keyed by `run_id` (the same
//! stateful pattern `engineering_ticket` uses): `setup` inserts a fresh
//! [`ChessGame`] at the start position and registers a run-scoped move channel
//! `e2etest::chess_move_<suffix>` on the suite's own engine connection. The
//! subject calls that channel with each of its moves; the handler validates
//! the move from the current FEN, plays the deterministic engine reply, and
//! returns the new position. The evaluator reads the same live game state back
//! out after the run.
//!
//! A generated setup function fixes depth 3 in the [`ChessGame`] while keeping
//! runtime state isolated per `run_id`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use iii_sdk::RegisterFunction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::chess_engine;
use super::validation_loop::suffix;
use super::{
    ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture, ComplexityProfile,
    DeliverableCaptureFuture, DeliverableContract, EvaluationFuture, ExecutionPolicy,
    InvariantSpec, MaterializedScenario, ObjectiveEvaluation, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "chess_play_ladder";
const VERSION: u32 = 3;
const GAME_RECORD_ID: &str = "game_record";

/// The line the subject must end on: `CHESS-RESULT <win|draw|loss>`.
const RESULT_MARKER: &str = "CHESS-RESULT";

/// Maximum plies (half-moves) the game may run before it is deemed unfinished:
/// roughly 60 full moves. `max_turns` bounds the live turn count below this, so
/// a subject that never reaches a terminal result simply fails `game_completed`
/// rather than looping forever.
const MOVE_CAP: u32 = 120;

/// Game-scaled workload floor: a full game is far more work than the profile's
/// derived minimum, so pin it explicitly rather than letting the vector shrink
/// it toward the artifact count.
const MINIMUM_EXPECTED_WORK: u64 = 20;

#[derive(Debug, Clone, Copy)]
struct Rung {
    depth: u32,
}

pub const CANONICAL_SEED: u64 = 6003;
const RUNG: Rung = Rung { depth: 3 };

const ZERO_ILLEGAL_MOVES: AssessmentSpec = AssessmentSpec::hard_gated(
    "zero_illegal_moves",
    30,
    "The subject attempted no illegal move and the move channel reported no function-call errors.",
);
const GAME_COMPLETED: AssessmentSpec = AssessmentSpec::hard_gated_in(
    "game_completed",
    25,
    "The game reached a terminal result within the move cap rather than running out of moves unfinished.",
    EvaluationDimension::Deliverable,
);
const RESULT_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "result_reported",
    20,
    "The final response reports `CHESS-RESULT <r>` matching the game's recorded outcome.",
);
const STRENGTH_SCORE: AssessmentSpec = AssessmentSpec::score_only(
    "strength_score",
    25,
    "Competitive result against the negamax opponent, banded win/draw/loss (centipawn nuance deferred).",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    ZERO_ILLEGAL_MOVES,
    GAME_COMPLETED,
    RESULT_REPORTED,
    STRENGTH_SCORE,
];

/// The subject's move, as long-algebraic UCI coordinates (for example `e2e4`
/// or `e7e8q`). The body shape is `{ "uci": "<move>" }`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ChessMoveInput {
    #[serde(default)]
    pub uci: String,
}

/// The move channel's reply. Optional fields are omitted per branch so each
/// reply carries only the keys that apply to it.
#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct ChessMoveReply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub your_move_legal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opponent_uci: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fen: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Per-run game state. `depth` is the opponent's search depth, written at
/// insert time so the handler knows the opponent's strength without a side
/// channel.
struct ChessGame {
    fen: String,
    moves: Vec<String>,
    illegal_attempts: u32,
    /// Subject-perspective result once terminal: `"win"`, `"loss"`, `"draw"`.
    result: Option<String>,
    finished: bool,
    depth: u32,
}

type SharedGame = Arc<Mutex<ChessGame>>;

fn game_registry() -> &'static Mutex<HashMap<String, SharedGame>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, SharedGame>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A post-run read-only view of the live game.
struct GameSnapshot {
    moves: Vec<String>,
    illegal_attempts: u32,
    result: Option<String>,
    finished: bool,
}

fn read_game(run_id: &str) -> Option<GameSnapshot> {
    let registry = game_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let game = registry.get(run_id)?;
    let game = game.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(GameSnapshot {
        moves: game.moves.clone(),
        illegal_attempts: game.illegal_attempts,
        result: game.result.clone(),
        finished: game.finished,
    })
}

fn move_function_id(run_id: &str) -> String {
    format!("e2etest::chess_move_{}", suffix(run_id))
}

/// Map an engine result string to the SUBJECT's perspective (the subject is
/// always White): a White win is a `win`, a Black win is a `loss`, and a draw
/// stays a `draw`.
fn subject_perspective(engine_result: &str) -> &'static str {
    match engine_result {
        "white" => "win",
        "black" => "loss",
        _ => "draw",
    }
}

fn response_reports_result(response: &str, subject_result: &str) -> bool {
    response.contains(&format!("{RESULT_MARKER} {subject_result}"))
}

/// The pure game-state transition for one subject move at the given opponent
/// `depth`. Mutates `game` and returns the reply the handler serializes. Kept
/// free of the async handler so it can be driven directly in tests.
fn step(game: &mut ChessGame, uci: &str, depth: u32) -> ChessMoveReply {
    if game.finished {
        return ChessMoveReply {
            status: game.result.clone().unwrap_or_else(|| "draw".to_string()),
            finished: Some(true),
            note: Some("game already over".to_string()),
            ..ChessMoveReply::default()
        };
    }

    // Validate and play the subject's move from the current position.
    let outcome = match chess_engine::apply_move(&game.fen, uci) {
        Ok(outcome) => outcome,
        Err(_) => {
            game.illegal_attempts += 1;
            return ChessMoveReply {
                your_move_legal: Some(false),
                fen: Some(game.fen.clone()),
                status: "illegal".to_string(),
                guidance: Some(
                    "that move is not legal in the current position; consult the FEN and try a legal move"
                        .to_string(),
                ),
                ..ChessMoveReply::default()
            };
        }
    };
    game.fen = outcome.new_fen;
    game.moves.push(uci.to_string());

    // A move that ends the game (checkmate or a draw the subject delivered)
    // leaves no engine reply.
    if let Some(result) = outcome.result {
        let perspective = subject_perspective(&result);
        game.result = Some(perspective.to_string());
        game.finished = true;
        return ChessMoveReply {
            your_move_legal: Some(true),
            fen: Some(game.fen.clone()),
            status: perspective.to_string(),
            finished: Some(true),
            ..ChessMoveReply::default()
        };
    }

    // The position after the subject's move is non-terminal, so a deterministic
    // engine reply exists. The `None` and error arms cannot fire in practice
    // (negamax returns a legal move for a non-terminal position); they stay
    // deterministic instead of panicking.
    let Some(opponent_uci) = chess_engine::negamax_best_move(&game.fen, depth) else {
        return ChessMoveReply {
            your_move_legal: Some(true),
            fen: Some(game.fen.clone()),
            status: "ongoing".to_string(),
            finished: Some(false),
            ..ChessMoveReply::default()
        };
    };
    let opponent_outcome = match chess_engine::apply_move(&game.fen, &opponent_uci) {
        Ok(outcome) => outcome,
        Err(_) => {
            return ChessMoveReply {
                your_move_legal: Some(true),
                fen: Some(game.fen.clone()),
                status: "ongoing".to_string(),
                finished: Some(false),
                ..ChessMoveReply::default()
            };
        }
    };
    game.fen = opponent_outcome.new_fen;
    game.moves.push(opponent_uci.clone());
    let (status, finished) = match opponent_outcome.result {
        Some(result) => {
            let perspective = subject_perspective(&result);
            game.result = Some(perspective.to_string());
            game.finished = true;
            (perspective.to_string(), true)
        }
        None => ("ongoing".to_string(), false),
    };
    ChessMoveReply {
        your_move_legal: Some(true),
        opponent_uci: Some(opponent_uci),
        fen: Some(game.fen.clone()),
        status,
        finished: Some(finished),
        ..ChessMoveReply::default()
    }
}

/// Insert a fresh game at the start position for this run and register the
/// run-scoped move channel. The opponent `depth` is baked in here — the only
/// per-rung value the setup hook needs — and stored in the game so the handler
/// can read it back.
fn setup_game<'a>(context: &'a E2eContext, run_id: &'a str, depth: u32) -> CleanupFuture<'a> {
    Box::pin(async move {
        let game = Arc::new(Mutex::new(ChessGame {
            fen: chess_engine::STARTPOS.to_string(),
            moves: Vec::new(),
            illegal_attempts: 0,
            result: None,
            finished: false,
            depth,
        }));
        game_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id.to_string(), game.clone());

        context.client().register_function(
            move_function_id(run_id),
            RegisterFunction::new_async(move |input: ChessMoveInput| {
                let game = game.clone();
                async move {
                    let reply = {
                        let mut game = game.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        let depth = game.depth;
                        step(&mut game, &input.uci, depth)
                    };
                    Ok::<ChessMoveReply, iii_sdk::errors::Error>(reply)
                }
            })
            .description(
                "E2E chess move channel: validates the subject's move from the current FEN, plays \
                 the deterministic negamax opponent reply, and returns the new position.",
            ),
        );
        Ok(())
    })
}

macro_rules! rung_setup {
    ($name:ident, $depth:expr) => {
        fn $name<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
            setup_game(context, run_id, $depth)
        }
    };
}

rung_setup!(setup_depth_3, 3);

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id, RUNG)
}

pub fn materialize(namespace: &str, _seed: u64) -> anyhow::Result<MaterializedScenario> {
    let rung = RUNG;
    let case = ScenarioCase::new(
        ID,
        VERSION,
        CANONICAL_SEED,
        // Nothing here is run-scoped: inputs are identical across namespaces
        // for a given seed, so canonical identity stays stable across attempts.
        json!({
            "opponent_depth": rung.depth,
            "move_cap_plies": MOVE_CAP,
            "start_fen": chess_engine::STARTPOS,
            "result_marker": RESULT_MARKER,
        }),
        complexity_profile(),
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
        ],
        deliverable_contract(),
    )?
    // A full game is the workload; the derived floor is far too small for it.
    .with_minimum_expected_work(MINIMUM_EXPECTED_WORK)?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace, rung),
        case,
        capture: Some(capture),
    })
}

/// Fixed across rungs on purpose: opponent strength scales the DIFFICULTY of
/// winning, not the SHAPE of the work — every rung plays exactly one full
/// stateful game — so the tier stays `L2Stateful` and the ladder reads as a
/// pure strength curve rather than a tier climb.
fn complexity_profile() -> ComplexityProfile {
    ComplexityProfile {
        external_systems: 1,
        state_transitions: MOVE_CAP as u16,
        dependency_depth: 2,
        artifact_count: 1,
        ..ComplexityProfile::default()
    }
}

fn scenario_for_case(run_id: &str, rung: Rung) -> ScenarioSpec {
    let move_function = move_function_id(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: prompt(&move_function, rung.depth),
        filesystem_root: None,
        execution: ExecutionPolicy {
            // One live turn per subject move, plus slack for discovery, the
            // occasional illegal-move retry, and the final report. This exceeds
            // the other scenarios' turn counts on purpose; `ExecutionPolicy`
            // only rejects zero and total < output, so a long game is allowed.
            max_turns: 8 + MOVE_CAP,
            max_output_tokens: Some(8_192),
            // Unbounded on purpose: this is a measurement ladder — capping the
            // shared token budget would distort the strength signal the ladder
            // exists to observe. Spend surfaces in the Efficiency dimension.
            max_total_tokens: None,
            stuck_timeout_seconds: 600,
            max_validation_retries: None,
        },
        denied_functions: &["state::*"],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: Some(setup_depth_3),
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn prompt(move_function: &str, depth: u32) -> String {
    format!(
        r#"You are playing a full game of chess as White against a deterministic engine whose search depth is {depth}.

A move channel is already registered: `{move_function}`. Send each of your moves by calling it with `{{"uci": "e2e4"}}`-style long-algebraic (UCI) coordinates — the from-square followed by the to-square, plus a promotion piece letter when you promote (for example `e7e8q`). Every reply tells you:
- `your_move_legal` — whether your move was accepted,
- `opponent_uci` — the engine's reply move, when the game continues,
- `fen` — the resulting position in FEN, after both moves,
- `status` — one of `ongoing`, `win`, `loss`, `draw`, or `illegal`,
- `finished` — whether the game is over.

You start from the standard initial position `{start}`. No board is drawn for you: track the position yourself from the FEN in each reply. If a reply comes back with `status` `illegal`, the board did not move — choose a legal move and try again. Keep sending moves until `status` is no longer `ongoing`.

When the game ends, finish with a single line matching the final status — exactly `{marker} win`, `{marker} draw`, or `{marker} loss` — and mention the engine search depth ({depth}) you faced."#,
        start = chess_engine::STARTPOS,
        marker = RESULT_MARKER,
    )
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move { evaluate_game(observation, run_id) })
}

fn evaluate_game(
    observation: &ScenarioObservation,
    run_id: &str,
) -> anyhow::Result<ObjectiveEvaluation> {
    let function_errors = observation.metrics.totals.function_call_errors;
    let Some(snapshot) = read_game(run_id) else {
        // The per-run game is the prerequisite for every assessment here.
        return Ok(assessment::prerequisite_failure(
            ASSESSMENTS,
            "game_state_present",
            "no per-run chess game was registered for this run",
        ));
    };
    let plies = snapshot.moves.len() as u32;
    let within_cap = plies <= MOVE_CAP;
    let zero_illegal = snapshot.illegal_attempts == 0 && function_errors == 0;
    let completed = snapshot.finished && snapshot.result.is_some() && within_cap;
    let reported = snapshot
        .result
        .as_deref()
        .is_some_and(|result| response_reports_result(&observation.response, result));
    let strength = match snapshot.result.as_deref() {
        Some("win") => 25,
        Some("draw") => 12,
        _ => 0,
    };
    let strength_award = STRENGTH_SCORE.award(
        strength,
        format!(
            "result {:?} banded to {strength}/25 (win=25, draw=12, loss=0; competitive-loss \
             nuance via centipawn is deferred)",
            snapshot.result
        ),
    )?;

    Ok(assessment::build_evaluation(
        if completed {
            crate::report::CompletionState::Completed
        } else {
            crate::report::CompletionState::TaskIncomplete
        },
        [
            ZERO_ILLEGAL_MOVES.full_or_zero(
                zero_illegal,
                format!(
                    "illegal_attempts={}, function_call_errors={function_errors}",
                    snapshot.illegal_attempts
                ),
            ),
            GAME_COMPLETED.full_or_zero(
                completed,
                format!(
                    "finished={}, result={:?}, plies={plies} (cap {MOVE_CAP})",
                    snapshot.finished, snapshot.result
                ),
            ),
            RESULT_REPORTED.full_or_zero(
                reported,
                format!(
                    "final response must contain `{RESULT_MARKER} {}`",
                    snapshot.result.as_deref().unwrap_or("<result>")
                ),
            ),
            strength_award,
        ],
    ))
}

fn case_depth(case: &ScenarioCase) -> u32 {
    case.inputs
        .get("opponent_depth")
        .and_then(Value::as_u64)
        .and_then(|depth| u32::try_from(depth).ok())
        .unwrap_or(1)
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let function_errors = observation.metrics.totals.function_call_errors;
        // Depth comes from the authoritative case inputs (always 1..=3), so the
        // record stays schema-valid even on the defensive missing-game path.
        let depth = case_depth(&observation.case);
        let (moves, result, illegal_attempts, finished, plies) = match read_game(run_id) {
            Some(snapshot) => {
                let plies = snapshot.moves.len() as u32;
                (
                    snapshot.moves,
                    snapshot.result,
                    snapshot.illegal_attempts,
                    snapshot.finished,
                    plies,
                )
            }
            None => (Vec::new(), None, 0u32, false, 0u32),
        };
        let within_cap = plies <= MOVE_CAP;
        let completed = finished && result.is_some() && within_cap;
        let zero_illegal = illegal_attempts == 0 && function_errors == 0;
        // Provenance links the record to the move channel that actually played
        // the game, but only when the game both completed and stayed clean.
        let provenance = if completed && zero_illegal {
            vec![ProvenanceEvidence {
                kind: "function".to_string(),
                source_id: move_function_id(run_id),
                relation: "played_opponent".to_string(),
            }]
        } else {
            Vec::new()
        };
        Ok(vec![CapturedDeliverable {
            id: GAME_RECORD_ID.to_string(),
            kind: "game_record".to_string(),
            content: json!({
                "moves": moves,
                "result": result.clone().unwrap_or_default(),
                "illegal_attempts": illegal_attempts,
                "opponent_depth": depth,
            })
            .into(),
            invariants: vec![
                CapturedInvariant {
                    id: "game_completed".to_string(),
                    passed: completed,
                    reason: format!(
                        "finished={finished}, result={result:?}, plies={plies} (cap {MOVE_CAP})"
                    ),
                },
                CapturedInvariant {
                    id: "zero_illegal_moves".to_string(),
                    passed: zero_illegal,
                    reason: format!(
                        "illegal_attempts={illegal_attempts}, function_call_errors={function_errors}"
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
            id: GAME_RECORD_ID.to_string(),
            kind: "game_record".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["moves", "result", "illegal_attempts", "opponent_depth"],
                "properties": {
                    "moves": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "result": { "type": "string" },
                    "illegal_attempts": { "type": "integer", "minimum": 0 },
                    "opponent_depth": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 16_384,
        }],
        invariants: vec![
            InvariantSpec {
                id: "game_completed".to_string(),
                description:
                    "The game reached a terminal result within the move cap rather than running out of moves unfinished."
                        .to_string(),
            },
            InvariantSpec {
                id: "zero_illegal_moves".to_string(),
                description:
                    "The subject attempted no illegal move and the move channel reported no errors."
                        .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

/// Nothing scenario-owned outlives the process except the registered function
/// (registered on the suite's own engine connection, so it needs no
/// unregister). Drop the run's live game from the registry.
fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        game_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_game(depth: u32) -> ChessGame {
        ChessGame {
            fen: chess_engine::STARTPOS.to_string(),
            moves: Vec::new(),
            illegal_attempts: 0,
            result: None,
            finished: false,
            depth,
        }
    }

    #[test]
    fn every_seed_request_normalizes_to_the_maximum_case() {
        assert_eq!(RUNG.depth, 3);
        assert_eq!(
            materialize("attempt", 6002).unwrap().case.seed,
            CANONICAL_SEED
        );
    }

    #[test]
    fn subject_perspective_maps_engine_results_to_the_white_view() {
        assert_eq!(subject_perspective("white"), "win");
        assert_eq!(subject_perspective("black"), "loss");
        assert_eq!(subject_perspective("draw"), "draw");
        // Anything unexpected reads as a draw rather than a false win/loss.
        assert_eq!(subject_perspective("unknown"), "draw");
    }

    #[test]
    fn result_marker_validator_requires_the_matching_token() {
        assert!(response_reports_result(
            "Good game.\nCHESS-RESULT win",
            "win"
        ));
        assert!(response_reports_result(
            "CHESS-RESULT draw at depth 2",
            "draw"
        ));
        assert!(!response_reports_result("CHESS-RESULT win", "loss"));
        assert!(!response_reports_result("no marker here", "draw"));
    }

    #[test]
    fn step_flags_an_illegal_move_without_advancing_the_board() {
        let mut game = fresh_game(1);
        // A pawn cannot leap to e5 from the start position.
        let reply = step(&mut game, "e2e5", 1);
        assert_eq!(reply.your_move_legal, Some(false));
        assert_eq!(reply.status, "illegal");
        assert!(reply.guidance.is_some());
        assert_eq!(game.illegal_attempts, 1);
        assert!(game.moves.is_empty());
        assert_eq!(game.fen, chess_engine::STARTPOS);
        assert!(!game.finished);
    }

    #[test]
    fn step_plays_a_legal_move_and_a_deterministic_engine_reply() {
        let mut game = fresh_game(1);
        let reply = step(&mut game, "e2e4", 1);
        assert_eq!(reply.your_move_legal, Some(true));
        assert_eq!(reply.status, "ongoing");
        assert_eq!(reply.finished, Some(false));
        assert!(reply.opponent_uci.is_some());
        // Subject move plus the engine reply.
        assert_eq!(game.moves.len(), 2);
        assert!(!game.finished);
        assert_ne!(game.fen, chess_engine::STARTPOS);
    }

    #[test]
    fn step_records_a_win_when_the_subject_delivers_mate() {
        // White to move with a back-rank mate in one: Ra1-a8#. The subject's own
        // move ends the game, so there is no engine reply.
        let mut game = ChessGame {
            fen: "6k1/5ppp/8/8/8/8/8/R6K w - - 0 1".to_string(),
            moves: Vec::new(),
            illegal_attempts: 0,
            result: None,
            finished: false,
            depth: 1,
        };
        let reply = step(&mut game, "a1a8", 1);
        assert_eq!(reply.your_move_legal, Some(true));
        assert_eq!(reply.status, "win");
        assert_eq!(reply.finished, Some(true));
        assert_eq!(reply.opponent_uci, None);
        assert!(game.finished);
        assert_eq!(game.result.as_deref(), Some("win"));
        assert_eq!(game.moves, vec!["a1a8".to_string()]);
    }

    #[test]
    fn step_short_circuits_once_the_game_is_over() {
        let mut game = fresh_game(1);
        game.finished = true;
        game.result = Some("draw".to_string());
        let reply = step(&mut game, "e2e4", 1);
        assert_eq!(reply.status, "draw");
        assert_eq!(reply.finished, Some(true));
        assert_eq!(reply.note.as_deref(), Some("game already over"));
        // A finished game never advances, even on a would-be-legal move.
        assert!(game.moves.is_empty());
        assert_eq!(game.illegal_attempts, 0);
    }

    #[test]
    fn retained_case_is_reproducible_and_uses_the_maximum_depth() {
        use super::super::ComplexityTier;

        let first = materialize("attempt-a", CANONICAL_SEED).unwrap();
        let retry = materialize("attempt-b", CANONICAL_SEED).unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs, retry.case.inputs);
        assert_eq!(first.case.inputs_sha256, retry.case.inputs_sha256);
        assert_eq!(
            first
                .case
                .inputs
                .get("opponent_depth")
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(first.case.complexity.tier, ComplexityTier::L2Stateful);
        assert_eq!(
            usize::from(first.case.complexity.profile.artifact_count),
            first.case.deliverable_contract.artifacts.len()
        );
        assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
        assert_eq!(first.case.work.minimum_expected_work, MINIMUM_EXPECTED_WORK);
        assert_eq!(first.spec.execution.max_turns, 8 + MOVE_CAP);
        assert!(first.spec.execution.max_total_tokens.is_none());
        assert!(first.capture.is_some());
        first.validate().unwrap();
    }

    #[test]
    fn the_materialized_spec_and_case_pass_the_shared_contract() {
        // The same invariants the registry-wide test enforces for a wired
        // scenario, checked here so the file stands on its own.
        let materialized = materialize("chess", CANONICAL_SEED).unwrap();
        materialized.validate().unwrap();
        assert_eq!(materialized.spec.id, ID);
        assert_eq!(materialized.spec.version, VERSION);
        let weights: u16 = materialized
            .spec
            .criteria
            .iter()
            .map(|criterion| u16::from(criterion.weight))
            .sum();
        assert_eq!(weights, 100);
        assert!(
            materialized
                .case
                .deliverable_contract
                .capture_before_cleanup
        );
    }
}
