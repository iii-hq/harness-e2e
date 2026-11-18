//! Pure scoring: one provider reply per case into decisions, the slice's
//! measurements, and the two criteria.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::cases::{Case, Gold, Slice};
use crate::report::EvaluationDimension;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::memory::score::Measurement;
use crate::scenarios::ObjectiveEvaluation;

pub const SCHEMA: &str = "judge-quality/v1";
/// A distribution may be off by this much from 1 (the judge contract's bound).
const SUM_TOLERANCE: f64 = 0.02;
/// The Harness acts on a judge answer only at this probability.
const HARNESS_THRESHOLD: f64 = 0.8;

/// What one request returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseRecord {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    pub ms: f64,
    /// The reply's error code, or `invalid_response` when an answer is unusable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One question of one case. `model`, `input_tokens` and `ms` repeat the
/// case's request, so a paired report can split decisions by routed
/// checkpoint and by input size without joining.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// `<case>/<question id>`.
    pub id: String,
    pub kind: String,
    pub answered: bool,
    pub correct: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p_gold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p_top: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_yes: Option<bool>,
    /// The `noul` the provider returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noul: Option<f64>,
    /// The `choice` label the provider returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    /// The case's group (JevBench paraphrases of one scenario share one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abs_error: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<BTreeMap<String, f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    pub ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub schema: String,
    pub provider: String,
    pub version: String,
    /// The provider worker's stored configuration: which defaults the arm ran.
    pub config: Value,
    /// `judge-<provider>::models::list` cards.
    pub cards: Value,
    pub slice: String,
    pub dataset: Value,
    pub cases: Vec<CaseRecord>,
    pub decisions: Vec<Decision>,
    pub errors: BTreeMap<String, usize>,
    pub measurements: Vec<Measurement>,
}

/// The slice as `case.inputs` and the evidence state it.
pub fn dataset(slice: &Slice) -> Value {
    json!({
        "dataset": slice.dataset,
        "revision": slice.revision,
        "sha256": slice.sha256,
        "cases": slice.cases,
        "decisions": slice.decisions,
    })
}

impl Evidence {
    /// The slice's evidence from its records and decisions. The provider,
    /// its version, configuration and cards are the caller's to fill in.
    pub fn new(
        slice: &Slice,
        cases: &[Case],
        records: Vec<CaseRecord>,
        decisions: Vec<Decision>,
    ) -> Self {
        let mut errors = BTreeMap::new();
        for code in records.iter().filter_map(|record| record.error.clone()) {
            *errors.entry(code).or_insert(0) += 1;
        }
        Self {
            schema: SCHEMA.to_string(),
            provider: String::new(),
            version: String::new(),
            config: Value::Null,
            cards: Value::Null,
            slice: slice.id.to_string(),
            dataset: dataset(slice),
            measurements: measurements(cases, &records, &decisions),
            cases: records,
            decisions,
            errors,
        }
    }
}

/// One answer checked against its gold: `None` when it is not a valid answer
/// to the question.
struct Checked {
    correct: bool,
    p_gold: f64,
    p_top: f64,
    noul: Option<f64>,
    choice: Option<String>,
    abs_error: Option<f64>,
    probabilities: Option<BTreeMap<String, f64>>,
}

/// The answer's probabilities, when every key is an outcome of the question
/// and they sum to 1 within the tolerance.
fn distribution(
    answer: &Map<String, Value>,
    outcome: impl Fn(&str) -> bool,
) -> Option<BTreeMap<String, f64>> {
    let probabilities: BTreeMap<String, f64> = answer
        .get("probabilities")?
        .as_object()?
        .iter()
        .map(|(key, value)| Some((key.clone(), value.as_f64()?)))
        .collect::<Option<_>>()?;
    let valid = probabilities.keys().all(|key| outcome(key))
        && probabilities
            .values()
            .all(|p| p.is_finite() && (0.0..=1.0).contains(p))
        && (1.0 - SUM_TOLERANCE..=1.0 + SUM_TOLERANCE)
            .contains(&probabilities.values().sum::<f64>());
    valid.then_some(probabilities)
}

