//! The four dataset slices and their cases, embedded from
//! `tests/fixtures/judge-quality/` (sources, revisions and licences in its
//! README). A slice's revision and sha256 are written into its hashed
//! `accuracy` criterion, so a re-vendored file is a new definition.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

pub struct Slice {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub dataset: &'static str,
    pub revision: &'static str,
    pub sha256: &'static str,
    pub accuracy_description: &'static str,
    pub cases: usize,
    pub decisions: usize,
    pub source: &'static str,
}

pub const PROTECTS: &str = "The answers of the local judge provider the Harness asks to reconcile \
calls and pick functions, measured against public gold labels. If it fails, a provider build or \
configuration answers fewer decisions, or fewer of them right, than the arm it is compared with.";

pub const SLICES: [Slice; 4] = [
    Slice {
        id: "judge_quality_jevbench_easy",
        title: "Judge quality on JevBench easy",
        summary:
            "The stack's one local judge provider answers the 48 JevBench easy decisions, one \
evaluation per call, and its answers are scored against the dataset's gold labels.",
        dataset: "fstandhartinger/jevbench easy.jsonl",
        revision: "609a664e",
        sha256: "sha256:231df3c2c8e88a1a8c137ebe85de96ba70fabd330849098ac7b3c52c70b7172b",
        accuracy_description: "Answers match the gold labels of fstandhartinger/jevbench@609a664e \
easy.jsonl sha256:231df3c2c8e88a1a8c137ebe85de96ba70fabd330849098ac7b3c52c70b7172b",
        cases: 48,
        decisions: 48,
        source: include_str!("../../../tests/fixtures/judge-quality/jevbench/easy.jsonl"),
    },
    Slice {
        id: "judge_quality_jevbench_original",
        title: "Judge quality on JevBench original",
        summary: "The stack's one local judge provider answers the 72 JevBench original \
decisions, one evaluation per call, and its answers are scored against the dataset's gold labels.",
        dataset: "fstandhartinger/jevbench original.jsonl",
        revision: "609a664e",
        sha256: "sha256:5c2414edb3006b8bfcb70fda433f0f9ca015759433849f8d3104328a1f7c4180",
        accuracy_description: "Answers match the gold labels of fstandhartinger/jevbench@609a664e \
original.jsonl sha256:5c2414edb3006b8bfcb70fda433f0f9ca015759433849f8d3104328a1f7c4180",
        cases: 72,
        decisions: 72,
        source: include_str!("../../../tests/fixtures/judge-quality/jevbench/original.jsonl"),
    },
    Slice {
        id: "judge_quality_jevbench_hard",
        title: "Judge quality on JevBench hard",
        summary: "The stack's one local judge provider answers the 111 JevBench hard decisions, \
one evaluation per call, and its answers are scored against the dataset's gold labels.",
        dataset: "fstandhartinger/jevbench hard.jsonl",
        revision: "609a664e",
        sha256: "sha256:89e9e6becb33ed88c1de7d42dcc87531b2fb64cfaef4e1986faf7c37b3f80ebb",
        accuracy_description: "Answers match the gold labels of fstandhartinger/jevbench@609a664e \
hard.jsonl sha256:89e9e6becb33ed88c1de7d42dcc87531b2fb64cfaef4e1986faf7c37b3f80ebb",
        cases: 111,
        decisions: 111,
        source: include_str!("../../../tests/fixtures/judge-quality/jevbench/hard.jsonl"),
    },
    Slice {
        id: "judge_quality_typed_decisions",
        title: "Judge quality on typed-decisions",
        summary: "The stack's one local judge provider answers the 400 typed-decisions test \
cases (2,000 decisions), one evaluation per call, and its answers are scored against the \
dataset's gold labels, a teacher's argmax.",
        dataset: "LocalLLaMA/typed-decisions test",
        revision: "e6aa632c4db73b59c7713bb21066ad1a8e1bfce6",
        sha256: "sha256:5a9c27cf889f7cbf1ca09e439d6ae46722104eb9591ac2348b9f2194d38bbbd8",
        accuracy_description: "Answers match the gold labels of \
LocalLLaMA/typed-decisions@e6aa632c4db73b59c7713bb21066ad1a8e1bfce6 test.jsonl \
sha256:5a9c27cf889f7cbf1ca09e439d6ae46722104eb9591ac2348b9f2194d38bbbd8",
        cases: 400,
        decisions: 2_000,
        source: include_str!("../../../tests/fixtures/judge-quality/typed-decisions/test.jsonl"),
    },
];

