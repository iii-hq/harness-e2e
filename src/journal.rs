use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact;

pub const EXECUTION_JOURNAL_SCHEMA: &str = "harness-e2e-execution-journal/v1";
static JOURNAL_APPEND_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionJournalHeader {
    pub schema: String,
    pub execution_id: String,
    pub request_sha256: String,
    pub result_contract_sha256: String,
    pub scoring_profile_sha256: String,
    pub created_at: String,
    pub request: Value,
    pub runner: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JournalTerminalState {
    Completed,
    Failed,
    Cancelled,
    NeedsReconciliation,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionJournalEventKind {
    ExecutionAdmitted,
    SlotInventoryCommitted {
        slots: Vec<JournalSlot>,
    },
    PhaseChanged {
        phase: String,
        reason: String,
    },
    AttemptStarted {
        scenario_id: String,
        run_id: String,
        attempt_id: String,
        session_id: String,
    },
    AttemptCheckpointed {
        attempt_id: String,
        state_sha256: String,
    },
    SubjectObservationCommitted {
        slot_id: String,
        attempt_id: String,
        artifact: crate::artifact::ArtifactReference,
    },
    RunCommitted {
        slot_id: String,
        run_id: String,
        artifact: crate::artifact::ArtifactReference,
    },
    SlotDeferred {
        slot_id: String,
        reason: String,
    },
    AttemptFinished {
        attempt_id: String,
    },
    ExecutionStopped {
        reason: String,
    },
    ExecutionFinalized {
        state: JournalTerminalState,
        reason: String,
    },
}

impl ExecutionJournalEventKind {
    fn file_label(&self) -> &'static str {
        match self {
            Self::ExecutionAdmitted => "execution-admitted",
            Self::SlotInventoryCommitted { .. } => "slot-inventory-committed",
            Self::PhaseChanged { .. } => "phase-changed",
            Self::AttemptStarted { .. } => "attempt-started",
            Self::AttemptCheckpointed { .. } => "attempt-checkpointed",
            Self::SubjectObservationCommitted { .. } => "subject-observation-committed",
            Self::RunCommitted { .. } => "run-committed",
            Self::SlotDeferred { .. } => "slot-deferred",
            Self::AttemptFinished { .. } => "attempt-finished",
            Self::ExecutionStopped { .. } => "execution-stopped",
            Self::ExecutionFinalized { .. } => "execution-finalized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalSlot {
    pub slot_id: String,
    pub ordinal: u64,
    pub scenario_id: String,
    pub case_id: String,
    pub seed: String,
    pub repetition: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionJournalEvent {
    pub sequence: u64,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_sha256: Option<String>,
    #[serde(flatten)]
    pub kind: ExecutionJournalEventKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JournalProgress {
    pub committed_events: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_attempt_id: Option<String>,
    pub attempts_started: u64,
    pub attempts_finished: u64,
    pub planned_slots: u64,
    pub subject_observations_committed: u64,
    pub runs_committed: u64,
    pub slots_deferred: u64,
    pub terminal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<JournalTerminalState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecutionJournal {
    root: PathBuf,
}

impl ExecutionJournal {
    pub fn initialize(root: &Path, header: &ExecutionJournalHeader) -> Result<Self> {
        if header.schema != EXECUTION_JOURNAL_SCHEMA {
            bail!("unsupported execution journal schema {}", header.schema);
        }
        if header.execution_id.trim().is_empty()
            || header.request_sha256.trim().is_empty()
            || header.result_contract_sha256.trim().is_empty()
            || header.scoring_profile_sha256.trim().is_empty()
        {
            bail!("execution journal identity must be non-empty");
        }
        let journal = Self {
            root: root.join("journal"),
        };
        fs::create_dir_all(journal.events_dir())
            .with_context(|| format!("create {}", journal.events_dir().display()))?;
        write_immutable_json(&journal.root.join("header.json"), header)?;
        journal.read_header()?;
        Ok(journal)
    }

    pub fn open(root: &Path) -> Result<Self> {
        let journal = Self {
            root: root.join("journal"),
        };
        journal.read_header()?;
        Ok(journal)
    }

    pub fn read_header(&self) -> Result<ExecutionJournalHeader> {
        let path = self.root.join("header.json");
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let header: ExecutionJournalHeader =
            serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))?;
        if header.schema != EXECUTION_JOURNAL_SCHEMA {
            bail!("unsupported execution journal schema {}", header.schema);
        }
        Ok(header)
    }

    pub fn append(&self, at: String, kind: ExecutionJournalEventKind) -> Result<JournalProgress> {
        let _guard = JOURNAL_APPEND_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| anyhow::anyhow!("execution journal append lock is poisoned"))?;
        let progress = self.replay()?;
        if progress.terminal {
            bail!("cannot append to a finalized execution journal");
        }
        let sequence = progress.committed_events + 1;
        let event = ExecutionJournalEvent {
            sequence,
            at,
            previous_sha256: progress.last_event_sha256,
            kind,
        };
        let path = self
            .events_dir()
            .join(format!("{sequence:08}-{}.json", event.kind.file_label()));
        write_immutable_json(&path, &event)?;
        self.replay()
    }

    pub fn replay(&self) -> Result<JournalProgress> {
        self.read_header()?;
        let mut paths = fs::read_dir(self.events_dir())
            .with_context(|| format!("read {}", self.events_dir().display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();

        let mut progress = JournalProgress::default();
        let mut inventory: Option<HashSet<String>> = None;
        let mut started_attempts = HashSet::new();
        let mut finished_attempts = HashSet::new();
        let mut observed_attempts = HashSet::new();
        let mut disposed_slots = HashSet::new();
        for path in paths {
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let event: ExecutionJournalEvent = serde_json::from_slice(&bytes)
                .with_context(|| format!("decode {}", path.display()))?;
            let expected = progress.committed_events + 1;
            if event.sequence != expected {
                bail!(
                    "execution journal sequence gap: expected {expected}, observed {}",
                    event.sequence
                );
            }
            if event.previous_sha256 != progress.last_event_sha256 {
                bail!("execution journal hash chain mismatch at sequence {expected}");
            }
            match &event.kind {
                ExecutionJournalEventKind::SlotInventoryCommitted { slots } => {
                    let ids = slots
                        .iter()
                        .map(|slot| slot.slot_id.clone())
                        .collect::<HashSet<_>>();
                    let ordinals = slots
                        .iter()
                        .map(|slot| slot.ordinal)
                        .collect::<HashSet<_>>();
                    if inventory.is_some()
                        || ids.len() != slots.len()
                        || ordinals.len() != slots.len()
                    {
                        bail!("execution journal slot inventory is duplicate or ambiguous");
                    }
                    inventory = Some(ids);
                }
                ExecutionJournalEventKind::AttemptStarted { attempt_id, .. } => {
                    if !started_attempts.insert(attempt_id.clone()) {
                        bail!("execution journal repeats attempt '{attempt_id}'");
                    }
                }
                ExecutionJournalEventKind::AttemptFinished { attempt_id } => {
                    if !started_attempts.contains(attempt_id)
                        || !finished_attempts.insert(attempt_id.clone())
                    {
                        bail!("execution journal finishes unknown or duplicate attempt '{attempt_id}'");
                    }
                }
                ExecutionJournalEventKind::SubjectObservationCommitted { attempt_id, .. } => {
                    if !finished_attempts.contains(attempt_id)
                        || !observed_attempts.insert(attempt_id.clone())
                    {
                        bail!("execution journal observes unknown or duplicate attempt '{attempt_id}'");
                    }
                }
                ExecutionJournalEventKind::RunCommitted { slot_id, .. }
                | ExecutionJournalEventKind::SlotDeferred { slot_id, .. }
                    if inventory
                        .as_ref()
                        .is_none_or(|slots| !slots.contains(slot_id))
                        || !disposed_slots.insert(slot_id.clone()) =>
                {
                    bail!("execution journal commits unknown or duplicate slot '{slot_id}'");
                }
                _ => {}
            }
            match &event.kind {
                ExecutionJournalEventKind::SubjectObservationCommitted { artifact, .. }
                | ExecutionJournalEventKind::RunCommitted { artifact, .. } => artifact.verify(
                    self.root
                        .parent()
                        .context("execution journal root has no execution parent")?,
                )?,
                _ => {}
            }
            apply_event(&mut progress, &event.kind)?;
            progress.committed_events = event.sequence;
            progress.last_event_sha256 = Some(artifact::sha256_bytes(&bytes));
        }
        Ok(progress)
    }

    fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }
}

fn apply_event(progress: &mut JournalProgress, event: &ExecutionJournalEventKind) -> Result<()> {
    if progress.terminal {
        bail!("execution journal contains events after finalization");
    }
    match event {
        ExecutionJournalEventKind::ExecutionAdmitted => {
            progress.phase = Some("admitted".into());
        }
        ExecutionJournalEventKind::SlotInventoryCommitted { slots } => {
            if progress.planned_slots != 0 {
                bail!("execution journal contains more than one slot inventory");
            }
            progress.planned_slots = slots.len().try_into().unwrap_or(u64::MAX);
        }
        ExecutionJournalEventKind::PhaseChanged { phase, .. } => {
            progress.phase = Some(phase.clone());
        }
        ExecutionJournalEventKind::AttemptStarted { attempt_id, .. } => {
            if progress.active_attempt_id.is_some() {
                bail!("execution journal starts an attempt while another is active");
            }
            progress.active_attempt_id = Some(attempt_id.clone());
            progress.attempts_started += 1;
        }
        ExecutionJournalEventKind::AttemptCheckpointed { attempt_id, .. }
        | ExecutionJournalEventKind::AttemptFinished { attempt_id } => {
            if progress.active_attempt_id.as_deref() != Some(attempt_id.as_str()) {
                bail!("execution journal event does not match the active attempt");
            }
            if matches!(event, ExecutionJournalEventKind::AttemptFinished { .. }) {
                progress.active_attempt_id = None;
                progress.attempts_finished += 1;
            }
        }
        ExecutionJournalEventKind::SubjectObservationCommitted { attempt_id, .. } => {
            if progress.attempts_finished <= progress.subject_observations_committed {
                bail!(
                    "subject observation for attempt '{attempt_id}' has no finished physical attempt"
                );
            }
            progress.subject_observations_committed += 1;
        }
        ExecutionJournalEventKind::RunCommitted { .. } => {
            progress.runs_committed += 1;
        }
        ExecutionJournalEventKind::SlotDeferred { .. } => {
            progress.slots_deferred += 1;
        }
        ExecutionJournalEventKind::ExecutionStopped { reason } => {
            progress.terminal_reason = Some(reason.clone());
        }
        ExecutionJournalEventKind::ExecutionFinalized { state, reason } => {
            progress.phase = Some(
                match state {
                    JournalTerminalState::Completed => "completed",
                    JournalTerminalState::Failed => "failed",
                    JournalTerminalState::Cancelled => "cancelled",
                    JournalTerminalState::NeedsReconciliation => "needs_reconciliation",
                    JournalTerminalState::Unsupported => "unsupported",
                }
                .into(),
            );
            progress.terminal = true;
            progress.terminal_state = Some(state.clone());
            progress.terminal_reason = Some(reason.clone());
            progress.active_attempt_id = None;
        }
    }
    Ok(())
}

fn write_immutable_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .with_context(|| format!("serialize {}", path.display()))?;
    bytes.push(b'\n');
    artifact::write_immutable_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> ExecutionJournalHeader {
        ExecutionJournalHeader {
            schema: EXECUTION_JOURNAL_SCHEMA.into(),
            execution_id: "execution-1".into(),
            request_sha256: "sha256:request".into(),
            result_contract_sha256: crate::report::RESULT_CONTRACT_SHA256.into(),
            scoring_profile_sha256: crate::report::SCORING_PROFILE_SHA256.into(),
            created_at: "2026-09-04T12:00:00Z".into(),
            request: serde_json::json!({"runs": 1}),
            runner: serde_json::json!({"version": "test"}),
        }
    }

    #[test]
    fn journal_replays_a_verified_hash_chain() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        journal
            .append(
                "2026-09-04T12:00:01Z".into(),
                ExecutionJournalEventKind::ExecutionAdmitted,
            )
            .unwrap();
        journal
            .append(
                "2026-09-04T12:00:01Z".into(),
                ExecutionJournalEventKind::SlotInventoryCommitted {
                    slots: vec![JournalSlot {
                        slot_id: "slot".into(),
                        ordinal: 0,
                        scenario_id: "scenario".into(),
                        case_id: "case".into(),
                        seed: "7".into(),
                        repetition: 0,
                    }],
                },
            )
            .unwrap();
        journal
            .append(
                "2026-09-04T12:00:02Z".into(),
                ExecutionJournalEventKind::AttemptStarted {
                    scenario_id: "scenario".into(),
                    run_id: "run".into(),
                    attempt_id: "attempt".into(),
                    session_id: "session".into(),
                },
            )
            .unwrap();
        let progress = journal
            .append(
                "2026-09-04T12:00:03Z".into(),
                ExecutionJournalEventKind::AttemptFinished {
                    attempt_id: "attempt".into(),
                },
            )
            .unwrap();
        assert_eq!(progress.committed_events, 4);
        assert_eq!(progress.planned_slots, 1);
        assert_eq!(progress.attempts_started, 1);
        assert_eq!(progress.attempts_finished, 1);
        assert!(progress.active_attempt_id.is_none());
        assert_eq!(
            ExecutionJournal::open(output.path())
                .unwrap()
                .replay()
                .unwrap(),
            progress
        );
    }

    #[test]
    fn journal_detects_a_corrupted_event() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        journal
            .append(
                "2026-09-04T12:00:01Z".into(),
                ExecutionJournalEventKind::ExecutionAdmitted,
            )
            .unwrap();
        let path = output
            .path()
            .join("journal/events/00000001-execution-admitted.json");
        fs::write(path, b"{}\n").unwrap();
        assert!(journal.replay().unwrap_err().to_string().contains("decode"));
    }

    #[test]
    fn finalized_journal_rejects_new_events() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        journal
            .append(
                "2026-09-04T12:00:01Z".into(),
                ExecutionJournalEventKind::ExecutionFinalized {
                    state: JournalTerminalState::Cancelled,
                    reason: "cancelled".into(),
                },
            )
            .unwrap();
        assert!(journal
            .append(
                "2026-09-04T12:00:02Z".into(),
                ExecutionJournalEventKind::ExecutionAdmitted,
            )
            .unwrap_err()
            .to_string()
            .contains("finalized"));
    }
}
