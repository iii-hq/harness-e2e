use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifact;
use crate::report::{E2eReport, EvaluationDimension};

const MAX_DELAY_MS: u64 = 120_000;
const MAX_DISCONNECT_MS: u64 = 300_000;
const MAX_ACTIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedTerminalOutcome {
    Recovered,
    Cancelled,
}

fn default_expected_outcome() -> ExpectedTerminalOutcome {
    ExpectedTerminalOutcome::Recovered
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelayRule {
    pub target: String,
    pub delay_ms: u64,
    #[serde(default = "default_occurrences")]
    pub occurrences: u8,
}

fn default_occurrences() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisconnectRule {
    pub target: String,
    pub after_action: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancellationRule {
    pub after_action: String,
    #[serde(default = "default_cancel_grace_ms")]
    pub grace_period_ms: u64,
}

fn default_cancel_grace_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultProfile {
    pub profile_id: String,
    pub seed: u64,
    #[serde(default = "default_expected_outcome")]
    pub expected_outcome: ExpectedTerminalOutcome,
    #[serde(default)]
    pub delayed_calls: Vec<DelayRule>,
    #[serde(default)]
    pub fail_first_attempt: Vec<String>,
    #[serde(default)]
    pub duplicate_events: bool,
    #[serde(default)]
    pub child_timeout_ms: Option<u64>,
    #[serde(default)]
    pub transient_disconnect: Option<DisconnectRule>,
    #[serde(default)]
    pub out_of_order_results: bool,
    #[serde(default)]
    pub cancellation: Option<CancellationRule>,
    pub max_work_amplification: f64,
}

impl FaultProfile {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let profile: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode fault profile {}", path.display()))?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.profile_id, "profile_id")?;
        if !self.max_work_amplification.is_finite() || self.max_work_amplification < 1.0 {
            bail!("max_work_amplification must be finite and at least 1.0");
        }
        for rule in &self.delayed_calls {
            validate_target(&rule.target, "delayed call target")?;
            if rule.delay_ms == 0 || rule.delay_ms > MAX_DELAY_MS {
                bail!("delay_ms must be between 1 and {MAX_DELAY_MS}");
            }
            if rule.occurrences == 0 {
                bail!("delay occurrences must be at least one");
            }
        }
        let mut fail_targets = BTreeSet::new();
        for target in &self.fail_first_attempt {
            validate_target(target, "fail-first target")?;
            if !fail_targets.insert(target) {
                bail!("fail-first target '{target}' is duplicated");
            }
        }
        if let Some(timeout_ms) = self.child_timeout_ms {
            if timeout_ms == 0 || timeout_ms > MAX_DELAY_MS {
                bail!("child_timeout_ms must be between 1 and {MAX_DELAY_MS}");
            }
        }
        if let Some(disconnect) = &self.transient_disconnect {
            validate_target(&disconnect.target, "disconnect target")?;
            validate_identifier(&disconnect.after_action, "disconnect after_action")?;
            if disconnect.duration_ms == 0 || disconnect.duration_ms > MAX_DISCONNECT_MS {
                bail!("disconnect duration_ms must be between 1 and {MAX_DISCONNECT_MS}");
            }
        }
        match (&self.expected_outcome, &self.cancellation) {
            (ExpectedTerminalOutcome::Cancelled, None) => {
                bail!("a cancelled profile must declare a cancellation rule")
            }
            (ExpectedTerminalOutcome::Recovered, Some(_)) => {
                bail!("a recovered profile cannot declare a cancellation rule")
            }
            (_, Some(cancellation)) => {
                validate_identifier(&cancellation.after_action, "cancellation after_action")?;
                if cancellation.grace_period_ms == 0
                    || cancellation.grace_period_ms > MAX_DISCONNECT_MS
                {
                    bail!("cancellation grace_period_ms must be between 1 and {MAX_DISCONNECT_MS}");
                }
            }
            _ => {}
        }
        let action_count = self.action_count();
        if action_count == 0 {
            bail!("fault profile must declare at least one perturbation");
        }
        if action_count > MAX_ACTIONS {
            bail!("fault profile declares {action_count} actions; maximum is {MAX_ACTIONS}");
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        artifact::sha256_value(self)
    }

    pub fn materialize(&self) -> Result<FaultPlan> {
        self.validate()?;
        let profile_sha256 = self.sha256()?;
        let mut planned = Vec::<PlannedAction>::new();
        for rule in &self.delayed_calls {
            for occurrence in 1..=rule.occurrences {
                planned.push(PlannedAction::new(
                    FaultActionKind::DelayCall,
                    rule.target.clone(),
                    Some(rule.delay_ms),
                    Some(u32::from(occurrence)),
                ));
            }
        }
        for target in &self.fail_first_attempt {
            planned.push(PlannedAction::new(
                FaultActionKind::FailFirstAttempt,
                target.clone(),
                None,
                Some(1),
            ));
        }
        if self.duplicate_events {
            planned.push(PlannedAction::new(
                FaultActionKind::DuplicateEvent,
                "event-delivery".into(),
                None,
                None,
            ));
        }
        if let Some(timeout_ms) = self.child_timeout_ms {
            planned.push(PlannedAction::new(
                FaultActionKind::ChildTimeout,
                "child-session".into(),
                Some(timeout_ms),
                None,
            ));
        }
        if let Some(disconnect) = &self.transient_disconnect {
            planned.push(PlannedAction::new(
                FaultActionKind::TransientDisconnect,
                disconnect.target.clone(),
                Some(disconnect.duration_ms),
                None,
            ));
        }
        if self.out_of_order_results {
            planned.push(PlannedAction::new(
                FaultActionKind::OutOfOrderResult,
                "child-results".into(),
                None,
                None,
            ));
        }
        if let Some(cancellation) = &self.cancellation {
            planned.push(PlannedAction::new(
                FaultActionKind::CancelExecution,
                cancellation.after_action.clone(),
                Some(cancellation.grace_period_ms),
                None,
            ));
        }
        planned.sort_by_key(|action| action.sort_key(self.seed));
        let actions = planned
            .into_iter()
            .enumerate()
            .map(|(index, action)| action.finish(index as u32 + 1, &profile_sha256))
            .collect();
        Ok(FaultPlan {
            profile_id: self.profile_id.clone(),
            profile_sha256,
            seed: self.seed,
            expected_outcome: self.expected_outcome,
            actions,
        })
    }

    fn action_count(&self) -> usize {
        self.delayed_calls
            .iter()
            .map(|rule| usize::from(rule.occurrences))
            .sum::<usize>()
            + self.fail_first_attempt.len()
            + usize::from(self.duplicate_events)
            + usize::from(self.child_timeout_ms.is_some())
            + usize::from(self.transient_disconnect.is_some())
            + usize::from(self.out_of_order_results)
            + usize::from(self.cancellation.is_some())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FaultActionKind {
    DelayCall,
    FailFirstAttempt,
    DuplicateEvent,
    ChildTimeout,
    TransientDisconnect,
    OutOfOrderResult,
    CancelExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultAction {
    pub action_id: String,
    pub sequence: u32,
    pub kind: FaultActionKind,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_attempt: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultPlan {
    pub profile_id: String,
    pub profile_sha256: String,
    pub seed: u64,
    pub expected_outcome: ExpectedTerminalOutcome,
    pub actions: Vec<FaultAction>,
}

impl FaultPlan {
    pub fn read(path: &Path) -> Result<Self> {
        let value = read_json(path, "fault plan")?;
        let plan: Self = serde_json::from_value(value)
            .with_context(|| format!("decode fault plan {}", path.display()))?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.profile_id, "profile_id")?;
        validate_sha256(&self.profile_sha256, "profile_sha256")?;
        if self.actions.is_empty() || self.actions.len() > MAX_ACTIONS {
            bail!("fault plan action count is outside the supported range");
        }
        let mut ids = BTreeSet::new();
        for (index, action) in self.actions.iter().enumerate() {
            if action.sequence != index as u32 + 1 {
                bail!("fault plan sequence must be contiguous and one-based");
            }
            validate_sha256(&action.action_id, "fault action id")?;
            validate_target(&action.target, "fault action target")?;
            if !ids.insert(&action.action_id) {
                bail!("fault plan action ids must be unique");
            }
        }
        Ok(())
    }

    pub fn write(&self, path: &Path) -> Result<PathBuf> {
        self.validate()?;
        write_json(path, self, "fault plan")
    }
}

#[derive(Debug, Clone)]
struct PlannedAction {
    kind: FaultActionKind,
    target: String,
    duration_ms: Option<u64>,
    trigger_attempt: Option<u32>,
}

impl PlannedAction {
    fn new(
        kind: FaultActionKind,
        target: String,
        duration_ms: Option<u64>,
        trigger_attempt: Option<u32>,
    ) -> Self {
        Self {
            kind,
            target,
            duration_ms,
            trigger_attempt,
        }
    }

    fn sort_key(&self, seed: u64) -> String {
        digest(format!(
            "{seed}:{:?}:{}:{:?}:{:?}",
            self.kind, self.target, self.duration_ms, self.trigger_attempt
        ))
    }

    fn finish(self, sequence: u32, profile_sha256: &str) -> FaultAction {
        let action_id = digest(format!(
            "{profile_sha256}:{sequence}:{:?}:{}:{:?}:{:?}",
            self.kind, self.target, self.duration_ms, self.trigger_attempt
        ));
        FaultAction {
            action_id,
            sequence,
            kind: self.kind,
            target: self.target,
            duration_ms: self.duration_ms,
            trigger_attempt: self.trigger_attempt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FaultObservationStatus {
    Recovered,
    Unrecovered,
    NotTriggered,
    InjectorFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultObservation {
    pub action_id: String,
    pub kind: FaultActionKind,
    pub status: FaultObservationStatus,
    pub applied_at: Option<String>,
    pub recovered_at: Option<String>,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultJournal {
    pub execution_id: String,
    pub lane: String,
    pub profile_sha256: String,
    pub started_at: String,
    pub completed_at: String,
    pub cleanup_completed: bool,
    pub cancellation_observed: bool,
    pub observed_work_amplification: Option<f64>,
    pub results_sha256: Option<String>,
    pub actions: Vec<FaultObservation>,
    #[serde(default)]
    pub infrastructure_failures: Vec<String>,
    #[serde(default)]
    pub cleanup_evidence: BTreeMap<String, String>,
}

impl FaultJournal {
    pub fn read(path: &Path) -> Result<Self> {
        let value = read_json(path, "fault journal")?;
        let journal: Self = serde_json::from_value(value)
            .with_context(|| format!("decode fault journal {}", path.display()))?;
        journal.validate()?;
        Ok(journal)
    }

    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.execution_id, "execution_id")?;
        validate_identifier(&self.lane, "lane")?;
        validate_sha256(&self.profile_sha256, "profile_sha256")?;
        let started = DateTime::parse_from_rfc3339(&self.started_at)
            .context("fault journal started_at must be RFC 3339")?;
        let completed = DateTime::parse_from_rfc3339(&self.completed_at)
            .context("fault journal completed_at must be RFC 3339")?;
        if completed < started {
            bail!("fault journal completed_at precedes started_at");
        }
        if let Some(value) = self.observed_work_amplification {
            if !value.is_finite() || value < 0.0 {
                bail!("observed_work_amplification must be finite and non-negative");
            }
        }
        if let Some(value) = &self.results_sha256 {
            validate_sha256(value, "results_sha256")?;
        }
        let mut ids = BTreeSet::new();
        for action in &self.actions {
            validate_sha256(&action.action_id, "fault observation action_id")?;
            if !ids.insert(&action.action_id) {
                bail!("fault journal action ids must be unique");
            }
            validate_optional_time(action.applied_at.as_deref(), "applied_at")?;
            validate_optional_time(action.recovered_at.as_deref(), "recovered_at")?;
            match action.status {
                FaultObservationStatus::Recovered => {
                    let applied = action
                        .applied_at
                        .as_deref()
                        .context("recovered action is missing applied_at")?;
                    let recovered = action
                        .recovered_at
                        .as_deref()
                        .context("recovered action is missing recovered_at")?;
                    if DateTime::parse_from_rfc3339(recovered)?
                        < DateTime::parse_from_rfc3339(applied)?
                    {
                        bail!("fault action recovered_at precedes applied_at");
                    }
                }
                FaultObservationStatus::Unrecovered if action.applied_at.is_none() => {
                    bail!("unrecovered action is missing applied_at");
                }
                _ => {}
            }
        }
        if self.cleanup_completed && self.cleanup_evidence.is_empty() {
            bail!("completed cleanup requires at least one evidence marker");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryClassification {
    CorrectRecovery,
    ExcessiveRecovery,
    IncorrectResult,
    StructuralFailure,
    InfrastructureFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultEvaluation {
    pub execution_id: String,
    pub profile_id: String,
    pub profile_sha256: String,
    pub classification: RecoveryClassification,
    pub deliverable_correct: Option<bool>,
    pub structural_integrity: Option<bool>,
    pub cleanup_completed: bool,
    pub cancellation_observed: bool,
    pub observed_work_amplification: Option<f64>,
    pub max_work_amplification: f64,
    pub planned_actions: u32,
    pub recovered_actions: u32,
    pub results_sha256: Option<String>,
    pub reasons: Vec<String>,
}

impl FaultEvaluation {
    pub fn evaluate(
        profile: &FaultProfile,
        plan: &FaultPlan,
        journal: &FaultJournal,
        report: Option<&E2eReport>,
    ) -> Result<Self> {
        profile.validate()?;
        plan.validate()?;
        journal.validate()?;
        let profile_sha256 = profile.sha256()?;
        if plan.profile_id != profile.profile_id
            || plan.profile_sha256 != profile_sha256
            || plan.seed != profile.seed
            || plan.expected_outcome != profile.expected_outcome
        {
            bail!("fault plan does not match its profile");
        }
        let expected_plan = profile.materialize()?;
        if serde_json::to_value(plan)? != serde_json::to_value(expected_plan)? {
            bail!("fault plan actions differ from deterministic profile materialization");
        }
        if journal.profile_sha256 != profile_sha256 {
            bail!("fault journal does not match its profile");
        }
        let planned = plan
            .actions
            .iter()
            .map(|action| (action.action_id.as_str(), action.kind))
            .collect::<BTreeMap<_, _>>();
        let observed = journal
            .actions
            .iter()
            .map(|action| (action.action_id.as_str(), action))
            .collect::<BTreeMap<_, _>>();
        if planned.len() != observed.len() || planned.keys().ne(observed.keys()) {
            bail!("fault journal action set differs from the materialized plan");
        }
        for (action_id, kind) in &planned {
            if observed[action_id].kind != *kind {
                bail!("fault journal action kind differs for {action_id}");
            }
        }

        let mut reasons = Vec::new();
        let mut infrastructure_failure = !journal.infrastructure_failures.is_empty();
        reasons.extend(journal.infrastructure_failures.iter().cloned());
        for observation in &journal.actions {
            match observation.status {
                FaultObservationStatus::InjectorFailed => {
                    infrastructure_failure = true;
                    reasons.push(format!("injector failed action {}", observation.action_id));
                }
                FaultObservationStatus::NotTriggered => {
                    infrastructure_failure = true;
                    reasons.push(format!(
                        "planned action {} was not triggered",
                        observation.action_id
                    ));
                }
                FaultObservationStatus::Unrecovered => reasons.push(format!(
                    "subject did not recover from action {}",
                    observation.action_id
                )),
                FaultObservationStatus::Recovered => {}
            }
        }

        let recovered_actions = journal
            .actions
            .iter()
            .filter(|action| action.status == FaultObservationStatus::Recovered)
            .count() as u32;
        let mut deliverable_correct = None;
        let mut structural_integrity = None;
        let mut results_sha256 = None;
        let mut report_work_amplification = None;
        if let Some(report) = report {
            let execution = &report.execution;
            if execution.execution_id != journal.execution_id {
                bail!("fault journal and results execution ids differ");
            }
            let sha = artifact::sha256_value(report)?;
            if journal.results_sha256.as_deref() != Some(sha.as_str()) {
                bail!("fault journal results hash does not match canonical results");
            }
            results_sha256 = Some(sha);
            deliverable_correct = dimension_outcome(report, EvaluationDimension::Deliverable);
            structural_integrity =
                dimension_outcome(report, EvaluationDimension::StructuralIntegrity);
            report_work_amplification = maximum_work_amplification(report);
        }

        match profile.expected_outcome {
            ExpectedTerminalOutcome::Recovered if report.is_none() => {
                reasons.push("recovery produced no canonical results".into());
            }
            ExpectedTerminalOutcome::Recovered if deliverable_correct.is_none() => {
                infrastructure_failure = true;
                reasons.push("canonical results have no deliverable outcome".into());
            }
            ExpectedTerminalOutcome::Recovered if structural_integrity.is_none() => {
                infrastructure_failure = true;
                reasons.push("canonical results have no structural outcome".into());
            }
            ExpectedTerminalOutcome::Cancelled if !journal.cancellation_observed => {
                reasons.push("cancellation was not observed by the control plane".into());
            }
            _ => {}
        }
        if report.is_none() && journal.results_sha256.is_some() {
            bail!("fault journal declares a results hash but no results were supplied");
        }

        let observed_work_amplification =
            report_work_amplification.or(journal.observed_work_amplification);
        if observed_work_amplification.is_none() {
            infrastructure_failure = true;
            reasons.push("work amplification evidence is unavailable".into());
        }
        if !journal.cleanup_completed {
            reasons.push("cleanup did not complete after the perturbation".into());
        }
        let has_unrecovered = journal
            .actions
            .iter()
            .any(|action| action.status == FaultObservationStatus::Unrecovered);
        let classification = if infrastructure_failure {
            RecoveryClassification::InfrastructureFailure
        } else if !journal.cleanup_completed
            || has_unrecovered
            || structural_integrity == Some(false)
        {
            RecoveryClassification::StructuralFailure
        } else if (profile.expected_outcome == ExpectedTerminalOutcome::Recovered
            && deliverable_correct != Some(true))
            || (profile.expected_outcome == ExpectedTerminalOutcome::Cancelled
                && !journal.cancellation_observed)
        {
            RecoveryClassification::IncorrectResult
        } else if observed_work_amplification
            .is_some_and(|value| value > profile.max_work_amplification)
        {
            reasons.push(format!(
                "work amplification exceeds {:.3}",
                profile.max_work_amplification
            ));
            RecoveryClassification::ExcessiveRecovery
        } else {
            RecoveryClassification::CorrectRecovery
        };
        if reasons.is_empty() {
            reasons.push("all planned perturbations recovered within policy".into());
        }
        Ok(Self {
            execution_id: journal.execution_id.clone(),
            profile_id: profile.profile_id.clone(),
            profile_sha256,
            classification,
            deliverable_correct,
            structural_integrity,
            cleanup_completed: journal.cleanup_completed,
            cancellation_observed: journal.cancellation_observed,
            observed_work_amplification,
            max_work_amplification: profile.max_work_amplification,
            planned_actions: plan.actions.len() as u32,
            recovered_actions,
            results_sha256,
            reasons,
        })
    }

    pub fn passed(&self) -> bool {
        self.classification == RecoveryClassification::CorrectRecovery
    }

    pub fn write(&self, path: &Path) -> Result<PathBuf> {
        write_json(path, self, "fault evaluation")
    }
}

fn dimension_outcome(report: &E2eReport, dimension: EvaluationDimension) -> Option<bool> {
    let runs = report
        .scenarios
        .iter()
        .flat_map(|scenario| &scenario.runs)
        .collect::<Vec<_>>();
    if runs.is_empty() {
        return None;
    }
    runs.into_iter().try_fold(true, |all_passed, run| {
        run.dimensions
            .iter()
            .find(|candidate| candidate.dimension == dimension)
            .and_then(|candidate| candidate.passed)
            .map(|passed| all_passed && passed)
    })
}

fn maximum_work_amplification(report: &E2eReport) -> Option<f64> {
    let runs = report
        .scenarios
        .iter()
        .flat_map(|scenario| &scenario.runs)
        .collect::<Vec<_>>();
    if runs.is_empty() {
        return None;
    }
    runs.into_iter().try_fold(0.0_f64, |maximum, run| {
        run.efficiency
            .as_ref()
            .and_then(|efficiency| efficiency.work_amplification)
            .map(|value| maximum.max(value))
    })
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        bail!("{label} must be a non-empty portable identifier up to 128 bytes");
    }
    Ok(())
}

fn validate_target(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        bail!("{label} must be a non-empty printable value up to 256 bytes");
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} is not a SHA-256 digest");
    }
    Ok(())
}

fn validate_optional_time(value: Option<&str>, label: &str) -> Result<()> {
    if let Some(value) = value {
        DateTime::parse_from_rfc3339(value).with_context(|| format!("{label} must be RFC 3339"))?;
    }
    Ok(())
}

fn digest(value: String) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn read_json(path: &Path, label: &str) -> Result<serde_json::Value> {
    let bytes = fs::read(path).with_context(|| format!("read {label} {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {label} {}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize, label: &str) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {label} directory {}", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).context("serialize JSON")?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {label} {}", path.display()))?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{
        CostReport, DimensionReport, E2eRunReport, E2eScenarioReport, EfficiencyReport,
        ModelArtifact, ObservedComplexityReport, RobustnessReport, RunStatus, ScenarioAggregate,
    };
    use crate::scenarios::ExecutionPolicy;
    use serde_json::json;

    fn profile() -> FaultProfile {
        FaultProfile {
            profile_id: "weekly-l5-recovery".into(),
            seed: 44,
            expected_outcome: ExpectedTerminalOutcome::Recovered,
            delayed_calls: vec![DelayRule {
                target: "harness::send".into(),
                delay_ms: 250,
                occurrences: 2,
            }],
            fail_first_attempt: vec!["child-2".into()],
            duplicate_events: true,
            child_timeout_ms: Some(1_000),
            transient_disconnect: Some(DisconnectRule {
                target: "subject-transport".into(),
                after_action: "first-child-started".into(),
                duration_ms: 500,
            }),
            out_of_order_results: true,
            cancellation: None,
            max_work_amplification: 4.0,
        }
    }

    #[test]
    fn plan_is_deterministic_and_covers_every_fault_kind() {
        let profile = profile();
        let first = profile.materialize().unwrap();
        let second = profile.materialize().unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap()
        );
        assert_eq!(first.actions.len(), 7);
        let kinds = first
            .actions
            .iter()
            .map(|action| action.kind)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            kinds,
            BTreeSet::from([
                FaultActionKind::DelayCall,
                FaultActionKind::FailFirstAttempt,
                FaultActionKind::DuplicateEvent,
                FaultActionKind::ChildTimeout,
                FaultActionKind::TransientDisconnect,
                FaultActionKind::OutOfOrderResult,
            ])
        );
    }

    #[test]
    fn checked_in_weekly_profiles_are_valid_and_materializable() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/profiles");
        for name in [
            "weekly-l4-recovery.json",
            "weekly-l5-recovery.json",
            "weekly-l5-cancellation.json",
        ] {
            let profile = FaultProfile::read(&root.join(name)).unwrap();
            let plan = profile.materialize().unwrap();
            plan.validate().unwrap();
            assert_eq!(plan.profile_id, name.trim_end_matches(".json"));
        }
    }

    #[test]
    fn cancellation_profile_requires_a_cancellation_rule() {
        let mut profile = profile();
        profile.expected_outcome = ExpectedTerminalOutcome::Cancelled;
        assert!(profile.validate().is_err());
        profile.cancellation = Some(CancellationRule {
            after_action: "fail-first-attempt".into(),
            grace_period_ms: 1_000,
        });
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn evaluation_separates_correct_and_expensive_recovery() {
        let profile = profile();
        let plan = profile.materialize().unwrap();
        let report = passing_report(3.0);
        let journal = recovered_journal(&plan, &report);
        let evaluation =
            FaultEvaluation::evaluate(&profile, &plan, &journal, Some(&report)).unwrap();
        assert_eq!(
            evaluation.classification,
            RecoveryClassification::CorrectRecovery
        );

        let report = passing_report(5.0);
        let journal = recovered_journal(&plan, &report);
        let evaluation =
            FaultEvaluation::evaluate(&profile, &plan, &journal, Some(&report)).unwrap();
        assert_eq!(
            evaluation.classification,
            RecoveryClassification::ExcessiveRecovery
        );
    }

    #[test]
    fn cleanup_failure_is_structural_and_injector_failure_is_infrastructure() {
        let profile = profile();
        let plan = profile.materialize().unwrap();
        let report = passing_report(2.0);
        let mut journal = recovered_journal(&plan, &report);
        journal.cleanup_completed = false;
        let evaluation =
            FaultEvaluation::evaluate(&profile, &plan, &journal, Some(&report)).unwrap();
        assert_eq!(
            evaluation.classification,
            RecoveryClassification::StructuralFailure
        );

        journal.cleanup_completed = true;
        journal.actions[0].status = FaultObservationStatus::InjectorFailed;
        let evaluation =
            FaultEvaluation::evaluate(&profile, &plan, &journal, Some(&report)).unwrap();
        assert_eq!(
            evaluation.classification,
            RecoveryClassification::InfrastructureFailure
        );
    }

    #[test]
    fn cancellation_requires_observed_cancel_and_cleanup() {
        let mut profile = profile();
        profile.expected_outcome = ExpectedTerminalOutcome::Cancelled;
        profile.cancellation = Some(CancellationRule {
            after_action: "fail-first-attempt".into(),
            grace_period_ms: 1_000,
        });
        let plan = profile.materialize().unwrap();
        let mut journal = FaultJournal {
            execution_id: "stress-cancel-1".into(),
            lane: "weekly-stress".into(),
            profile_sha256: profile.sha256().unwrap(),
            started_at: "2026-08-12T10:00:00Z".into(),
            completed_at: "2026-08-12T10:01:00Z".into(),
            cleanup_completed: true,
            cancellation_observed: true,
            observed_work_amplification: Some(1.5),
            results_sha256: None,
            actions: recovered_actions(&plan),
            infrastructure_failures: Vec::new(),
            cleanup_evidence: BTreeMap::from([("active_attempt".into(), "none".into())]),
        };
        let evaluation = FaultEvaluation::evaluate(&profile, &plan, &journal, None).unwrap();
        assert_eq!(
            evaluation.classification,
            RecoveryClassification::CorrectRecovery
        );
        journal.cleanup_completed = false;
        let evaluation = FaultEvaluation::evaluate(&profile, &plan, &journal, None).unwrap();
        assert_eq!(
            evaluation.classification,
            RecoveryClassification::StructuralFailure
        );
    }

    fn passing_report(work_amplification: f64) -> E2eReport {
        let run = E2eRunReport {
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            attempt_number: 1,
            session_id: "session-1".into(),
            prompt: "test".into(),
            wall_time_ms: 10,
            score: Some(10),
            status: RunStatus::Passed,
            hard_gates: Vec::new(),
            criteria: Vec::new(),
            transcript: None,
            metrics: None,
            judge_attempts: None,
            judge_usage: None,
            cost: CostReport::default(),
            evidence: Vec::new(),
            deliverables: Vec::new(),
            dimensions: vec![
                DimensionReport {
                    dimension: EvaluationDimension::Deliverable,
                    passed: Some(true),
                    signals: json!({}),
                },
                DimensionReport {
                    dimension: EvaluationDimension::StructuralIntegrity,
                    passed: Some(true),
                    signals: json!({}),
                },
            ],
            efficiency: Some(EfficiencyReport {
                wall_time_ms: 10,
                root_turns: Some(1),
                child_turns: Some(1),
                child_sessions: Some(1),
                function_calls: Some(1),
                function_call_errors: Some(0),
                validation_retries: Some(0),
                transient_resumes: Some(0),
                wake_resumes: Some(0),
                effective_fan_out: Some(1),
                critical_path_ms: Some(10),
                input_tokens: Some(1),
                output_tokens: Some(1),
                total_tokens: Some(2),
                cost_usd: Some(0.01),
                minimum_expected_work: 1,
                observed_work: Some(1),
                work_amplification: Some(work_amplification),
                technical_attempts: 1,
                observed_complexity: ObservedComplexityReport::default(),
                unavailable: BTreeMap::new(),
            }),
            retry_attempts: Vec::new(),
            failures: Vec::new(),
            terminal_status: None,
            assessment_results: Vec::new(),
            asset_assessments: Vec::new(),
            asset_capture_manifest: None,
            asset_redaction: Default::default(),
        };
        let scenario = E2eScenarioReport {
            scenario_id: "coordination.4".into(),
            case_id: "case-1".into(),
            scenario_version: 1,
            case: None,
            execution_policy: ExecutionPolicy {
                max_turns: 10,
                max_output_tokens: Some(1_000),
                max_total_tokens: 2_000,
                stuck_timeout_seconds: 30,
            },
            aggregate: ScenarioAggregate {
                runs: 1,
                scored_runs: 1,
                passed_runs: 1,
                required_passes: 1,
                pass_rate: 1.0,
                median_score: Some(10.0),
                hard_gate_failures: 0,
                technical_failures: 0,
                cost: CostReport::default(),
                robustness: RobustnessReport {
                    sample_size: 1,
                    minimum_sample_size: 1,
                    tail_minimum_sample_size: 1,
                    eligible: true,
                    deliverable_success_rate: Some(1.0),
                    structural_integrity_rate: Some(1.0),
                    technical_failure_rate: Some(0.0),
                    flaky_rate: Some(0.0),
                    median_wall_time_ms: Some(10.0),
                    wall_time_variance: Some(0.0),
                    p95_wall_time_ms: Some(10),
                    cost_per_successful_deliverable: Some(0.01),
                    unavailable: BTreeMap::new(),
                },
            },
            passed: true,
            runs: vec![run],
        };
        E2eReport {
            execution: crate::identity::ExecutionIdentity {
                execution_id: "execution-1".into(),
                lane: "weekly-stress".into(),
                started_at: "2026-08-12T10:00:00Z".into(),
                completed_at: "2026-08-12T10:01:00Z".into(),
            },
            system_under_test: crate::identity::SystemUnderTestIdentity {
                stack: crate::identity::StackIdentity::Source {
                    workers_repository: "iii-hq/workers".into(),
                    workers_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                },
                engine_version: "engine".into(),
                engine_revision: None,
                harness_version: "harness".into(),
                e2e_repository: "iii-hq/harness-e2e".into(),
                e2e_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                contract_hashes: BTreeMap::from([(
                    "harness::send".into(),
                    format!("sha256:{}", "a".repeat(64)),
                )]),
            },
            manifest: None,
            subject: ModelArtifact {
                model: "model".into(),
                provider: "provider".into(),
                context_window: 1,
                max_output_tokens: 1,
                supports_tools: Some(true),
                supports_vision: Some(false),
            },
            judge: None,
            judge_protocol: None,
            engine_revision: None,
            passed: true,
            redaction: Default::default(),
            assessment_contract: crate::assessment::AssessmentContract { runs: Vec::new() },
            scenarios: vec![scenario],
        }
    }

    fn recovered_journal(plan: &FaultPlan, report: &E2eReport) -> FaultJournal {
        FaultJournal {
            execution_id: report.execution.execution_id.clone(),
            lane: "weekly-stress".into(),
            profile_sha256: plan.profile_sha256.clone(),
            started_at: "2026-08-12T10:00:00Z".into(),
            completed_at: "2026-08-12T10:01:00Z".into(),
            cleanup_completed: true,
            cancellation_observed: false,
            observed_work_amplification: None,
            results_sha256: Some(artifact::sha256_value(report).unwrap()),
            actions: recovered_actions(plan),
            infrastructure_failures: Vec::new(),
            cleanup_evidence: BTreeMap::from([("pending_children".into(), "0".into())]),
        }
    }

    fn recovered_actions(plan: &FaultPlan) -> Vec<FaultObservation> {
        plan.actions
            .iter()
            .map(|action| FaultObservation {
                action_id: action.action_id.clone(),
                kind: action.kind,
                status: FaultObservationStatus::Recovered,
                applied_at: Some("2026-08-12T10:00:10Z".into()),
                recovered_at: Some("2026-08-12T10:00:11Z".into()),
                detail: "verified".into(),
            })
            .collect()
    }
}
