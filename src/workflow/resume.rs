use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AdaptivePlanRevisionEvidence, ReplayPolicy, WorkflowAttemptReport, WorkflowStepReport,
    WorkflowStepStatus,
};

pub const WORKFLOW_RESUME_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepResumePhase {
    Pending,
    Running,
    Executed,
    Captured,
    Evaluated,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResumeDisposition {
    Active,
    Completed,
    ExplicitlyCancelled,
    NeedsReconciliation,
}

/// Immutable identity supplied by trusted runner code. A resume state is only
/// usable when every identity and contract hash still matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowResumeIdentityV1 {
    pub execution_id: String,
    pub scenario_id: String,
    pub scenario_contract_sha256: String,
    pub workflow_id: String,
    pub workflow_sha256: String,
    pub catalog_sha256: String,
    pub policy_sha256: String,
    pub plan_sha256: String,
    pub system_identity_sha256: String,
    pub model: String,
    pub provider: String,
}

impl WorkflowResumeIdentityV1 {
    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.execution_id, "execution id")?;
        validate_identifier(&self.scenario_id, "scenario id")?;
        validate_identifier(&self.workflow_id, "workflow id")?;
        for (label, digest) in [
            ("scenario contract", &self.scenario_contract_sha256),
            ("workflow", &self.workflow_sha256),
            ("catalog", &self.catalog_sha256),
            ("policy", &self.policy_sha256),
            ("plan", &self.plan_sha256),
            ("system identity", &self.system_identity_sha256),
        ] {
            validate_sha256(digest).with_context(|| format!("validate {label} digest"))?;
        }
        if self.model.trim().is_empty() || self.provider.trim().is_empty() {
            bail!("resume identity requires model and provider");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowResumeStepV1 {
    pub phase: StepResumePhase,
    pub replay_policy: ReplayPolicy,
    pub report: WorkflowStepReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowResumeStateV1 {
    pub schema_version: u32,
    pub sequence: u64,
    pub identity: WorkflowResumeIdentityV1,
    pub run_id: String,
    pub attempt_id: String,
    pub updated_at: String,
    pub disposition: ResumeDisposition,
    #[serde(default)]
    pub explicit_cancellation: bool,
    #[serde(default)]
    pub cleanup_started: bool,
    #[serde(default)]
    pub cleanup_completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_report: Option<Box<WorkflowAttemptReport>>,
    #[serde(default)]
    pub plan_revisions: Vec<AdaptivePlanRevisionEvidence>,
    #[serde(default)]
    pub steps: BTreeMap<String, WorkflowResumeStepV1>,
}

impl WorkflowResumeStateV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != WORKFLOW_RESUME_SCHEMA_VERSION {
            bail!("unsupported workflow resume state schema version");
        }
        self.identity.validate()?;
        validate_identifier(&self.run_id, "run id")?;
        validate_identifier(&self.attempt_id, "attempt id")?;
        if self.sequence == 0 || self.updated_at.trim().is_empty() {
            bail!("workflow resume state requires a positive sequence and timestamp");
        }
        if self.explicit_cancellation && self.disposition != ResumeDisposition::ExplicitlyCancelled
        {
            bail!("explicit cancellation must use explicitly_cancelled disposition");
        }
        if self.cleanup_completed && !self.cleanup_started {
            bail!("completed cleanup requires a durable cleanup-start receipt");
        }
        if self.cleanup_completed && self.disposition == ResumeDisposition::Active {
            bail!("an active resume state cannot have completed cleanup");
        }
        if self.disposition == ResumeDisposition::NeedsReconciliation
            && (self.reconciliation_node_id.is_none() || self.reconciliation_reason.is_none())
        {
            bail!("needs_reconciliation requires a node id and reason");
        }
        if self.disposition == ResumeDisposition::Completed && self.final_report.is_none() {
            bail!("completed resume state requires its final report");
        }
        let mut revisions = HashSet::new();
        for revision in &self.plan_revisions {
            if !revisions.insert(revision.revision) {
                bail!(
                    "resume state contains duplicate plan revision {}",
                    revision.revision
                );
            }
        }
        for (id, step) in &self.steps {
            validate_identifier(id, "resume step id")?;
            if &step.report.node_id != id {
                bail!("resume step key '{id}' does not match its report");
            }
            if step.phase == StepResumePhase::Terminal && !step.report.status.terminal() {
                bail!("terminal resume step '{id}' has a non-terminal report");
            }
            if step.phase != StepResumePhase::Terminal
                && step.report.status.terminal()
                && step.report.status != WorkflowStepStatus::Cancelled
            {
                bail!("non-terminal resume step '{id}' has a terminal report");
            }
        }
        Ok(())
    }

    pub fn canonical_sha256(&self) -> Result<String> {
        crate::artifact::sha256_value(self)
    }

    pub(crate) fn advance(&mut self) {
        self.sequence = self.sequence.saturating_add(1);
        self.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowResumeEnvelopeV1 {
    pub state_sha256: String,
    pub state: WorkflowResumeStateV1,
}

/// Private runner store. Its location is derived from trusted identities and
/// is deliberately not represented by ArtifactReference or exposed in the
/// evidence checkpoint.
#[derive(Debug, Clone)]
pub struct WorkflowResumeStore {
    state_root: PathBuf,
    relative_path: PathBuf,
}

impl WorkflowResumeStore {
    pub fn new(
        state_root: impl AsRef<Path>,
        execution_id: &str,
        run_id: &str,
        attempt_id: &str,
    ) -> Result<Self> {
        validate_identifier(execution_id, "execution id")?;
        validate_identifier(run_id, "run id")?;
        validate_identifier(attempt_id, "attempt id")?;
        Ok(Self {
            state_root: state_root.as_ref().to_path_buf(),
            relative_path: PathBuf::from("workflow-resume")
                .join(execution_id)
                .join(run_id)
                .join(attempt_id)
                .join("state-v1.json"),
        })
    }

    pub fn path(&self) -> PathBuf {
        self.state_root.join(&self.relative_path)
    }

    pub fn persist(&self, state: &WorkflowResumeStateV1) -> Result<String> {
        state.validate()?;
        let state_sha256 = state.canonical_sha256()?;
        let envelope = WorkflowResumeEnvelopeV1 {
            state_sha256: state_sha256.clone(),
            state: state.clone(),
        };
        let mut bytes =
            serde_json::to_vec_pretty(&envelope).context("encode workflow resume state")?;
        bytes.push(b'\n');
        let path = self.path();
        let parent = path.parent().context("resume state path has no parent")?;
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        if let Ok(bytes) = fs::read(&path) {
            let existing: WorkflowResumeEnvelopeV1 = serde_json::from_slice(&bytes)
                .with_context(|| format!("decode existing {}", path.display()))?;
            let observed = existing.state.canonical_sha256()?;
            if observed != existing.state_sha256 {
                bail!("refusing to replace a corrupted workflow resume state");
            }
            if existing.state.sequence > state.sequence
                || (existing.state.sequence == state.sequence
                    && existing.state_sha256 != state_sha256)
            {
                bail!("refusing a stale or conflicting workflow resume state write");
            }
            if existing.state_sha256 == state_sha256 {
                return Ok(state_sha256);
            }
        }
        let temporary = path.with_file_name(".state-v1.json.tmp");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(state_sha256)
    }

    pub fn load(
        &self,
        expected: &WorkflowResumeIdentityV1,
    ) -> Result<Option<WorkflowResumeStateV1>> {
        expected.validate()?;
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        let envelope: WorkflowResumeEnvelopeV1 =
            serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))?;
        envelope.state.validate()?;
        let observed = envelope.state.canonical_sha256()?;
        if observed != envelope.state_sha256 {
            bail!("workflow resume state digest mismatch");
        }
        if &envelope.state.identity != expected {
            bail!("workflow resume identity or immutable contract hash changed");
        }
        Ok(Some(envelope.state))
    }
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !valid || value == "." || value == ".." {
        bail!("{label} '{value}' is invalid");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        bail!("digest must use sha256:<hex>");
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("digest must contain exactly 64 hexadecimal characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn identity() -> WorkflowResumeIdentityV1 {
        WorkflowResumeIdentityV1 {
            execution_id: "execution-1".into(),
            scenario_id: "adaptive.test".into(),
            scenario_contract_sha256: digest('1'),
            workflow_id: "adaptive.test".into(),
            workflow_sha256: digest('2'),
            catalog_sha256: digest('3'),
            policy_sha256: digest('4'),
            plan_sha256: digest('5'),
            system_identity_sha256: digest('6'),
            model: "model".into(),
            provider: "provider".into(),
        }
    }

    fn state() -> WorkflowResumeStateV1 {
        WorkflowResumeStateV1 {
            schema_version: 1,
            sequence: 1,
            identity: identity(),
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            updated_at: "2026-08-24T00:00:00.000Z".into(),
            disposition: ResumeDisposition::Active,
            explicit_cancellation: false,
            cleanup_started: false,
            cleanup_completed: false,
            reconciliation_node_id: None,
            reconciliation_reason: None,
            final_report: None,
            plan_revisions: Vec::new(),
            steps: BTreeMap::new(),
        }
    }

    #[test]
    fn private_store_round_trips_and_rejects_identity_drift() {
        let root = tempfile::tempdir().unwrap();
        let store =
            WorkflowResumeStore::new(root.path(), "execution-1", "run-1", "attempt-1").unwrap();
        store.persist(&state()).unwrap();
        let loaded = store.load(&identity()).unwrap().unwrap();
        assert_eq!(loaded.sequence, 1);

        let mut changed = identity();
        changed.plan_sha256 = digest('a');
        assert!(store
            .load(&changed)
            .unwrap_err()
            .to_string()
            .contains("identity"));
    }

    #[test]
    fn store_rejects_tampered_state() {
        let root = tempfile::tempdir().unwrap();
        let store =
            WorkflowResumeStore::new(root.path(), "execution-1", "run-1", "attempt-1").unwrap();
        store.persist(&state()).unwrap();
        let path = store.path();
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("\"provider\": \"provider\"", "\"provider\": \"tampered\"");
        fs::write(path, text).unwrap();
        assert!(store
            .load(&identity())
            .unwrap_err()
            .to_string()
            .contains("digest mismatch"));
    }
}