#[derive(Debug, Clone, PartialEq)]
pub enum Gold {
    Noul(bool),
    Choice(String),
    Score(usize),
}

#[derive(Debug, Clone)]
pub struct Case {
    pub id: String,
    pub workflow: Option<String>,
    /// JevBench's scenario group: the paraphrases of one scenario share it.
    pub group: Option<String>,
    pub state: Value,
    pub questions: BTreeMap<String, Value>,
    pub gold: BTreeMap<String, Gold>,
    pub agree: BTreeMap<String, bool>,
}

/// Both shapes: JevBench's one `question` with its `expected` label (question
/// id `q`), or typed-decisions' `questions` with `gold` and `agree`.
#[derive(Deserialize)]
struct Row {
    id: String,
    #[serde(default)]
    workflow: Option<String>,
    #[serde(default)]
    group: Option<String>,
    state: Value,
    #[serde(default)]
    question: Option<Value>,
    #[serde(default)]
    expected: Option<Value>,
    #[serde(default)]
    questions: BTreeMap<String, Value>,
    #[serde(default)]
    gold: BTreeMap<String, Value>,
    #[serde(default)]
    agree: BTreeMap<String, bool>,
}

pub fn load(slice: &Slice) -> Result<Vec<Case>> {
    parse(slice.source).with_context(|| format!("load the {} cases", slice.id))
}

pub(super) fn parse(source: &str) -> Result<Vec<Case>> {
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: Row = serde_json::from_str(line).context("parse a dataset row")?;
            case(row)
        })
        .collect()
}

fn case(row: Row) -> Result<Case> {
    let (questions, labels) = match (row.question, row.expected) {
        (Some(question), Some(expected)) => (
            BTreeMap::from([("q".to_string(), question)]),
            BTreeMap::from([("q".to_string(), expected)]),
        ),
        (None, None) => (row.questions, row.gold),
        _ => bail!("case {}: a question needs its expected label", row.id),
    };
    if questions.is_empty() || questions.len() != labels.len() {
        bail!("case {}: every question needs one gold label", row.id);
    }
    let gold = questions
        .iter()
        .map(|(qid, question)| {
            let label = labels
                .get(qid)
                .with_context(|| format!("case {}: no gold for {qid}", row.id))?;
            let gold =
                gold(question, label).with_context(|| format!("case {}: gold of {qid}", row.id))?;
            Ok((qid.clone(), gold))
        })
        .collect::<Result<_>>()?;
    Ok(Case {
        id: row.id,
        workflow: row.workflow,
        group: row.group,
        state: row.state,
        questions,
        gold,
        agree: row.agree,
    })
}

