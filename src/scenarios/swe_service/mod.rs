//! Software engineering tasks sharing one versioned service and trusted curriculum.
//! Only a selected entry snapshot reaches the subject; the lifecycle retains its own code.

mod assets;
mod runtime;
pub(crate) mod workflow;

use anyhow::Result;
use serde_json::json;

use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedInvariant,
    CriterionSpec, DeliverableContract, ExecutionPolicy, InvariantSpec, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioCharacterization, ScenarioExecutionKind,
    ScenarioId, ScenarioObservation, ScenarioSpec,
};
use crate::context::E2eContext;
use crate::report::EvaluationDimension;

pub const REPORT_ID: &str = "swe_service_report";
pub const FIXTURE_REPOSITORY: &str = "iii-hq/e2e-fixture";
pub const FIXTURE_REVISION: &str = "ab373b11ae167ef853f5b5c5184cdcd431a444ea";
pub const WORKSPACE_ROOT_ENV: &str = "HARNESS_E2E_SWE_WORKSPACE_ROOT";

pub const LIFECYCLE_CRITERIA: &[(&str, u8, &str)] = &[
    ("demand", 8, "Record customer acceptance and a real GitHub issue before source changes."),
    ("planning", 8, "Link ownership, dependencies and planned tests to acceptance, with the plan committed in a real PR before implementation."),
    ("implementation", 20, "Deliver the working product and authored regression tests at the SHA published in the PR."),
    ("review_ci", 15, "Record technical self-review and successful GitHub CI on the exact candidate SHA, with tests that detect the original defects."),
    ("release", 12, "Merge and publish an immutable GitHub release after compatibility review, and verify HTTP behavior and restart locally."),
    ("evolution", 10, "Deliver the tenant requirement through CI, review and merge without regressing accepted behavior."),
    ("operations", 12, "Merge the incident repair with verified CI and demonstrate restart, rollback and upgrade without data loss or duplicate effects."),
    ("handoff", 5, "Deliver the runbook through CI, review, merge and a final release, close the issue and revalidate operational procedures."),
    ("convergence", 5, "Completed stages / (completed stages + rejected checkpoints), awarded after lifecycle completion; no retry cutoff."),
    ("resource_efficiency", 5, "After completion, mean min(1, reference / observed) for elapsed time, generations and tokens; references are 5400 s, 320 and 1500000, with no execution cutoff. Missing measurements remain unevaluated."),
];

pub fn criteria(case: Case) -> Vec<CriterionSpec> {
    if case.lifecycle() {
        LIFECYCLE_CRITERIA
            .iter()
            .map(|(id, weight, description)| {
                CriterionSpec::scored(
                    id,
                    *weight,
                    description,
                    if matches!(*id, "convergence" | "resource_efficiency") {
                        EvaluationDimension::Efficiency
                    } else {
                        EvaluationDimension::Deliverable
                    },
                )
            })
            .collect()
    } else {
        vec![CriterionSpec::scored(
            "swe_delivery",
            100,
            "Deliver the requested ticket while preserving accepted software and protected checks.",
            EvaluationDimension::Deliverable,
        )]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// Zero selects the continuous lifecycle; 1..=8 select an isolated ticket.
    pub ticket: u8,
    pub id: &'static str,
}

impl Case {
    pub fn from_scenario(scenario: ScenarioId) -> Option<Self> {
        let ticket = match scenario {
            ScenarioId::SweConfigIsolation => 1,
            ScenarioId::SweCacheInvalidation => 2,
            ScenarioId::SweBatchReplay => 3,
            ScenarioId::SweReplayRecovery => 4,
            ScenarioId::SweContractMigration => 5,
            ScenarioId::SweTenantIsolation => 6,
            ScenarioId::SweReplayPerformance => 7,
            ScenarioId::SweReleaseHandoff => 8,
            ScenarioId::SoftwareCompanyLifecycle => 0,
            _ => return None,
        };
        Some(Self {
            ticket,
            id: scenario.as_str(),
        })
    }

    pub fn lifecycle(self) -> bool {
        self.ticket == 0
    }
    pub fn mode(self) -> &'static str {
        if self.lifecycle() {
            "lifecycle"
        } else {
            "isolated"
        }
    }
    pub fn first_ticket(self) -> u8 {
        self.ticket.max(1)
    }
    pub fn deadline_seconds(self) -> Option<u64> {
        (!self.lifecycle()).then_some(900)
    }
    pub fn generations(self) -> Option<u32> {
        (!self.lifecycle()).then_some(64)
    }
    pub fn tokens(self) -> Option<u64> {
        (!self.lifecycle()).then_some(250_000)
    }

    pub fn description(self) -> &'static str {
        match self.ticket {
            1 => "Diagnose configuration precedence and isolate independent CLI settings.",
            2 => "Repair stale profile caches under out-of-order change notifications.",
            3 => "Implement ordered, bounded event replay with resumable cursor semantics.",
            4 => "Recover interrupted event replay without losing or duplicating durable effects.",
            5 => "Migrate a profile API and adapt to a legacy consumer revealed by a real canary.",
            6 => "Repair cross-tenant profile access while preserving authorized operations.",
            7 => "Optimize replay work while preserving paging, order and existing behavior.",
            8 => "Address configuration-removal feedback and document the software handoff.",
            _ => "Run a software company lifecycle: demand, planning, implementation, review and CI, release, evolution, incident recovery and handoff.",
        }
    }
}

