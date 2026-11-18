//! `judge_quality_*`: how well the stack's local judge provider answers a
//! public dataset slice.
//!
//! The provider is the subject, not the model the turn runs on: the turn is a
//! formality every run needs, and `capture` calls `judge-<provider>::evaluate`
//! directly, once per dataset case, then scores the answers against the
//! slice's gold labels. The provider is the stack (the arm), so one id names
//! a slice and pairs the same test across providers and builds.

use anyhow::{Context, Result};
use serde_json::json;

use self::cases::{Slice, PROTECTS, SLICES};
use self::score::Evidence;
use super::{
    assessment, async_trait, ArtifactExpectation, Capability, CapturedDeliverable,
    DeliverableContract, ExecutionPolicy, ObjectiveEvaluation, ProvenanceEvidence, Scenario,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};
use crate::context::E2eContext;

pub mod cases;
pub mod runtime;
pub mod score;

const DELIVERABLE_ID: &str = "judge_quality_evaluation";
const DELIVERABLE_KIND: &str = "judge_quality_audit";
const PROMPT: &str = "Reply with exactly: OK";

/// The registered ids, in `SLICES` order.
pub const IDS: [&str; SLICES.len()] = {
    let mut ids = [""; SLICES.len()];
    let mut index = 0;
    while index < SLICES.len() {
        ids[index] = SLICES[index].id;
        index += 1;
    }
    ids
};

/// One dataset slice, indexed into [`SLICES`].
pub struct JudgeQuality(pub usize);

impl JudgeQuality {
    fn slice(&self) -> &'static Slice {
        SLICES
            .get(self.0)
            .expect("a registered judge-quality slice")
    }
}

#[async_trait]
impl Scenario for JudgeQuality {
    fn id(&self) -> &'static str {
        IDS[self.0]
    }

    fn title(&self) -> Option<&'static str> {
        Some(self.slice().title)
    }

    fn summary(&self) -> Option<&'static str> {
        Some(self.slice().summary)
    }

    fn protects(&self) -> Option<&'static str> {
        Some(PROTECTS)
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            self.id(),
            self.canonical_seed(),
            score::dataset(self.slice()),
            vec![Capability::E2eControlPlaneV1, Capability::IiiFunctions],
            deliverable_contract(),
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        ScenarioSpec {
            id: self.id(),
            prompt: PROMPT.to_string(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(2),
                max_output_tokens: Some(1_024),
                max_total_tokens: Some(200_000),
                stuck_timeout_seconds: 180,
                max_validation_retries: Some(0),
            },
            denied_functions: &[],
            criteria: assessment::criteria(&score::assessments(self.slice())),
        }
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    async fn setup(&self, context: &E2eContext, _run_id: &str) -> Result<()> {
        let (provider, _) = runtime::provider(context).await?;
        runtime::warm_up(context, provider).await
    }

    async fn capture(
        &self,
        context: &E2eContext,
        _observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let evidence = runtime::measure(context, self.slice()).await?;
        Ok(vec![deliverable(&evidence)?])
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
            .context("the judge-quality evidence was not captured")?;
        let evidence: Evidence = serde_json::from_value(content.clone())
            .context("the captured judge-quality evidence does not parse")?;
        Ok(score::evaluate(self.slice(), &evidence))
    }
}

fn deliverable(evidence: &Evidence) -> Result<CapturedDeliverable> {
    Ok(CapturedDeliverable {
        id: DELIVERABLE_ID.to_string(),
        kind: DELIVERABLE_KIND.to_string(),
        provenance: vec![ProvenanceEvidence {
            kind: "judge_provider".to_string(),
            source_id: format!("judge-{}", evidence.provider),
            relation: "evaluated_for_this_run".to_string(),
        }],
        content: serde_json::to_value(evidence)
            .context("serialize the judge-quality evidence")?
            .into(),
        invariants: Vec::new(),
    })
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
                    "schema", "provider", "version", "config", "slice", "cases",
                    "decisions", "measurements"
                ],
                "properties": {
                    "schema": {"const": score::SCHEMA},
                    "provider": {"type": "string"},
                    "version": {"type": "string"},
                    "slice": {"type": "string"},
                    "cases": {"type": "array"},
                    "decisions": {"type": "array"},
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
    use super::score::tests::{evidence_from, null_reply, reference_reply};
    use super::*;
    use crate::report::CompletionState;

    #[test]
    fn every_slice_has_a_valid_formality_spec() {
        for (index, slice) in SLICES.iter().enumerate() {
            let module = JudgeQuality(index);
            assert_eq!(module.id(), slice.id);
            let spec = module.spec("run");
            spec.validate().unwrap();
            assert_eq!(spec.prompt, PROMPT);
            assert_eq!(
                spec.criteria
                    .iter()
                    .map(|c| u16::from(c.weight))
                    .sum::<u16>(),
                100
            );
            assert_eq!(module.allowed_functions("run"), Some(Vec::new()));
        }
    }

    /// The null judge answers nothing and the reference judge echoes the gold
    /// with probability 1, over every embedded decision.
    #[test]
    fn the_null_and_reference_judges_bound_every_slice() {
        for slice in &SLICES {
            let cases = cases::load(slice).unwrap();
            let specs = score::assessments(slice);
            let points = |evaluation: &ObjectiveEvaluation| -> u16 {
                evaluation
                    .awards
                    .iter()
                    .map(|award| u16::from(award.awarded.unwrap_or_default()))
                    .sum()
            };

            let null = score::evaluate(slice, &evidence_from(slice, &cases, null_reply));
            assert_eq!(points(&null), 0, "{}", slice.id);
            assert_eq!(
                assessment::completion(&specs, &null),
                CompletionState::TaskIncomplete,
                "{}",
                slice.id
            );

            let reference = evidence_from(slice, &cases, reference_reply);
            let evaluation = score::evaluate(slice, &reference);
            assert_eq!(points(&evaluation), 100, "{}", slice.id);
            assert_eq!(
                assessment::completion(&specs, &evaluation),
                CompletionState::Completed,
                "{}",
                slice.id
            );
        }
    }

    #[test]
    fn the_reference_evidence_fits_its_contract() {
        let slice = &SLICES[3];
        let cases = cases::load(slice).unwrap();
        let evidence = evidence_from(slice, &cases, reference_reply);
        let captured = deliverable(&evidence).unwrap();
        let contract = deliverable_contract();
        let artifact = &contract.artifacts[0];
        assert_eq!(
            (captured.id.as_str(), captured.kind.as_str()),
            (artifact.id.as_str(), artifact.kind.as_str())
        );
        let content = captured.content.as_json().unwrap();
        let size = serde_json::to_vec(content).unwrap().len();
        assert!(size < artifact.max_size_bytes as usize, "{size} bytes");
        assert!(jsonschema::JSONSchema::compile(&artifact.schema)
            .unwrap()
            .is_valid(content));
        assert_eq!(captured.provenance[0].source_id, "judge-test");
        let parsed: Evidence = serde_json::from_value(content.clone()).unwrap();
        assert_eq!(parsed, evidence);
    }
}