fn gold(question: &Value, label: &Value) -> Result<Gold> {
    let criteria = &question["criteria"];
    let gold = match question["type"].as_str() {
        Some("choice") => label
            .as_str()
            .filter(|label| criteria.get(label).is_some())
            .map(|label| Gold::Choice(label.to_string())),
        Some("noul") => match label {
            Value::Bool(yes) => Some(*yes),
            Value::String(text) if text == "yes" => Some(true),
            Value::String(text) if text == "no" => Some(false),
            _ => None,
        }
        .map(Gold::Noul),
        Some("score") => label
            .as_u64()
            .map(|level| level as usize)
            .filter(|level| {
                criteria
                    .as_array()
                    .is_some_and(|levels| *level < levels.len())
            })
            .map(Gold::Score),
        other => bail!("unknown question type {other:?}"),
    };
    gold.with_context(|| format!("{label} is not a valid answer to the question"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::artifact::sha256_bytes;

    fn kinds(cases: &[Case]) -> [usize; 3] {
        let mut counts = [0; 3];
        for question in cases.iter().flat_map(|case| case.questions.values()) {
            let index = ["choice", "noul", "score"]
                .iter()
                .position(|kind| question["type"] == *kind)
                .expect("a known question type");
            counts[index] += 1;
        }
        counts
    }

    #[test]
    fn each_embedded_file_is_the_pinned_one() {
        for slice in &SLICES {
            assert_eq!(
                sha256_bytes(slice.source.as_bytes()),
                slice.sha256,
                "{}",
                slice.id
            );
            assert!(slice.accuracy_description.contains(slice.revision));
            assert!(slice.accuracy_description.contains(slice.sha256));
        }
    }

    #[test]
    fn the_slices_hold_the_counted_cases_and_decisions() {
        let expected = [
            (48, [36, 12, 0]),
            (72, [36, 24, 12]),
            (111, [67, 38, 6]),
            (400, [600, 600, 800]),
        ];
        for (slice, (cases, counts)) in SLICES.iter().zip(expected) {
            let loaded = load(slice).unwrap();
            assert_eq!(loaded.len(), cases, "{}", slice.id);
            assert_eq!(loaded.len(), slice.cases, "{}", slice.id);
            assert_eq!(kinds(&loaded), counts, "{}", slice.id);
            assert_eq!(
                counts.iter().sum::<usize>(),
                slice.decisions,
                "{}",
                slice.id
            );
            let ids: HashSet<&str> = loaded.iter().map(|case| case.id.as_str()).collect();
            assert_eq!(ids.len(), loaded.len(), "{}: case ids are unique", slice.id);
            for case in &loaded {
                assert_eq!(case.gold.len(), case.questions.len());
            }
        }
    }

    #[test]
    fn typed_decisions_cases_carry_a_workflow_and_an_agreement_flag_per_question() {
        let cases = load(&SLICES[3]).unwrap();
        for case in &cases {
            assert!(case.workflow.is_some(), "{}", case.id);
            assert_eq!(
                case.agree.keys().collect::<Vec<_>>(),
                case.questions.keys().collect::<Vec<_>>()
            );
        }
        let agreed = cases
            .iter()
            .flat_map(|case| case.agree.values())
            .filter(|agreed| **agreed)
            .count();
        assert_eq!(agreed, 1_188);
    }

    #[test]
    fn the_original_paraphrases_share_a_group() {
        let cases = load(&SLICES[1]).unwrap();
        let groups: HashSet<&str> = cases
            .iter()
            .map(|case| case.group.as_deref().unwrap())
            .collect();
        assert_eq!(groups.len(), 36);
    }

    #[test]
    fn a_gold_label_must_answer_its_question() {
        let choice = serde_json::json!({"type": "choice", "criteria": {"a": "A", "b": "B"}});
        let score = serde_json::json!({"type": "score", "criteria": ["low", "high"]});
        let noul = serde_json::json!({"type": "noul"});
        assert_eq!(
            gold(&choice, &"b".into()).unwrap(),
            Gold::Choice("b".into())
        );
        assert!(gold(&choice, &"c".into()).is_err());
        assert_eq!(gold(&score, &1.into()).unwrap(), Gold::Score(1));
        assert!(gold(&score, &2.into()).is_err());
        assert_eq!(gold(&noul, &"no".into()).unwrap(), Gold::Noul(false));
        assert_eq!(gold(&noul, &true.into()).unwrap(), Gold::Noul(true));
        assert!(gold(&noul, &"maybe".into()).is_err());
        assert!(parse(r#"{"id": "x", "state": "s", "question": {"type": "noul"}}"#).is_err());
    }
}