pub fn is_swe(scenario: ScenarioId) -> bool {
    Case::from_scenario(scenario).is_some()
}

/// One SWE ticket, or the continuous lifecycle, identified by its registered id.
pub struct SweService(pub ScenarioId);

#[async_trait]
impl Scenario for SweService {
    fn id(&self) -> &'static str {
        self.0.as_str()
    }

    fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::CompositeFlow
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        Ok(ScenarioCharacterization::realistic())
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        let selection = Case::from_scenario(self.0).expect("SWE scenario identity");
        ScenarioCase::new(
            selection.id,
            super::stable_seed(selection.id),
            json!({
                "fixture_repository": FIXTURE_REPOSITORY,
                "fixture_revision": FIXTURE_REVISION,
                "mode": selection.mode(),
                "entry_snapshot": selection.first_ticket() - 1,
                "task": selection.ticket,
                "task_count": if selection.lifecycle() { 8 } else { 1 },
                "deadline_seconds": selection.deadline_seconds(),
                "delegation": "optional",
                "curriculum_version": if selection.lifecycle() { 3 } else { 1 },
                "lifecycle_contract_sha256": if selection.lifecycle() { Some(crate::artifact::sha256_value(&include_str!("lifecycle.py"))?) } else { None },
                "github_contract_sha256": if selection.lifecycle() { Some(crate::artifact::sha256_value(&json!({"bridge": include_str!("github_ops.py"), "workflow": include_str!("github-ci.yml")}))?) } else { None },
                "evaluator_contract_sha256": crate::artifact::sha256_value(&json!({
                    "controller": include_str!("controller.py"),
                    "probes": include_str!("probes.py"),
                    "isolation": include_str!("isolation.py"),
                }))?,
                "efficiency_reference": selection.lifecycle().then_some(json!({"elapsed_ms":5_400_000,"generations":320,"tokens":1_500_000})),
            }),
            vec![
                Capability::IiiFunctions,
                Capability::E2eControlPlaneV1,
                Capability::SweIsolatedPythonWorkspace,
            ],
            DeliverableContract {
                artifacts: vec![ArtifactExpectation {
                    id: REPORT_ID.into(),
                    kind: "swe-service-report".into(),
                    media_type: "application/json".into(),
                    schema: json!({"type":"object","required":["schema","scenario_id","fixture_revision","accepted_head","accepted_tickets","terminal_status","accepted_patch","unaccepted_patch"],"properties":{"schema":{"const":"swe-service-report"},"accepted_tickets":{"type":"array"},"terminal_status":{"type":"string"}}}),
                    max_size_bytes: 16 * 1024 * 1024,
                }],
                invariants: vec![InvariantSpec {
                    id: "delivery_complete".into(),
                    description: "All requested checkpoints have been delivered and verified."
                        .into(),
                }],
                provenance_required: true,
                capture_before_cleanup: true,
            },
        )
    }

    fn spec(&self, _run_id: &str) -> ScenarioSpec {
        let case = Case::from_scenario(self.0).expect("SWE scenario identity");
        ScenarioSpec {
            id: case.id,
            prompt: case.description().into(),
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: case.generations(),
                max_output_tokens: (!case.lifecycle()).then_some(32_768),
                max_total_tokens: case.tokens(),
                stuck_timeout_seconds: (!case.lifecycle()).then_some(600),
                max_validation_retries: None,
            },
            denied_functions: &[
                "e2e::*",
                "github::*",
                "configuration::*",
                "compose::*",
                "router::*",
            ],
            criteria: criteria(case),
        }
    }

    /// The trusted runtime owns the verdict; the generic report bridge attaches it.
    async fn evaluate(
        &self,
        _context: &E2eContext,
        _observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        Ok(ObjectiveEvaluation {
            completion: crate::report::CompletionState::Undetermined,
            awards: Vec::new(),
            infrastructure_error: None,
        })
    }
}

