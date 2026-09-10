use anyhow::{bail, Result};

use crate::assessment::{AssessmentKind, AssessmentPolicy, DeclaredAssessment};
use crate::report::{CompletionState, EvaluationDimension};

use super::{CriterionAward, CriterionSpec, ObjectiveEvaluation, ScenarioSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AssessmentSpec {
    id: &'static str,
    weight: u8,
    description: &'static str,
    dimension: EvaluationDimension,
}

impl AssessmentSpec {
    pub(super) const fn scored(id: &'static str, weight: u8, description: &'static str) -> Self {
        Self {
            id,
            weight,
            description,
            dimension: EvaluationDimension::StructuralIntegrity,
        }
    }

    pub(super) const fn scored_in(
        id: &'static str,
        weight: u8,
        description: &'static str,
        dimension: EvaluationDimension,
    ) -> Self {
        Self {
            id,
            weight,
            description,
            dimension,
        }
    }

    pub(super) const fn weight(self) -> u8 {
        self.weight
    }

    pub(super) const fn id(self) -> &'static str {
        self.id
    }

    pub(super) const fn description(self) -> &'static str {
        self.description
    }

    pub(super) fn full_or_zero(
        self,
        satisfied: bool,
        details: impl Into<String>,
    ) -> AssessmentOutcome {
        AssessmentOutcome {
            spec: self,
            awarded: if satisfied { self.weight } else { 0 },
            details: details.into(),
        }
    }

    /// Records a check that could not run behind an explicit prerequisite gate.
    /// Prefer [`prerequisite_failure`] when constructing the complete evaluation.
    fn skipped_due_to_prerequisite(self, details: impl Into<String>) -> AssessmentOutcome {
        AssessmentOutcome {
            spec: self,
            awarded: 0,
            details: details.into(),
        }
    }

    pub(super) fn award(
        self,
        awarded: u8,
        details: impl Into<String>,
    ) -> Result<AssessmentOutcome> {
        self.validate_award("award", awarded)?;
        Ok(AssessmentOutcome {
            spec: self,
            awarded,
            details: details.into(),
        })
    }

    fn validate_award(self, operation: &str, awarded: u8) -> Result<()> {
        if awarded > self.weight {
            bail!(
                "assessment '{}': {operation}(awarded={awarded}) exceeds max_points={}; expected awarded in 0..={}",
                self.id,
                self.weight,
                self.weight
            );
        }
        Ok(())
    }

    fn criterion(self) -> CriterionSpec {
        let declaration = self.declaration();
        CriterionSpec {
            id: self.id,
            weight: self.weight,
            description: self.description,
            kind: declaration.kind,
            policy: declaration.policy,
            dimension: declaration.dimension,
        }
    }

    fn declaration(self) -> DeclaredAssessment {
        DeclaredAssessment {
            criterion_id: self.id.to_string(),
            possible: self.weight,
            description: self.description.to_string(),
            kind: AssessmentKind::Signal,
            policy: AssessmentPolicy::Advisory,
            dimension: self.dimension,
        }
    }
}

impl ScenarioSpec {
    pub(crate) fn declared_assessments(&self) -> Vec<DeclaredAssessment> {
        self.criteria
            .iter()
            .map(|criterion| DeclaredAssessment {
                criterion_id: criterion.id.to_string(),
                possible: criterion.weight,
                description: criterion.description.to_string(),
                kind: criterion.kind,
                policy: criterion.policy,
                dimension: criterion.dimension,
            })
            .collect()
    }
}

#[derive(Debug)]
pub(super) struct AssessmentOutcome {
    spec: AssessmentSpec,
    awarded: u8,
    details: String,
}

pub(super) fn criteria(specs: &[AssessmentSpec]) -> Vec<CriterionSpec> {
    specs
        .iter()
        .copied()
        .map(AssessmentSpec::criterion)
        .collect()
}

