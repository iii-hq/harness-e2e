//! `memory` — what the memory worker puts in front of the subject.
//!
//! Each case plants a fresh bank (the stack's own `main` is never touched),
//! sends its prompt and follow-ups, and audits what memory injected into each
//! turn against what that turn needed. The subject has no callable functions:
//! memory is judged only on what it delivers unasked.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use self::cases::{bank, render, Case, Change, CASES};
use self::score::Evidence;
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, DeliverableContract,
    ExecutionPolicy, ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};
use crate::context::E2eContext;

pub mod cases;
pub mod observe;
pub mod runtime;
pub mod score;

const DELIVERABLE_ID: &str = "memory_evaluation";
const DELIVERABLE_KIND: &str = "memory_injection_audit";

/// The registered ids, in `CASES` order.
pub const IDS: [&str; CASES.len()] = {
    let mut ids = [""; CASES.len()];
    let mut index = 0;
    while index < CASES.len() {
        ids[index] = CASES[index].id;
        index += 1;
    }
    ids
};

/// One of the memory cases, indexed into [`IDS`].
pub struct Memory(pub usize);

impl Memory {
    fn item(&self) -> &'static Case {
        CASES.get(self.0).expect("a registered memory case")
    }
}

/// What `setup` plants and what the case changes later, rendered for
/// `run_id`, and what its teacher says. `entities`, `changes` and `teacher`
/// are written only when the case has them, so a case without them keeps its
/// digest.
fn planted_state(case: &Case, run_id: &str) -> Value {
    if case.baseline {
        return json!({"memories": [], "rules": []});
    }
    let rules: Vec<Value> = case
        .rules
        .iter()
        .map(|rule| json!({"name": rule.name, "content": render(rule.content, run_id)}))
        .collect();
    let mut state = json!({"memories": cases::planted_memories(case, run_id), "rules": rules});
    let text = |seed: usize| render(case.memories[seed].text, run_id);
    let entities: serde_json::Map<String, Value> = case
        .memories
        .iter()
        .filter(|seed| !seed.entities.is_empty())
        .map(|seed| (render(seed.text, run_id), json!(seed.entities)))
        .collect();
    if !entities.is_empty() {
        state["entities"] = Value::Object(entities);
    }
    if !case.changes.is_empty() {
        let changes = case.changes.iter().map(|&(after, change)| {
            let mut described = match change {
                Change::RuleSet { name, content } => {
                    json!({"name": name, "content": render(content, run_id)})
                }
                Change::Save { seed } | Change::Delete { seed } => json!({"text": text(seed)}),
                Change::Supersede { old, new } => json!({"old": text(old), "new": text(new)}),
                Change::Update { seed, to } => json!({"old": text(seed), "text": text(to)}),
                Change::DeleteBank => json!({}),
            };
            described["after"] = after.into();
            described["function"] = change.function().into();
            described
        });
        state["changes"] = changes.collect();
    }
    // Templates: rendered, a token-shaped value said in passing would sit in
    // every definition.
    if !case.teacher.is_empty() {
        state["teacher"] = json!(case.teacher);
    }
    state
}

/// Whether `harness::compact` runs once exchange `finished` has its answer:
/// the case names the exchange it compacts before.
fn compacts_after(case: &Case, finished: usize) -> bool {
    case.compact_before == Some(finished + 1)
}