/// Read the trusted runtime's typed termination outcome for the generic report bridge.
pub(crate) fn execution_outcome(
    output: &std::path::Path,
    attempt: &str,
) -> Option<crate::report::RunStatus> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(runtime::final_report_path(output, attempt)).ok()?)
            .ok()?;
    match value.get("terminal_status")?.as_str()? {
        "resource_limit" => Some(crate::report::RunStatus::ResourceLimit),
        "cancelled" => Some(crate::report::RunStatus::SubjectError),
        _ => None,
    }
}

/// Attach the independent pre-cleanup record even when a workflow deadline skipped its capture node.
pub(crate) fn attach_report(
    output: &std::path::Path,
    attempt: &str,
    case: &ScenarioCase,
    report: &mut crate::report::E2eRunReport,
) -> Result<()> {
    let path = runtime::final_report_path(output, attempt);
    if !path.is_file() {
        if report.failures.is_empty() {
            anyhow::bail!("SWE execution has no final checkpoint evidence");
        }
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let terminal = value
        .get("terminal_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("infrastructure_error");
    let completed = terminal == "completed";
    if matches!(terminal, "completed" | "capability_failure") {
        report.set_completion(
            if completed {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            crate::report::EvaluatorAvailability::Available,
        );
    }
    if let Some(session) = value.get("session_id").and_then(serde_json::Value::as_str) {
        report.session_id = session.into();
    }
    if let Some(metrics) = value.get("metrics") {
        report.metrics = Some(serde_json::from_value(metrics.clone())?);
    }
    if let Some(transcript) = value.get("transcript") {
        report.transcript = Some(transcript.clone());
    }
    if let Some(cost) = report
        .metrics
        .as_ref()
        .and_then(|metrics| metrics.totals.cost_usd)
    {
        report.cost.subject_usd = Some(cost);
        report.cost.total_usd = Some(cost);
    }
    if matches!(
        terminal,
        "resource_limit" | "cancelled" | "infrastructure_error"
    ) {
        report.push_failure(
            if terminal == "resource_limit" {
                crate::report::RunStatus::ResourceLimit
            } else if terminal == "cancelled" {
                crate::report::RunStatus::SubjectError
            } else {
                crate::report::RunStatus::InfrastructureError
            },
            crate::report::FailurePhase::Execute,
            format!(
                "SWE execution ended as {terminal}; its last accepted Git prefix was preserved"
            ),
        );
    }
    let head = value
        .get("accepted_head")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut deliverables = crate::report::evaluate_deliverables(
        case,
        vec![CapturedDeliverable {
            id: REPORT_ID.into(),
            kind: "swe-service-report".into(),
            content: value.clone().into(),
            invariants: vec![CapturedInvariant {
                id: "delivery_complete".into(),
                passed: completed,
                reason: format!("terminal state: {terminal}"),
            }],
            provenance: vec![ProvenanceEvidence {
                kind: "git-checkpoint".into(),
                source_id: head,
                relation: "immutable accepted prefix captured before workspace cleanup".into(),
            }],
        }],
    )?;
    let reference = crate::artifact::write_json(
        output,
        path.strip_prefix(output)?,
        format!("{attempt}-swe-report"),
        "swe-service-report",
        &value,
    )?;
    for deliverable in &mut deliverables {
        deliverable.artifact = Some(reference.clone());
    }
    report.evidence.push(reference);
    report.deliverables.extend(deliverables);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluator_asset_changes_change_case_inputs_identity() -> Result<()> {
        let assets = json!({
            "controller": include_str!("controller.py"),
            "probes": include_str!("probes.py"),
            "isolation": include_str!("isolation.py"),
        });
        let digest = crate::artifact::sha256_value(&assets)?;
        for scenario in [
            ScenarioId::SoftwareCompanyLifecycle,
            ScenarioId::SweConfigIsolation,
        ] {
            let case = SweService(scenario).case(0)?;
            assert_eq!(
                case.inputs["evaluator_contract_sha256"].as_str(),
                Some(digest.as_str())
            );
            for name in ["controller", "probes", "isolation"] {
                let mut changed_assets = assets.clone();
                let content = changed_assets[name].as_str().unwrap().to_owned();
                changed_assets[name] = json!(format!("{content}\n# evaluator changed"));
                let mut changed_inputs = case.inputs.clone();
                changed_inputs["evaluator_contract_sha256"] =
                    json!(crate::artifact::sha256_value(&changed_assets)?);
                assert_ne!(
                    case.inputs_sha256,
                    crate::artifact::sha256_value(&changed_inputs)?,
                    "{scenario:?}: {name} must affect comparability"
                );
            }
        }
        Ok(())
    }
}