pub(super) fn build_evaluation(
    completion: CompletionState,
    results: impl IntoIterator<Item = AssessmentOutcome>,
) -> ObjectiveEvaluation {
    let mut awards = Vec::new();

    for result in results {
        awards.push(CriterionAward {
            id: result.spec.id.to_string(),
            awarded: Some(result.awarded),
            reason: result.details,
        });
    }

    ObjectiveEvaluation {
        completion,
        awards,
        infrastructure_error: None,
    }
}

pub(super) fn prerequisite_failure(
    specs: &[AssessmentSpec],
    gate_id: impl Into<String>,
    details: impl Into<String>,
) -> ObjectiveEvaluation {
    failed_evaluation(
        CompletionState::Undetermined,
        "prerequisite",
        specs,
        gate_id,
        details,
    )
}

/// Records valid evidence that the subject stopped without producing the
/// requested terminal output. Unlike a missing harness prerequisite, this is
/// a measured incomplete task rather than an undetermined observation.
pub(super) fn task_incomplete(
    specs: &[AssessmentSpec],
    gate_id: impl Into<String>,
    details: impl Into<String>,
) -> ObjectiveEvaluation {
    failed_evaluation(
        CompletionState::TaskIncomplete,
        "completion",
        specs,
        gate_id,
        details,
    )
}

fn failed_evaluation(
    completion: CompletionState,
    gate_kind: &str,
    specs: &[AssessmentSpec],
    gate_id: impl Into<String>,
    details: impl Into<String>,
) -> ObjectiveEvaluation {
    let gate_id = gate_id.into();
    let reason = format!("{gate_kind} '{gate_id}' failed: {}", details.into());
    build_evaluation(
        completion,
        specs
            .iter()
            .copied()
            .map(|spec| spec.skipped_due_to_prerequisite(reason.clone())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: AssessmentSpec = AssessmentSpec::scored("required", 70, "A required outcome.");
    const SIGNAL: AssessmentSpec = AssessmentSpec::scored("signal", 30, "A quality signal.");

    #[test]
    fn criteria_are_numeric_advisory_signals() {
        let criteria = criteria(&[REQUIRED, SIGNAL]);

        assert!(criteria.iter().all(|criterion| {
            criterion.kind == AssessmentKind::Signal
                && criterion.policy == AssessmentPolicy::Advisory
        }));
        assert_eq!(
            criteria
                .iter()
                .map(|criterion| u16::from(criterion.weight))
                .sum::<u16>(),
            100
        );
    }

    #[test]
    fn criteria_preserve_full_zero_and_partial_scores() {
        let evaluation = build_evaluation(
            CompletionState::Completed,
            [
                REQUIRED.full_or_zero(true, "satisfied"),
                SIGNAL.award(12, "partial").unwrap(),
            ],
        );

        assert_eq!(evaluation.awards[0].awarded, Some(70));
        assert_eq!(evaluation.awards[1].awarded, Some(12));
    }

    #[test]
    fn awards_above_the_criterion_weight_are_rejected() {
        assert_eq!(
            SIGNAL.award(31, "too many").unwrap_err().to_string(),
            "assessment 'signal': award(awarded=31) exceeds max_points=30; expected awarded in 0..=30"
        );
    }

    #[test]
    fn prerequisite_and_missing_output_preserve_completion_without_gates() {
        let unavailable = prerequisite_failure(
            &[REQUIRED, SIGNAL],
            "database_available",
            "database capability is unavailable",
        );
        let incomplete = task_incomplete(
            &[REQUIRED, SIGNAL],
            "output_present",
            "the subject produced no output",
        );

        assert_eq!(unavailable.completion, CompletionState::Undetermined);
        assert_eq!(incomplete.completion, CompletionState::TaskIncomplete);
        assert!(unavailable
            .awards
            .iter()
            .all(|award| award.awarded == Some(0)));
        assert!(incomplete
            .awards
            .iter()
            .all(|award| award.awarded == Some(0)));
    }
}
