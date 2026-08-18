//! A bar chart whose geometry is arithmetic, not taste. Every rectangle's
//! position and size is recomputed from the data and compared exactly.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.svg_chart";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "svg_chart_artifact";
const CHART_FILE: &str = "chart/bars.svg";
const VALUES: [i64; 5] = [3, 7, 2, 9, 5];
const BAR_WIDTH: i64 = 20;
const BAR_GAP: i64 = 10;
const UNIT: i64 = 10;
const CHART_HEIGHT: i64 = 100;

const CHART_PARSES: AssessmentSpec = AssessmentSpec::hard_gated(
    "chart_parses",
    15,
    "The file is an SVG sized to the declared viewport.",
);
const BAR_COUNT_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "bar_count_exact",
    20,
    "The chart draws one rectangle per data point.",
);
const GEOMETRY_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "geometry_exact",
    50,
    "Every bar has the x, y, width, and height the data produces.",
);
const SUMMARY_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "summary_reported",
    15,
    "The response reports the bar count and the tallest bar.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    CHART_PARSES,
    BAR_COUNT_EXACT,
    GEOMETRY_EXACT,
    SUMMARY_REPORTED,
];

fn chart_width() -> i64 {
    i64::try_from(VALUES.len()).unwrap_or_default() * (BAR_WIDTH + BAR_GAP) - BAR_GAP
}

fn expected_bars() -> Vec<Value> {
    VALUES
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let height = value * UNIT;
            json!({
                "x": i64::try_from(index).unwrap_or_default() * (BAR_WIDTH + BAR_GAP),
                "y": CHART_HEIGHT - height,
                "width": BAR_WIDTH,
                "height": height,
            })
        })
        .collect()
}

fn observed_bars(source: &str) -> Vec<Value> {
    workspace::elements(source, "rect")
        .iter()
        .map(|element| {
            let attribute = |name: &str| {
                workspace::attribute(element, name)
                    .and_then(|value| value.trim().parse::<f64>().ok())
                    .map(|value| value.round() as i64)
            };
            json!({
                "x": attribute("x").unwrap_or(i64::MIN),
                "y": attribute("y").unwrap_or(i64::MIN),
                "width": attribute("width").unwrap_or(i64::MIN),
                "height": attribute("height").unwrap_or(i64::MIN),
            })
        })
        .collect()
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let values = VALUES
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Draw a bar chart as SVG in this workspace.\n\n\
             The data is: {values}.\n\n\
             1. Write `{CHART_FILE}`, an `<svg>` with `width=\"{width}\"` and \
             `height=\"{CHART_HEIGHT}\"`.\n\
             2. Draw one `<rect>` per value, in data order, with these attributes: `width` is \
             {BAR_WIDTH}; `height` is the value times {UNIT}; `x` is the zero-based index times \
             {step}; `y` is {CHART_HEIGHT} minus the height. Use plain integers with no units.\n\
             3. Add no other `<rect>` elements. Axes or labels, if you add any, must use other \
             elements.\n\
             4. Reply with exactly one line: `BARS:{count} MAX_HEIGHT:{max}`.",
            width = chart_width(),
            step = BAR_WIDTH + BAR_GAP,
            count = VALUES.len(),
            max = VALUES.iter().copied().max().unwrap_or_default() * UNIT,
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(16, 200_000, 360),
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
            "chart_file": CHART_FILE,
            "values": VALUES,
            "bars": expected_bars(),
            "viewport": { "width": chart_width(), "height": CHART_HEIGHT },
        }),
        super::build_profile(1, 2),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["bars", "response"],
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
        let source = workspace::read(&workspace::root(ID, run_id), CHART_FILE).unwrap_or_default();
        let bars = observed_bars(&source);
        let expected = expected_bars();
        let viewport = source.contains(&format!("width=\"{}\"", chart_width()))
            && source.contains(&format!("height=\"{CHART_HEIGHT}\""));
        let summary = format!(
            "BARS:{} MAX_HEIGHT:{}",
            VALUES.len(),
            VALUES.iter().copied().max().unwrap_or_default() * UNIT
        );

        Ok(assessment::build_evaluation([
            CHART_PARSES.full_or_zero(
                source.contains("<svg") && viewport,
                format!(
                    "svg element present: {}; viewport declared: {viewport}",
                    source.contains("<svg")
                ),
            ),
            BAR_COUNT_EXACT.full_or_zero(
                bars.len() == VALUES.len(),
                format!("observed {} bar(s), expected {}", bars.len(), VALUES.len()),
            ),
            GEOMETRY_EXACT.full_or_zero(bars == expected, format!("observed bars {bars:?}")),
            SUMMARY_REPORTED.full_or_zero(
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
        let source = workspace::read(&workspace::root(ID, run_id), CHART_FILE).unwrap_or_default();
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "bars": observed_bars(&source),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_svg_chart_before_cleanup",
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
    fn bars_are_read_back_from_the_svg_they_describe() {
        let source = expected_bars()
            .iter()
            .map(|bar| {
                format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" />",
                    bar["x"], bar["y"], bar["width"], bar["height"]
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(observed_bars(&source), expected_bars());
    }

    #[test]
    fn a_missing_attribute_never_matches_the_expected_geometry() {
        assert_ne!(
            observed_bars("<rect x=\"0\" y=\"70\" width=\"20\" />"),
            expected_bars()
        );
    }
}