fn top(probabilities: &BTreeMap<String, f64>) -> f64 {
    probabilities.values().copied().fold(0.0, f64::max)
}

fn check(question: &Value, gold: &Gold, answer: &Value) -> Option<Checked> {
    let answer = answer.as_object()?;
    if answer.get("type") != question.get("type") {
        return None;
    }
    match gold {
        Gold::Noul(yes) => {
            let noul = answer
                .get("noul")?
                .as_f64()
                .filter(|p| (0.0..=1.0).contains(p))?;
            let p_gold = if *yes { noul } else { 1.0 - noul };
            Some(Checked {
                correct: (noul >= 0.5) == *yes,
                p_gold,
                p_top: noul.max(1.0 - noul),
                noul: Some(noul),
                choice: None,
                abs_error: None,
                probabilities: None,
            })
        }
        Gold::Choice(label) => {
            let criteria = question["criteria"].as_object()?;
            let choice = answer
                .get("choice")?
                .as_str()
                .filter(|choice| criteria.contains_key(*choice))?;
            let probabilities = distribution(answer, |key| criteria.contains_key(key))?;
            Some(Checked {
                correct: choice == label,
                p_gold: *probabilities.get(label)?,
                p_top: top(&probabilities),
                noul: None,
                choice: Some(choice.to_string()),
                abs_error: None,
                probabilities: Some(probabilities),
            })
        }
        Gold::Score(level) => {
            let levels = question["criteria"].as_array()?.len();
            let score = answer
                .get("score")?
                .as_f64()
                .filter(|score| (0.0..=levels.saturating_sub(1) as f64).contains(score))?;
            let probabilities = distribution(answer, |key| {
                key.parse::<usize>()
                    .is_ok_and(|level| level < levels && level.to_string() == key)
            })?;
            // The first level on a tie.
            let argmax = (0..levels)
                .filter_map(|index| Some((index, *probabilities.get(&index.to_string())?)))
                .fold(None, |best: Option<(usize, f64)>, (index, p)| match best {
                    Some((_, best_p)) if best_p >= p => best,
                    _ => Some((index, p)),
                })?
                .0;
            Some(Checked {
                correct: argmax == *level,
                p_gold: *probabilities.get(&level.to_string())?,
                p_top: top(&probabilities),
                noul: None,
                choice: None,
                abs_error: Some((score - *level as f64).abs()),
                probabilities: Some(probabilities),
            })
        }
    }
}

/// The case's record and one decision per question, from the provider's
/// reply to a request holding that case alone, sent in `ms`.
pub fn decide(case: &Case, reply: &Value, ms: f64) -> (CaseRecord, Vec<Decision>) {
    let ok = reply["status"] == "ok";
    let result = &reply["results"][&case.id];
    let model = ok
        .then(|| reply["model"].as_str().map(str::to_string))
        .flatten();
    let input_tokens = result["usage"]["input_tokens"].as_u64();
    let decisions: Vec<Decision> = case
        .questions
        .iter()
        .map(|(qid, question)| {
            let gold = &case.gold[qid];
            let checked = ok
                .then(|| check(question, gold, &result["answers"][qid]))
                .flatten();
            Decision {
                id: format!("{}/{qid}", case.id),
                kind: question["type"].as_str().unwrap_or_default().to_string(),
                answered: checked.is_some(),
                correct: checked.as_ref().is_some_and(|checked| checked.correct),
                agreed: case.agree.get(qid).copied(),
                p_gold: checked.as_ref().map(|checked| checked.p_gold),
                p_top: checked.as_ref().map(|checked| checked.p_top),
                gold_yes: match gold {
                    Gold::Noul(yes) => Some(*yes),
                    _ => None,
                },
                noul: checked.as_ref().and_then(|checked| checked.noul),
                choice: checked.as_ref().and_then(|checked| checked.choice.clone()),
                group: case.group.clone(),
                abs_error: checked.as_ref().and_then(|checked| checked.abs_error),
                probabilities: checked.and_then(|checked| checked.probabilities),
                model: model.clone(),
                input_tokens,
                ms,
            }
        })
        .collect();
    let error = if !ok {
        Some(
            reply["code"]
                .as_str()
                .unwrap_or("invalid_response")
                .to_string(),
        )
    } else if decisions.iter().any(|decision| !decision.answered) {
        Some("invalid_response".to_string())
    } else {
        None
    };
    let record = CaseRecord {
        id: case.id.clone(),
        model,
        input_tokens,
        ms,
        error,
    };
    (record, decisions)
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    numerator as f64 / denominator as f64
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0usize), |(sum, count), value| {
        (sum + value, count + 1)
    });
    (count > 0).then(|| sum / count as f64)
}

