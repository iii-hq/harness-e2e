//! A dataset with planted defects and three stated rules. The runner applies
//! the same rules to the same bytes and compares the findings exactly.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.anomaly_report";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "anomaly_report_artifact";
const READINGS_FILE: &str = "data/readings.json";
const REPORT_FILE: &str = "data/anomalies.json";
const MINIMUM_CELSIUS: f64 = -40.0;
const MAXIMUM_CELSIUS: f64 = 85.0;
const READINGS: [(u64, &str, f64); 12] = [
    (1, "s-a", 21.5),
    (2, "s-b", 19.0),
    (3, "s-c", 120.4),
    (4, "s-d", 22.1),
    (5, "", 20.0),
    (6, "s-f", 18.7),
    (7, "s-g", 23.3),
    (7, "s-g", 23.9),
    (8, "s-h", -55.2),
    (9, "s-i", 25.0),
    (10, "s-j", 24.4),
    (11, "s-k", 20.2),
];

const REPORT_PARSES: AssessmentSpec = AssessmentSpec::hard_gated(
    "report_parses",
    15,
    "The report is JSON in the requested shape.",
);
const FINDINGS_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "findings_exact",
    45,
    "The reported anomalies and their reasons match the rules applied to the data.",
);
const COVERAGE_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "coverage_reported",
    15,
    "The report states that every record was checked.",
);
const SUMMARY_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "summary_reported",
    25,
    "The response reports the anomaly and record counts.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    REPORT_PARSES,
    FINDINGS_EXACT,
    COVERAGE_REPORTED,
    SUMMARY_REPORTED,
];

fn readings() -> Vec<Value> {
    READINGS
        .iter()
        .map(|(id, sensor, celsius)| json!({ "id": id, "sensor": sensor, "celsius": celsius }))
        .collect()
}

fn expected_findings() -> Vec<Value> {
    let duplicated: Vec<u64> = READINGS
        .iter()
        .filter(|(id, _, _)| READINGS.iter().filter(|(other, _, _)| other == id).count() > 1)
        .map(|(id, _, _)| *id)
        .collect();
    READINGS
        .iter()
        .enumerate()
        .filter_map(|(position, (id, sensor, celsius))| {
            let index = position + 1;
            let reason = if sensor.trim().is_empty() {
                "missing_sensor"
            } else if duplicated.contains(id) {
                "duplicate_id"
            } else if *celsius < MINIMUM_CELSIUS || *celsius > MAXIMUM_CELSIUS {
                "out_of_range"
            } else {
                return None;
            };
            Some(json!({ "index": index, "reason": reason }))
        })
        .collect()
}

fn setup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let root = workspace::root(ID, run_id);
        workspace::write(
            &root,
            READINGS_FILE,
            &serde_json::to_string_pretty(&json!({ "readings": readings() }))?,
        )
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Audit a sensor dataset in this workspace.\n\n\
             `{READINGS_FILE}` holds {{\"readings\": [...]}}. Each reading has `id`, `sensor`, \
             and `celsius`.\n\n\
             A reading is an anomaly when, in this order of precedence:\n\
             1. its `sensor` is empty or missing: reason `missing_sensor`;\n\
             2. its `id` appears on more than one reading: reason `duplicate_id`, and every \
             reading sharing that id is an anomaly;\n\
             3. its `celsius` is below {MINIMUM_CELSIUS} or above {MAXIMUM_CELSIUS}: reason \
             `out_of_range`.\n\n\
             1. Check every reading. Refer to a reading by its 1-based position in the array, \
             not by its id.\n\
             2. Write `{REPORT_FILE}` in the shape {{\"checked\": <how many readings you \
             examined>, \"anomalies\": [{{\"index\": <position>, \"reason\": <reason>}}]}}, \
             sorted by index ascending.\n\
             3. Reply with exactly one line: `ANOMALIES:<n> CHECKED:{}`.",
            READINGS.len()
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(18, 240_000, 900),
        assessments: ASSESSMENTS,
        setup: Some(setup),
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
            "readings_file": READINGS_FILE,
            "report_file": REPORT_FILE,
            "records": READINGS.len(),
            "expected_anomalies": expected_findings(),
        }),
        super::build_profile(1, 4),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["anomalies", "checked", "response"],
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

fn report(run_id: &str) -> Value {
    workspace::read_json(&workspace::root(ID, run_id), REPORT_FILE).unwrap_or(Value::Null)
}

fn observed_findings(report: &Value) -> Vec<Value> {
    let mut findings: Vec<Value> = report
        .get("anomalies")
        .and_then(Value::as_array)
        .map(|anomalies| {
            anomalies
                .iter()
                .map(|anomaly| {
                    json!({
                        "index": anomaly.get("index").and_then(Value::as_u64).unwrap_or_default(),
                        "reason": anomaly.get("reason").and_then(Value::as_str).unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    findings.sort_by_key(|finding| finding["index"].as_u64().unwrap_or_default());
    findings
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let report = report(run_id);
        let findings = observed_findings(&report);
        let expected = expected_findings();
        let checked = report.get("checked").and_then(Value::as_u64);
        let summary = format!("ANOMALIES:{} CHECKED:{}", expected.len(), READINGS.len());

        Ok(assessment::build_evaluation([
            REPORT_PARSES.full_or_zero(
                report.get("anomalies").and_then(Value::as_array).is_some(),
                format!("`{REPORT_FILE}` parsed: {}", report.is_object()),
            ),
            FINDINGS_EXACT.full_or_zero(
                findings == expected,
                format!("observed findings {findings:?}, expected {expected:?}"),
            ),
            COVERAGE_REPORTED.full_or_zero(
                checked == Some(READINGS.len() as u64),
                format!("observed checked={checked:?}, expected {}", READINGS.len()),
            ),
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
        let report = report(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "anomalies": observed_findings(&report),
                "checked": report.get("checked").cloned().unwrap_or(Value::Null),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_anomaly_report_before_cleanup",
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
    fn the_planted_defects_are_the_expected_findings() {
        assert_eq!(
            expected_findings(),
            vec![
                json!({ "index": 3, "reason": "out_of_range" }),
                json!({ "index": 5, "reason": "missing_sensor" }),
                json!({ "index": 7, "reason": "duplicate_id" }),
                json!({ "index": 8, "reason": "duplicate_id" }),
                json!({ "index": 9, "reason": "out_of_range" }),
            ]
        );
    }

    #[test]
    fn findings_are_compared_regardless_of_report_order() {
        let report = json!({
            "anomalies": [
                { "index": 9, "reason": "out_of_range" },
                { "index": 3, "reason": "out_of_range" },
                { "index": 8, "reason": "duplicate_id" },
                { "index": 5, "reason": "missing_sensor" },
                { "index": 7, "reason": "duplicate_id" }
            ]
        });
        assert_eq!(observed_findings(&report), expected_findings());
    }
}
