//! Shared chess kernel — the deterministic ground truth for the chess
//! scenarios (`chess_engine_build`, `chess_play_ladder`).
//!
//! This is a support module, NOT a registered scenario. It wraps `shakmaty`
//! so both scenarios share one correct move generator: the build scenario
//! verifies a subject-authored engine against `perft`/`legal_moves`, and the
//! play ladder drives a deterministic negamax opponent.
//!
//! All public I/O is plain `String` (FEN in, UCI out) so callers never need
//! `shakmaty` types in their JSON-schema'd function signatures. Every routine
//! is a pure function of its inputs; the negamax opponent is deterministic
//! given `(fen, depth)` via an explicit total order over candidate moves.

use anyhow::{anyhow, Result};
use shakmaty::fen::Fen;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Color, EnPassantMode, Position, Role};

/// Outcome of applying one legal move, described in caller-friendly strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    pub new_fen: String,
    pub is_check: bool,
    pub is_checkmate: bool,
    pub is_stalemate: bool,
    pub is_draw: bool,
    /// `"white"`, `"black"`, `"draw"`, or `None` while the game continues.
    pub result: Option<String>,
}

fn parse_position(fen: &str) -> Result<Chess> {
    let parsed: Fen = fen
        .parse()
        .map_err(|error| anyhow!("invalid FEN `{fen}`: {error}"))?;
    parsed
        .into_position(CastlingMode::Standard)
        .map_err(|error| anyhow!("illegal position `{fen}`: {error}"))
}

fn position_fen(position: &Chess) -> String {
    Fen::from_position(position, EnPassantMode::Legal).to_string()
}

fn move_to_uci(mv: &shakmaty::Move) -> String {
    mv.to_uci(CastlingMode::Standard).to_string()
}

/// Terminal result of a position, if any, as a caller-facing string.
fn result_string(position: &Chess) -> Option<String> {
    match position.outcome() {
        shakmaty::Outcome::Known(known) => Some(match known.winner() {
            Some(Color::White) => "white".to_string(),
            Some(Color::Black) => "black".to_string(),
            None => "draw".to_string(),
        }),
        shakmaty::Outcome::Unknown => None,
    }
}

/// Legal moves from `fen`, as UCI strings in a stable total order.
pub fn legal_moves(fen: &str) -> Result<Vec<String>> {
    let position = parse_position(fen)?;
    let mut moves: Vec<String> = position.legal_moves().iter().map(move_to_uci).collect();
    moves.sort();
    Ok(moves)
}

/// Apply one UCI move to `fen`. Errors if the move is illegal or malformed —
/// the caller distinguishes an illegal attempt from a transport failure.
pub fn apply_move(fen: &str, uci: &str) -> Result<MoveOutcome> {
    let position = parse_position(fen)?;
    let parsed: UciMove = uci
        .parse()
        .map_err(|error| anyhow!("invalid UCI `{uci}`: {error}"))?;
    let mv = parsed
        .to_move(&position)
        .map_err(|error| anyhow!("illegal move `{uci}` in `{fen}`: {error}"))?;
    let next = position
        .play(mv)
        .map_err(|error| anyhow!("could not play `{uci}`: {error}"))?;
    Ok(MoveOutcome {
        new_fen: position_fen(&next),
        is_check: next.is_check(),
        is_checkmate: next.is_checkmate(),
        is_stalemate: next.is_stalemate(),
        is_draw: next.is_insufficient_material()
            || matches!(result_string(&next).as_deref(), Some("draw")),
        result: result_string(&next),
    })
}

/// Node count of the move tree from `fen` at `depth` — the exact perft value.
pub fn perft(fen: &str, depth: u32) -> Result<u64> {
    let position = parse_position(fen)?;
    Ok(shakmaty::perft(&position, depth))
}

/// Centipawn value of a role, from White's perspective magnitude.
fn role_value(role: Role) -> i32 {
    match role {
        Role::Pawn => 100,
        Role::Knight => 320,
        Role::Bishop => 330,
        Role::Rook => 500,
        Role::Queen => 900,
        Role::King => 0,
    }
}

/// Static evaluation from the side-to-move's perspective: material plus a
/// small mobility term. Deterministic and side-relative so negamax can negate
/// cleanly.
fn evaluate(position: &Chess) -> i32 {
    let board = position.board();
    let mut material = 0i32;
    for (square, piece) in board.clone().into_iter() {
        let _ = square;
        let value = role_value(piece.role);
        if piece.color == Color::White {
            material += value;
        } else {
            material -= value;
        }
    }
    // Mobility: legal-move count for the side to move (cheap, deterministic).
    let mobility = position.legal_moves().len() as i32;
    let perspective = if position.turn() == Color::White {
        1
    } else {
        -1
    };
    material * perspective + mobility
}