/// Nearest rank.
fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// 10 equal-width bins on `p_top`.
fn expected_calibration_error(decisions: &[&Decision]) -> f64 {
    let mut bins = [(0.0, 0.0); 10];
    for decision in decisions {
        let p = decision.p_top.unwrap_or_default();
        let bin = &mut bins[((p * 10.0) as usize).min(9)];
        bin.0 += f64::from(u8::from(decision.correct));
        bin.1 += p;
    }
    bins.iter()
        .map(|(correct, confidence)| (correct - confidence).abs())
        .sum::<f64>()
        / decisions.len() as f64
}

/// Mann-Whitney over the returned `noul`, ties counted half.
fn auroc(decisions: &[&Decision]) -> Option<f64> {
    let noul = |decision: &&Decision| decision.noul.unwrap_or_default();
    let (yes, no): (Vec<&Decision>, Vec<&Decision>) = decisions
        .iter()
        .partition(|decision| decision.gold_yes == Some(true));
    if yes.is_empty() || no.is_empty() {
        return None;
    }
    let mut wins = 0.0;
    for positive in yes.iter().map(noul) {
        for negative in no.iter().map(noul) {
            wins += if positive > negative {
                1.0
            } else if positive == negative {
                0.5
            } else {
                0.0
            };
        }
    }
    Some(wins / (yes.len() * no.len()) as f64)
}

/// Always answering the commonest gold label of each (workflow, question id).
fn majority_accuracy(cases: &[Case]) -> Option<f64> {
    let mut labels: BTreeMap<(&str, &str), BTreeMap<String, usize>> = BTreeMap::new();
    let mut total = 0;
    for case in cases {
        let workflow = case.workflow.as_deref()?;
        for (qid, gold) in &case.gold {
            *labels
                .entry((workflow, qid))
                .or_default()
                .entry(format!("{gold:?}"))
                .or_insert(0) += 1;
            total += 1;
        }
    }
    let hits: usize = labels
        .values()
        .map(|counts| counts.values().copied().max().unwrap_or_default())
        .sum();
    (total > 0).then(|| ratio(hits, total))
}