#[async_trait]
impl Scenario for Memory {
    fn id(&self) -> &'static str {
        IDS[self.0]
    }

    fn title(&self) -> Option<&'static str> {
        Some(self.item().title)
    }

    fn summary(&self) -> Option<&'static str> {
        Some(self.item().summary)
    }

    fn protects(&self) -> Option<&'static str> {
        Some(self.item().protects)
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        let item = self.item();
        ScenarioCase::new(
            item.id,
            self.canonical_seed(),
            json!({
                "baseline": item.baseline,
                "memories": item.memories.len(),
                "rules": item.rules.len(),
                "exchanges": 1 + item.followups.len(),
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::E2eRunScopedFixtures,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        let item = self.item();
        ScenarioSpec {
            id: item.id,
            prompt: item.prompt.to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(4),
                max_output_tokens: Some(4096),
                max_total_tokens: Some(200_000),
                stuck_timeout_seconds: 300,
                max_validation_retries: Some(0),
            },
            denied_functions: &["memory::*"],
            criteria: score::criteria(item),
        }
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    fn dialogue_followups(&self, _run_id: &str) -> Vec<String> {
        self.item()
            .followups
            .iter()
            .map(|followup| followup.to_string())
            .collect()
    }

    fn send_metadata(&self, run_id: &str) -> serde_json::Map<String, Value> {
        serde_json::Map::from_iter([("memory_bank".to_string(), json!(bank(run_id)))])
    }

    fn planted_state(&self, run_id: &str) -> Option<Value> {
        Some(planted_state(self.item(), run_id))
    }

    async fn before_followup(
        &self,
        context: &E2eContext,
        run_id: &str,
        session_id: &str,
        exchange: usize,
    ) {
        // Bank changes before the compaction, as `Case.changes` states.
        runtime::apply_changes(context, self.item(), run_id, exchange).await;
        if compacts_after(self.item(), exchange) {
            runtime::compact(context, run_id, session_id).await;
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        runtime::setup(context, self.item(), run_id).await
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let mut snapshot = runtime::snapshot(run_id);
        if cases::audits_bank_after(self.item()) {
            snapshot.bank_after =
                Some(runtime::bank_after(context, run_id, &observation.transcript).await?);
        }
        let evidence = score::evidence(self.item(), run_id, &observation.transcript, snapshot);
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: DELIVERABLE_KIND.to_string(),
            provenance: vec![ProvenanceEvidence {
                kind: "memory_worker".to_string(),
                source_id: evidence.bank.clone(),
                relation: "planted_before_session".to_string(),
            }],
            content: serde_json::to_value(&evidence)
                .context("serialize the memory evidence")?
                .into(),
            invariants: Vec::new(),
        }])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        let content = observation
            .deliverables
            .iter()
            .find(|deliverable| deliverable.id == DELIVERABLE_ID)
            .and_then(|deliverable| deliverable.content.as_json())
            .context("the memory evidence was not captured")?;
        let evidence: Evidence = serde_json::from_value(content.clone())
            .context("the captured memory evidence does not parse")?;
        Ok(score::evaluate(self.item(), &evidence))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        runtime::cleanup(context, self.item(), run_id).await
    }
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: DELIVERABLE_KIND.to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": [
                    "schema", "bank", "retrieval", "canaries", "seeds", "rules",
                    "observed", "prompts", "compaction_status", "measurements"
                ],
                "properties": {
                    "schema": {"type": "string"},
                    "bank": {"type": "string"},
                    "retrieval": {"enum": ["minilm", "lexical", null]},
                    "measurements": {"type": "array"},
                },
            }),
            max_size_bytes: 4 * 1024 * 1024,
        }],
        invariants: Vec::new(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::ScenarioId;

    #[test]
    fn the_spec_carries_the_cases_prompt_and_criteria() {
        for (index, case) in CASES.iter().enumerate() {
            let module = Memory(index);
            assert_eq!(module.id(), case.id);
            let spec = module.spec("run");
            spec.validate().unwrap();
            assert_eq!(spec.prompt, case.prompt);
            assert_eq!(spec.criteria.len(), case.criteria.len());
            assert_eq!(module.dialogue_followups("run").len(), case.followups.len());
        }
    }

    #[test]
    fn planted_state_names_changes_entities_and_a_teacher_only_for_a_case_with_them() {
        for case in &CASES {
            let state = planted_state(case, "contract");
            let changes = state.get("changes").and_then(Value::as_array);
            assert_eq!(
                changes.map_or(0, Vec::len),
                case.changes.len(),
                "{}",
                case.id
            );
            assert!(state.get("entities").is_none(), "{}", case.id);
            // The teacher's messages as written: the key said in passing
            // stays a template, never a token-shaped value.
            match state.get("teacher") {
                Some(teacher) => assert_eq!(*teacher, json!(case.teacher), "{}", case.id),
                None => assert!(case.teacher.is_empty(), "{}", case.id),
            }
        }
        let learning = CASES
            .iter()
            .find(|case| case.id == "memory_learn_then_recall")
            .unwrap();
        let teacher = &planted_state(learning, "contract")["teacher"];
        assert!(teacher[1].as_str().unwrap().contains("{k1}"), "{teacher}");
        let case = &cases::CHANGING;
        let text = |index: usize| render(case.memories[index].text, "a");
        let state = planted_state(case, "a");
        // The seeds a change saves later are not planted up front.
        assert_eq!(
            state["memories"],
            json!([text(0), text(1), text(2), text(3), text(6)])
        );
        let mut entities = serde_json::Map::new();
        entities.insert(text(6), json!(["search"]));
        assert_eq!(state["entities"], Value::Object(entities));
        let changes = state["changes"].as_array().unwrap();
        let functions: Vec<&str> = changes
            .iter()
            .map(|change| change["function"].as_str().unwrap())
            .collect();
        assert_eq!(
            functions,
            case.changes
                .iter()
                .map(|(_, change)| change.function())
                .collect::<Vec<_>>()
        );
        assert_eq!(changes[0]["after"], 0);
        assert!(changes[0]["content"]
            .as_str()
            .unwrap()
            .contains(&cases::canary("a", "after")));
        assert_eq!(
            changes[3],
            json!({"after": 0, "function": "memory::supersede", "old": text(2), "new": text(4)})
        );
        assert_eq!(
            changes[4],
            json!({"after": 0, "function": "memory::update", "old": text(3), "text": text(5)})
        );
        assert_eq!(
            changes[5],
            json!({"after": 1, "function": "memory::bank::delete"})
        );
    }

    #[test]
    fn the_compaction_cue_fires_after_the_exchange_before_the_named_one() {
        let repeat = CASES
            .iter()
            .find(|case| case.id == "memory_session_repeat")
            .unwrap();
        let cue = repeat.compact_before.unwrap();
        let firing: Vec<usize> = (0..=repeat.followups.len())
            .filter(|&finished| compacts_after(repeat, finished))
            .collect();
        assert_eq!(firing, [cue - 1]);
        assert!((0..4).all(|finished| !compacts_after(&CASES[0], finished)));
    }

    #[tokio::test]
    async fn a_capture_satisfies_its_contract_and_evaluates() {
        let module = Memory(0);
        let context = E2eContext::from_client(iii_sdk::IIIClient::new("ws://127.0.0.1:1"));
        let mut observation = ScenarioObservation::for_test(
            ScenarioId::MemoryRelevantRecall,
            "run",
            json!({"messages": []}),
            "",
            Vec::new(),
        );

        let captured = module.capture(&context, &observation, "run").await.unwrap();
        assert_eq!(captured.len(), 1);
        let deliverable = &captured[0];
        let artifact = &observation.case.deliverable_contract.artifacts[0];
        assert_eq!(
            (deliverable.id.as_str(), deliverable.kind.as_str()),
            (artifact.id.as_str(), artifact.kind.as_str())
        );
        let content = deliverable.content.as_json().unwrap();
        assert!(jsonschema::JSONSchema::compile(&artifact.schema)
            .unwrap()
            .is_valid(content));
        let measurements = content["measurements"].as_array().unwrap();
        assert!(!measurements.is_empty());
        assert!(measurements
            .iter()
            .all(|measurement| measurement["id"].as_str().unwrap().starts_with("memory_")));
        let provenance = &deliverable.provenance[0];
        assert_eq!(
            (
                provenance.kind.as_str(),
                provenance.source_id.as_str(),
                provenance.relation.as_str()
            ),
            ("memory_worker", "e2e-run", "planted_before_session")
        );

        // Nothing was observed: an infrastructure error, not a score.
        observation.deliverables = captured;
        let evaluation = module
            .evaluate(&context, &observation, "run")
            .await
            .unwrap();
        assert_eq!(evaluation.awards.len(), CASES[0].criteria.len());
        assert!(evaluation.infrastructure_error.is_some());

        observation.deliverables.clear();
        assert!(module
            .evaluate(&context, &observation, "run")
            .await
            .is_err());
    }
}
