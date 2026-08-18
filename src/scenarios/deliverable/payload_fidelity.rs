//! A large payload is handed to the agent and has to come back whole. There
//! is nothing to work out: every byte is in the prompt. What is measured is
//! whether the stack carries a big structured answer through the output
//! budget, the transcript, the workspace, and the capture pipeline without
//! losing or reshaping it.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.payload_fidelity";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "payload_fidelity_artifact";
const PAYLOAD_FILE: &str = "payload/rows.json";
const ROWS: usize = 14;
const COLUMNS: usize = 14;

const SHAPE_INTACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "shape_intact",
    30,
    "Every row and column arrived: nothing was truncated, elided, or summarised.",
);
const PAYLOAD_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "payload_exact",
    55,
    "The stored payload equals the payload the prompt carried, cell for cell.",
);
const CHECKSUM_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "checksum_reported",
    15,
    "The reported cell total matches the payload, which a shortened answer cannot do.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[SHAPE_INTACT, PAYLOAD_EXACT, CHECKSUM_REPORTED];

/// The payload is a fixed function of the row and column, so the prompt, the
/// expectation, and the evidence all agree without a stored fixture.
fn payload() -> Vec<Vec<u64>> {
    (0..ROWS)
        .map(|row| {
            (0..COLUMNS)
                .map(|column| ((row * 7 + column * 3) % 10) as u64)
                .collect()
        })
        .collect()
}

fn total(rows: &[Vec<u64>]) -> u64 {
    rows.iter().flatten().sum()
}

fn render(rows: &[Vec<u64>]) -> String {
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row.iter().map(u64::to_string).collect();
            format!("  [{}]", cells.join(", "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn observed(run_id: &str) -> Option<Vec<Vec<u64>>> {
    let value = workspace::read_json(&workspace::root(ID, run_id), PAYLOAD_FILE)?;
    let rows = value.get("rows")?.as_array()?;
    rows.iter()
        .map(|row| {
            row.as_array()?
                .iter()
                .map(Value::as_u64)
                .collect::<Option<Vec<u64>>>()
        })
        .collect()
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let payload = payload();
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Store a payload exactly as given. Nothing here needs working out.\n\n\
             The payload is {ROWS} rows of {COLUMNS} integers:\n\n{}\n\n\
             1. Write `{PAYLOAD_FILE}` containing exactly {{\"rows\": <the payload>}}, with every \
             row and every value as given, in the same order.\n\
             2. Do not abbreviate, elide, summarise, or generate the rows programmatically: copy \
             them.\n\
             3. Reply with exactly one line: `ROWS:{ROWS} CELLS:{} TOTAL:{}`.",
            render(&payload),
            ROWS * COLUMNS,
            total(&payload),
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(14, 320_000, 900),
        assessments: ASSESSMENTS,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let payload = payload();
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "payload_file": PAYLOAD_FILE,
            "rows": ROWS,
            "columns": COLUMNS,
            "total": total(&payload),
        }),
        super::build_profile(1, 1),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["rows", "cells", "response"],
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

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let expected = payload();
        let stored = observed(run_id);
        let shape_intact = stored
            .as_ref()
            .is_some_and(|rows| rows.len() == ROWS && rows.iter().all(|row| row.len() == COLUMNS));
        let summary = format!(
            "ROWS:{ROWS} CELLS:{} TOTAL:{}",
            ROWS * COLUMNS,
            total(&expected)
        );

        Ok(assessment::build_evaluation([
            SHAPE_INTACT.full_or_zero(
                shape_intact,
                format!(
                    "expected {ROWS}x{COLUMNS}, observed {:?}",
                    stored
                        .as_ref()
                        .map(|rows| (rows.len(), rows.first().map_or(0, Vec::len)))
                ),
            ),
            PAYLOAD_EXACT.full_or_zero(
                stored.as_ref() == Some(&expected),
                format!(
                    "stored payload matches the prompt: {}",
                    stored.as_ref() == Some(&expected)
                ),
            ),
            CHECKSUM_REPORTED.full_or_zero(
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
        let stored = observed(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "rows": stored.as_ref().map_or(0, Vec::len),
                "cells": stored.as_ref().map_or(0, |rows| rows.iter().map(Vec::len).sum()),
                "total": stored.as_ref().map(|rows| total(rows)),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_payload_before_cleanup",
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
    fn the_payload_is_the_declared_shape_and_total() {
        let payload = payload();
        assert_eq!(payload.len(), ROWS);
        assert!(payload.iter().all(|row| row.len() == COLUMNS));
        assert_eq!(total(&payload), payload.iter().flatten().sum::<u64>());
    }

    #[test]
    fn every_value_stays_a_single_digit() {
        assert!(payload().iter().flatten().all(|cell| *cell < 10));
    }
}