pub fn measurements(
    cases: &[Case],
    records: &[CaseRecord],
    decisions: &[Decision],
) -> Vec<Measurement> {
    let mut out = Vec::new();
    let mut push = |id: &str, value: f64, unit: &str| {
        out.push(Measurement {
            id: id.to_string(),
            value,
            unit: unit.to_string(),
        })
    };
    fn correct(subset: &[&Decision]) -> usize {
        subset.iter().filter(|d| d.correct).count()
    }
    fn answered<'a>(subset: &[&'a Decision]) -> Vec<&'a Decision> {
        subset.iter().copied().filter(|d| d.answered).collect()
    }
    let all: Vec<&Decision> = decisions.iter().collect();
    push("cases", records.len() as f64, "count");
    push("decisions", all.len() as f64, "count");
    push("answered", answered(&all).len() as f64, "count");
    if !all.is_empty() {
        push("accuracy", ratio(correct(&all), all.len()), "ratio");
    }
    if let Some(majority) = majority_accuracy(cases) {
        push("majority_accuracy", majority, "ratio");
    }
    if all.iter().any(|d| d.agreed.is_some()) {
        let agreed: Vec<&Decision> = all
            .iter()
            .copied()
            .filter(|d| d.agreed == Some(true))
            .collect();
        push("agreed_n", agreed.len() as f64, "count");
        if !agreed.is_empty() {
            push(
                "accuracy_agreed",
                ratio(correct(&agreed), agreed.len()),
                "ratio",
            );
        }
    }
    let of_kind =
        |kind: &str| -> Vec<&Decision> { all.iter().copied().filter(|d| d.kind == kind).collect() };
    let choice = of_kind("choice");
    if !choice.is_empty() {
        push("choice_n", choice.len() as f64, "count");
        push(
            "choice_top1",
            ratio(correct(&choice), choice.len()),
            "ratio",
        );
        let valid = answered(&choice);
        if let Some(nll) = mean(
            valid
                .iter()
                .map(|d| -d.p_gold.unwrap_or_default().max(1e-9).ln()),
        ) {
            push("choice_nll", nll, "nats");
            push("choice_ece", expected_calibration_error(&valid), "ratio");
        }
    }
    let score = of_kind("score");
    if !score.is_empty() {
        push("score_n", score.len() as f64, "count");
        push(
            "score_accuracy",
            ratio(correct(&score), score.len()),
            "ratio",
        );
        if let Some(mae) = mean(answered(&score).iter().filter_map(|d| d.abs_error)) {
            push("score_mae", mae, "levels");
        }
    }
    let noul = of_kind("noul");
    if !noul.is_empty() {
        let strict = noul
            .iter()
            .filter(|d| d.correct && d.p_gold.is_some_and(|p| p >= HARNESS_THRESHOLD))
            .count();
        push("noul_n", noul.len() as f64, "count");
        push("noul_accuracy", ratio(correct(&noul), noul.len()), "ratio");
        push("noul_accuracy_at_0_8", ratio(strict, noul.len()), "ratio");
        let valid = answered(&noul);
        if let Some(brier) = mean(
            valid
                .iter()
                .map(|d| (1.0 - d.p_gold.unwrap_or_default()).powi(2)),
        ) {
            push("noul_brier", brier, "ratio");
        }
        if let Some(auroc) = auroc(&valid) {
            push("noul_auroc", auroc, "ratio");
        }
    }
    if let Some(tokens) = records.iter().filter_map(|r| r.input_tokens).max() {
        push("input_tokens_max", tokens as f64, "count");
    }
    if !records.is_empty() {
        let mut ms: Vec<f64> = records.iter().map(|r| r.ms).collect();
        ms.sort_by(f64::total_cmp);
        push("case_ms_p50", percentile(&ms, 0.50), "ms");
        push("case_ms_p95", percentile(&ms, 0.95), "ms");
    }
    out
}

pub(in crate::scenarios) fn assessments(slice: &Slice) -> [AssessmentSpec; 2] {
    [
        AssessmentSpec::gate_in(
            "answered",
            10,
            "Every decision of the slice came back as a valid answer to its question.",
            EvaluationDimension::Deliverable,
        )
        .labeled("Every decision came back answered"),
        AssessmentSpec::scored_in(
            "accuracy",
            90,
            slice.accuracy_description,
            EvaluationDimension::Deliverable,
        )
        .labeled("Answers match the gold labels"),
    ]
}