/// Negamax with a fixed depth. Returns the side-to-move's best score.
fn negamax(position: &Chess, depth: u32) -> i32 {
    if depth == 0 || position.outcome() != shakmaty::Outcome::Unknown {
        return terminal_or_eval(position, depth);
    }
    let mut best = i32::MIN + 1;
    for mv in ordered_moves(position) {
        let next = position.clone().play(mv).expect("legal move replays");
        let score = -negamax(&next, depth - 1);
        if score > best {
            best = score;
        }
    }
    best
}

/// Score a leaf: checkmate/stalemate handled from the side-to-move's view,
/// otherwise the static evaluation. Mate scores fold in remaining depth so a
/// faster mate is preferred.
fn terminal_or_eval(position: &Chess, depth: u32) -> i32 {
    if position.is_checkmate() {
        // Side to move is mated: worst possible, sooner is worse for us.
        return -30_000 - depth as i32;
    }
    if position.is_stalemate() || position.outcome() != shakmaty::Outcome::Unknown {
        return 0;
    }
    evaluate(position)
}

/// Legal moves in a stable total order (by UCI string) so equal-scored
/// searches always resolve identically.
fn ordered_moves(position: &Chess) -> Vec<shakmaty::Move> {
    let mut moves: Vec<shakmaty::Move> = position.legal_moves().to_vec();
    moves.sort_by_key(move_to_uci);
    moves
}

/// The deterministic best move for the side to move at `depth`, as UCI.
/// `None` only when there are no legal moves (terminal position).
pub fn negamax_best_move(fen: &str, depth: u32) -> Option<String> {
    let position = parse_position(fen).ok()?;
    let mut best_move: Option<shakmaty::Move> = None;
    let mut best_score = i32::MIN + 1;
    for mv in ordered_moves(&position) {
        let next = position.clone().play(mv).ok()?;
        let score = -negamax(&next, depth.saturating_sub(1));
        if best_move.is_none() || score > best_score {
            best_score = score;
            best_move = Some(mv);
        }
    }
    best_move.map(|mv| move_to_uci(&mv))
}

pub const STARTPOS: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[cfg(test)]
mod tests {
    use super::*;

    const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

    #[test]
    fn perft_matches_the_canonical_node_counts() {
        assert_eq!(perft(STARTPOS, 1).unwrap(), 20);
        assert_eq!(perft(STARTPOS, 2).unwrap(), 400);
        assert_eq!(perft(STARTPOS, 3).unwrap(), 8_902);
        assert_eq!(perft(STARTPOS, 4).unwrap(), 197_281);
    }

    #[test]
    fn perft_matches_kiwipete() {
        assert_eq!(perft(KIWIPETE, 1).unwrap(), 48);
        assert_eq!(perft(KIWIPETE, 2).unwrap(), 2_039);
    }

    #[test]
    fn legal_moves_are_sorted_and_complete() {
        let moves = legal_moves(STARTPOS).unwrap();
        assert_eq!(moves.len(), 20);
        let mut sorted = moves.clone();
        sorted.sort();
        assert_eq!(moves, sorted);
        assert!(moves.contains(&"e2e4".to_string()));
        assert!(moves.contains(&"g1f3".to_string()));
    }

    #[test]
    fn legal_moves_handle_en_passant_and_promotion() {
        // White pawn on e5, black just played d7d5 -> e5xd6 e.p. available.
        let ep = "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3";
        assert!(legal_moves(ep).unwrap().contains(&"e5d6".to_string()));
        // White pawn on a7 promotes.
        let promo = "8/P7/8/8/8/8/8/k1K5 w - - 0 1";
        let moves = legal_moves(promo).unwrap();
        assert!(moves.contains(&"a7a8q".to_string()));
        assert!(moves.contains(&"a7a8n".to_string()));
    }

    #[test]
    fn apply_move_reports_checkmate() {
        // Fool's mate: after 1.f3 e5 2.g4 Qh4#.
        let before = "rnbqkbnr/pppp1ppp/8/4p3/6P1/5P2/PPPPP2P/RNBQKBNR b KQkq g3 0 2";
        let outcome = apply_move(before, "d8h4").unwrap();
        assert!(outcome.is_checkmate);
        assert_eq!(outcome.result.as_deref(), Some("black"));
    }

    #[test]
    fn apply_move_rejects_an_illegal_move() {
        assert!(apply_move(STARTPOS, "e2e5").is_err());
    }

    #[test]
    fn negamax_is_deterministic() {
        let first = negamax_best_move(STARTPOS, 3);
        let again = negamax_best_move(STARTPOS, 3);
        assert!(first.is_some());
        assert_eq!(first, again);
    }

    #[test]
    fn negamax_finds_mate_in_one() {
        // White to move: Qh5-f7 is mate (scholar's-mate finish).
        let fen = "r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 4";
        // Above is already checkmate; instead use a clean mate-in-1 for White:
        let mate_in_one = "6k1/5ppp/8/8/8/8/8/R6K w - - 0 1";
        let best = negamax_best_move(mate_in_one, 2).unwrap();
        assert!(apply_move(mate_in_one, &best).unwrap().is_checkmate);
        let _ = fen;
    }
}