fn counted<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_insert(0) += 1;
    }
    counts
        .iter()
        .map(|(value, count)| format!("{value} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn evaluate(slice: &Slice, evidence: &Evidence) -> ObjectiveEvaluation {
    let [answered_spec, accuracy_spec] = assessments(slice);
    let total = slice.decisions;
    let answered = evidence.decisions.iter().filter(|d| d.answered).count();
    let correct = evidence
        .decisions
        .iter()
        .filter(|d| d.correct)
        .count()
        .min(total);
    let whole = answered == total && evidence.decisions.len() == total;
    let mut answered_reason = format!("{answered} of {total} decisions answered");
    if !evidence.errors.is_empty() {
        let errors = evidence
            .errors
            .iter()
            .map(|(code, count)| format!("{code} {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        answered_reason.push_str(&format!("; cases by error: {errors}"));
    }
    let models = counted(
        evidence
            .cases
            .iter()
            .filter_map(|record| record.model.as_deref()),
    );
    let points = (90.0 * ratio(correct, total)).round() as u8;
    let accuracy = accuracy_spec
        .award(
            points,
            format!(
                "{correct} of {total} correct ({:.3}); models: {}",
                ratio(correct, total),
                if models.is_empty() { "none" } else { &models }
            ),
        )
        .expect("correct never exceeds the slice's decisions");
    assessment::build_evaluation([answered_spec.full_or_zero(whole, answered_reason), accuracy])
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::report::CompletionState;
    use crate::scenarios::judge_quality::cases::{self, SLICES};

    const GOLDEN: &str = include_str!("../../../tests/fixtures/judge-quality/golden.json");

    /// The reply a judge that echoes the gold with probability 1 gives.
    pub(crate) fn reference_reply(case: &Case) -> Value {
        let answers: Map<String, Value> = case
            .questions
            .iter()
            .map(|(qid, question)| {
                let answer = match &case.gold[qid] {
                    Gold::Noul(yes) => json!({"type": "noul", "noul": if *yes { 1.0 } else { 0.0 }}),
                    Gold::Choice(label) => {
                        let probabilities: Map<String, Value> = question["criteria"]
                            .as_object()
                            .unwrap()
                            .keys()
                            .map(|key| (key.clone(), json!(f64::from(u8::from(key == label)))))
                            .collect();
                        json!({"type": "choice", "choice": label, "probabilities": probabilities, "confidence": 1.0})
                    }
                    Gold::Score(level) => {
                        let probabilities: Map<String, Value> = (0..question["criteria"]
                            .as_array()
                            .unwrap()
                            .len())
                            .map(|index| (index.to_string(), json!(f64::from(u8::from(index == *level)))))
                            .collect();
                        json!({"type": "score", "score": *level as f64, "probabilities": probabilities, "confidence": 1.0, "legend": {}})
                    }
                };
                (qid.clone(), answer)
            })
            .collect();
        json!({
            "status": "ok",
            "model": "reference",
            "results": {(case.id.clone()): {"answers": answers, "usage": {"input_tokens": 1}}},
            "stats": {},
        })
    }

    /// A judge that answers nothing.
    pub(crate) fn null_reply(_case: &Case) -> Value {
        json!({"status": "ok", "model": "null", "results": {}, "stats": {}})
    }

    pub(crate) fn evidence_from(
        slice: &Slice,
        cases: &[Case],
        reply: impl Fn(&Case) -> Value,
    ) -> Evidence {
        let mut records = Vec::new();
        let mut decisions = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let (record, decided) = decide(case, &reply(case), (index + 1) as f64 * 100.0);
            records.push(record);
            decisions.extend(decided);
        }
        Evidence {
            provider: "test".to_string(),
            version: "0.0.0".to_string(),
            config: json!({}),
            cards: json!([]),
            ..Evidence::new(slice, cases, records, decisions)
        }
    }

    fn value(evidence: &Evidence, id: &str) -> Option<f64> {
        evidence
            .measurements
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.value)
    }

    fn golden() -> (Vec<Case>, Value) {
        let golden: Value = serde_json::from_str(GOLDEN).unwrap();
        let lines: Vec<String> = golden["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::to_string)
            .collect();
        (cases::parse(&lines.join("\n")).unwrap(), golden)
    }

    /// A slice sized to `decisions`, for the criteria.
    fn sized(decisions: usize) -> Slice {
        Slice {
            decisions,
            ..SLICES[0]
        }
    }

    #[test]
    fn the_golden_replies_give_the_golden_metrics() {
        let (cases, golden) = golden();
        let slice = sized(10);
        let evidence = evidence_from(&slice, &cases, |case| golden["replies"][&case.id].clone());
        let expected = golden["expected"]["measurements"].as_object().unwrap();
        for (id, want) in expected {
            let got = value(&evidence, id).unwrap_or_else(|| panic!("{id} is missing"));
            assert!(
                (got - want.as_f64().unwrap()).abs() < 1e-6,
                "{id}: {got} != {want}"
            );
        }
        assert_eq!(evidence.measurements.len(), expected.len());
        for absent in ["majority_accuracy", "agreed_n", "accuracy_agreed"] {
            assert!(value(&evidence, absent).is_none(), "{absent}");
        }
        assert_eq!(
            serde_json::to_value(&evidence.errors).unwrap(),
            golden["expected"]["errors"]
        );
        let m1 = &evidence.cases[0];
        assert_eq!(
            (m1.model.as_deref(), m1.input_tokens),
            (Some("m"), Some(40))
        );
        let c1 = &evidence.decisions[0];
        assert_eq!(c1.id, "m1/c1");
        assert_eq!(c1.probabilities.as_ref().unwrap().len(), 4);
        assert_eq!(
            (c1.model.as_deref(), c1.input_tokens, c1.ms),
            (Some("m"), Some(40), 100.0)
        );
        let e1 = evidence.decisions.iter().find(|d| d.id == "e1/q").unwrap();
        assert!(!e1.answered && e1.model.is_none() && e1.ms == 800.0);

        let evaluation = evaluate(&slice, &evidence);
        let awarded: BTreeMap<&str, Option<u8>> = evaluation
            .awards
            .iter()
            .map(|award| (award.id.as_str(), award.awarded))
            .collect();
        let want = &golden["expected"]["awards"];
        assert_eq!(
            awarded["answered"],
            want["answered"].as_u64().map(|p| p as u8)
        );
        assert_eq!(
            awarded["accuracy"],
            want["accuracy"].as_u64().map(|p| p as u8)
        );
        assert_eq!(
            assessment::completion(&assessments(&slice), &evaluation),
            CompletionState::TaskIncomplete
        );
        assert!(
            evaluation.awards[1].reason.contains("models: m 8"),
            "{}",
            evaluation.awards[1].reason
        );
    }

    #[test]
    fn agreement_and_majority_are_read_from_the_gold_and_the_flags() {
        let lines: Vec<String> = ["a", "a", "a", "b"]
            .iter()
            .zip([true, true, false, false])
            .enumerate()
            .map(|(index, (gold, agree))| {
                json!({
                    "id": format!("k{index}"),
                    "workflow": "w",
                    "state": "s",
                    "questions": {"q": {"type": "choice", "criteria": {"a": "A", "b": "B"}}},
                    "gold": {"q": gold},
                    "agree": {"q": agree},
                })
                .to_string()
            })
            .collect();
        let cases = cases::parse(&lines.join("\n")).unwrap();
        let evidence = evidence_from(&sized(4), &cases, |case| {
            if case.id == "k0" {
                reference_reply(case)
            } else {
                null_reply(case)
            }
        });
        assert_eq!(value(&evidence, "majority_accuracy"), Some(0.75));
        assert_eq!(value(&evidence, "agreed_n"), Some(2.0));
        assert_eq!(value(&evidence, "accuracy_agreed"), Some(0.5));
    }

    #[test]
    fn the_typed_decisions_majority_baseline_is_pinned() {
        let cases = cases::load(&SLICES[3]).unwrap();
        let majority = majority_accuracy(&cases).unwrap();
        assert!((majority - 0.5225).abs() < 1e-9, "{majority}");
    }

    #[test]
    fn invalid_answers_and_missing_primitives_are_left_out() {
        let case = &cases::parse(
            &json!({"id": "x", "state": "s",
                "questions": {"q": {"type": "choice", "criteria": {"a": "A", "b": "B"}}},
                "gold": {"q": "a"}})
            .to_string(),
        )
        .unwrap()[0];
        let short = json!({"status": "ok", "model": "m", "results": {"x": {"answers": {"q":
            {"type": "choice", "choice": "a", "probabilities": {"a": 0.6, "b": 0.3}, "confidence": 0.2}}}}});
        let (record, decisions) = decide(case, &short, 1.0);
        assert!(!decisions[0].answered);
        assert_eq!(record.error.as_deref(), Some("invalid_response"));

        // Every noul gold is yes: no AUROC. No score question: no score ids.
        let cases = cases::parse(
            &json!({"id": "y", "state": "s", "questions": {"q": {"type": "noul"}}, "gold": {"q": true}})
                .to_string(),
        )
        .unwrap();
        let evidence = evidence_from(&sized(1), &cases, reference_reply);
        assert!(value(&evidence, "noul_auroc").is_none());
        assert!(evidence
            .measurements
            .iter()
            .all(|m| !m.id.starts_with("score_")));
    }

    /// Whether `answer` to `question` (gold `gold`) is answered.
    fn answers(question: Value, gold: Value, answer: Value) -> Option<Decision> {
        let case = &cases::parse(
            &json!({"id": "x", "state": "s", "questions": {"q": question}, "gold": {"q": gold}})
                .to_string(),
        )
        .unwrap()[0];
        let reply =
            json!({"status": "ok", "model": "m", "results": {"x": {"answers": {"q": answer}}}});
        let (_, mut decisions) = decide(case, &reply, 1.0);
        let decision = decisions.remove(0);
        decision.answered.then_some(decision)
    }

    #[test]
    fn an_answer_holds_only_the_question_s_outcomes() {
        let choice = json!({"type": "choice", "criteria": {"A": "a", "B": "b"}});
        let pick = |label: &str, probabilities: Value| {
            answers(
                choice.clone(),
                json!("A"),
                json!({"type": "choice", "choice": label, "probabilities": probabilities}),
            )
        };
        let kept = pick("B", json!({"A": 0.4, "B": 0.6})).unwrap();
        assert!(!kept.correct);
        assert_eq!(kept.choice.as_deref(), Some("B"));
        assert!(pick("C", json!({"A": 0.6, "B": 0.4})).is_none());
        assert!(pick("A", json!({"A": 0.6, "B": 0.2, "C": 0.2})).is_none());
        // The tolerance is inclusive at both ends.
        assert!(pick("A", json!({"A": 0.49, "B": 0.49})).is_some());
        assert!(pick("A", json!({"A": 0.51, "B": 0.51})).is_some());
        assert!(pick("A", json!({"A": 0.48, "B": 0.49})).is_none());
        assert!(pick("A", json!({"A": 0.52, "B": 0.51})).is_none());

        let score = json!({"type": "score", "criteria": ["low", "mid", "high", "top"]});
        let rate = |value: f64, probabilities: Value| {
            answers(
                score.clone(),
                json!(0),
                json!({"type": "score", "score": value, "probabilities": probabilities}),
            )
        };
        let fractional = rate(1.6, json!({"0": 0.1, "1": 0.2, "2": 0.6, "3": 0.1})).unwrap();
        assert!((fractional.abs_error.unwrap() - 1.6).abs() < 1e-12);
        assert!(rate(3.0, json!({"0": 1.0})).is_some());
        assert!(rate(1e308, json!({"0": 1.0})).is_none());
        assert!(rate(-0.5, json!({"0": 1.0})).is_none());
        assert!(rate(0.0, json!({"0": 0.1, "1": 0.1, "4": 0.8})).is_none());
        assert!(rate(0.0, json!({"00": 1.0})).is_none());
    }

    #[test]
    fn auroc_ranks_the_returned_noul() {
        let decisions: Vec<Decision> = [(true, 0.2), (false, 0.2)]
            .iter()
            .map(|(yes, noul)| {
                answers(
                    json!({"type": "noul"}),
                    json!(yes),
                    json!({"type": "noul", "noul": noul}),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(auroc(&decisions.iter().collect::<Vec<_>>()), Some(0.5));
    }

    #[test]
    fn measurement_ids_are_unique_and_values_finite_on_every_slice() {
        for slice in &SLICES {
            let cases = cases::load(slice).unwrap();
            let evidence = evidence_from(slice, &cases, reference_reply);
            let mut ids: Vec<&str> = evidence
                .measurements
                .iter()
                .map(|m| m.id.as_str())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), evidence.measurements.len(), "{}", slice.id);
            assert!(evidence.measurements.iter().all(|m| m.value.is_finite()));
        }
    }
}
