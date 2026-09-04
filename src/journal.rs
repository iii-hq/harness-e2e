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
        let mut state = self.replay_state()?;
        if state.progress.terminal {
            bail!("cannot append to a finalized execution journal");
        }
        let sequence = state.progress.committed_events + 1;
        let event = ExecutionJournalEvent {
            sequence,
            at,
            previous_sha256: state.progress.last_event_sha256.clone(),
            kind,
        };
        let path = self
            .events_dir()
            .join(format!("{sequence:08}-{}.json", event.kind.file_label()));
        let bytes = immutable_json_bytes(&path, &event)?;
        // Use the same transition and artifact checks as recovery before installing
        // the event. A rejected candidate must never become part of the chain.
        state.apply(&event, &bytes, self.execution_root()?)?;
        artifact::write_immutable_atomic(&path, &bytes)?;
        Ok(state.progress)
    }

    pub fn replay(&self) -> Result<JournalProgress> {
        Ok(self.replay_state()?.progress)
    }

    fn replay_state(&self) -> Result<ReplayState> {
        self.read_header()?;
        let mut paths = fs::read_dir(self.events_dir())
            .with_context(|| format!("read {}", self.events_dir().display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();

        let mut state = ReplayState::default();
        for path in paths {
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let event: ExecutionJournalEvent = serde_json::from_slice(&bytes)
                .with_context(|| format!("decode {}", path.display()))?;
            state.apply(&event, &bytes, self.execution_root()?)?;
        }
        Ok(state)
    }

    fn execution_root(&self) -> Result<&Path> {
        self.root
            .parent()
            .context("execution journal root has no execution parent")
    }

    fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }
}

#[derive(Default)]
struct ReplayState {
    progress: JournalProgress,
    inventory: Option<HashSet<String>>,
    started_attempts: HashSet<String>,
    finished_attempts: HashSet<String>,
    observed_attempts: HashSet<String>,
    disposed_slots: HashSet<String>,
}

impl ReplayState {
    fn apply(
        &mut self,
        event: &ExecutionJournalEvent,
        bytes: &[u8],
        execution_root: &Path,
    ) -> Result<()> {
        let expected = self.progress.committed_events + 1;
        if event.sequence != expected {
            bail!(
                "execution journal sequence gap: expected {expected}, observed {}",
                event.sequence
            );
        }
        if event.previous_sha256 != self.progress.last_event_sha256 {
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
                if self.inventory.is_some()
                    || ids.len() != slots.len()
                    || ordinals.len() != slots.len()
                {
                    bail!("execution journal slot inventory is duplicate or ambiguous");
                }
                self.inventory = Some(ids);
            }
            ExecutionJournalEventKind::AttemptStarted { attempt_id, .. } => {
                if !self.started_attempts.insert(attempt_id.clone()) {
                    bail!("execution journal repeats attempt '{attempt_id}'");
                }
            }
            ExecutionJournalEventKind::AttemptFinished { attempt_id } => {
                if !self.started_attempts.contains(attempt_id)
                    || !self.finished_attempts.insert(attempt_id.clone())
                {
                    bail!("execution journal finishes unknown or duplicate attempt '{attempt_id}'");
                }
            }
            ExecutionJournalEventKind::SubjectObservationCommitted { attempt_id, .. } => {
                if !self.finished_attempts.contains(attempt_id)
                    || !self.observed_attempts.insert(attempt_id.clone())
                {
                    bail!("execution journal observes unknown or duplicate attempt '{attempt_id}'");
                }
            }
            ExecutionJournalEventKind::RunCommitted { slot_id, .. }
            | ExecutionJournalEventKind::SlotDeferred { slot_id, .. }
                if self
                    .inventory
                    .as_ref()
                    .is_none_or(|slots| !slots.contains(slot_id))
                    || !self.disposed_slots.insert(slot_id.clone()) =>
            {
                bail!("execution journal commits unknown or duplicate slot '{slot_id}'");
            }
            _ => {}
        }
        match &event.kind {
            ExecutionJournalEventKind::SubjectObservationCommitted { artifact, .. }
            | ExecutionJournalEventKind::RunCommitted { artifact, .. } => {
                artifact.verify(execution_root)?;
            }
            _ => {}
        }
        apply_event(&mut self.progress, &event.kind)?;
        self.progress.committed_events = event.sequence;
        self.progress.last_event_sha256 = Some(artifact::sha256_bytes(bytes));
        Ok(())
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
    artifact::write_immutable_atomic(path, &immutable_json_bytes(path, value)?)
}

fn immutable_json_bytes(path: &Path, value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .with_context(|| format!("serialize {}", path.display()))?;
    bytes.push(b'\n');
    Ok(bytes)
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

    fn slot() -> JournalSlot {
        JournalSlot {
            slot_id: "slot".into(),
            ordinal: 0,
            scenario_id: "scenario".into(),
            case_id: "case".into(),
            seed: "7".into(),
            repetition: 0,
        }
    }

    fn append(journal: &ExecutionJournal, kind: ExecutionJournalEventKind) -> JournalProgress {
        journal.append("2026-09-04T12:00:01Z".into(), kind).unwrap()
    }

    fn admit_slot(journal: &ExecutionJournal) {
        append(journal, ExecutionJournalEventKind::ExecutionAdmitted);
        append(
            journal,
            ExecutionJournalEventKind::SlotInventoryCommitted {
                slots: vec![slot()],
            },
        );
    }

    fn start_attempt(attempt_id: &str) -> ExecutionJournalEventKind {
        ExecutionJournalEventKind::AttemptStarted {
            scenario_id: "scenario".into(),
            run_id: "run".into(),
            attempt_id: attempt_id.into(),
            session_id: "session".into(),
        }
    }

    fn journal_bytes(journal: &ExecutionJournal) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = vec![(
            PathBuf::from("header.json"),
            fs::read(journal.root.join("header.json")).unwrap(),
        )];
        for entry in fs::read_dir(journal.events_dir()).unwrap() {
            let path = entry.unwrap().path();
            files.push((
                path.strip_prefix(&journal.root).unwrap().to_path_buf(),
                fs::read(path).unwrap(),
            ));
        }
        files.sort();
        files
    }

    fn assert_rejected_without_mutation(
        journal: &ExecutionJournal,
        event: ExecutionJournalEventKind,
        expected_error: &str,
    ) {
        let before_progress = journal.replay().unwrap();
        let before_bytes = journal_bytes(journal);
        let error = journal
            .append("2026-09-04T12:00:02Z".into(), event)
            .unwrap_err();
        assert!(
            error.to_string().contains(expected_error),
            "unexpected append error: {error:#}"
        );
        assert_eq!(journal_bytes(journal), before_bytes);
        assert_eq!(journal.replay().unwrap(), before_progress);
    }

    fn evidence(output: &Path, name: &str) -> artifact::ArtifactReference {
        artifact::write_json(
            output,
            Path::new(name),
            name,
            "test-evidence",
            &serde_json::json!({"completed": true}),
        )
        .unwrap()
    }

    #[test]
    fn rejected_inventory_and_slot_events_leave_the_chain_unchanged() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        append(&journal, ExecutionJournalEventKind::ExecutionAdmitted);
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::SlotInventoryCommitted {
                slots: vec![slot(), slot()],
            },
            "inventory is duplicate or ambiguous",
        );
        let mut same_ordinal = slot();
        same_ordinal.slot_id = "another-slot".into();
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::SlotInventoryCommitted {
                slots: vec![slot(), same_ordinal],
            },
            "inventory is duplicate or ambiguous",
        );
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::SlotDeferred {
                slot_id: "slot".into(),
                reason: "no inventory".into(),
            },
            "unknown or duplicate slot",
        );
        append(
            &journal,
            ExecutionJournalEventKind::SlotInventoryCommitted {
                slots: vec![slot()],
            },
        );
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::SlotInventoryCommitted { slots: vec![] },
            "inventory is duplicate or ambiguous",
        );
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::SlotDeferred {
                slot_id: "unknown-slot".into(),
                reason: "invalid slot".into(),
            },
            "unknown or duplicate slot",
        );
        let deferred = ExecutionJournalEventKind::SlotDeferred {
            slot_id: "slot".into(),
            reason: "deadline".into(),
        };
        append(&journal, deferred.clone());
        assert_rejected_without_mutation(&journal, deferred, "unknown or duplicate slot");
        let progress = append(
            &journal,
            ExecutionJournalEventKind::ExecutionFinalized {
                state: JournalTerminalState::Completed,
                reason: "partial execution".into(),
            },
        );
        assert_eq!(progress.committed_events, 4);
        assert_eq!(progress.slots_deferred, 1);
    }

    #[test]
    fn rejected_attempt_events_allow_a_valid_checkpoint_and_finish() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        admit_slot(&journal);
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::AttemptFinished {
                attempt_id: "unknown".into(),
            },
            "unknown or duplicate attempt",
        );
        append(&journal, start_attempt("attempt"));
        assert_rejected_without_mutation(
            &journal,
            start_attempt("overlapping-attempt"),
            "while another is active",
        );
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::AttemptCheckpointed {
                attempt_id: "unknown".into(),
                state_sha256: "sha256:state".into(),
            },
            "does not match the active attempt",
        );
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::SubjectObservationCommitted {
                slot_id: "slot".into(),
                attempt_id: "attempt".into(),
                artifact: evidence(output.path(), "observation.json"),
            },
            "unknown or duplicate attempt",
        );
        let checkpoint = append(
            &journal,
            ExecutionJournalEventKind::AttemptCheckpointed {
                attempt_id: "attempt".into(),
                state_sha256: "sha256:state".into(),
            },
        );
        assert_eq!(checkpoint.active_attempt_id.as_deref(), Some("attempt"));
        assert_eq!(checkpoint.attempts_finished, 0);
        let finished = ExecutionJournalEventKind::AttemptFinished {
            attempt_id: "attempt".into(),
        };
        append(&journal, finished.clone());
        assert_rejected_without_mutation(&journal, finished, "unknown or duplicate attempt");
        assert_rejected_without_mutation(&journal, start_attempt("attempt"), "repeats attempt");
        let observation = ExecutionJournalEventKind::SubjectObservationCommitted {
            slot_id: "slot".into(),
            attempt_id: "attempt".into(),
            artifact: evidence(output.path(), "observation.json"),
        };
        append(&journal, observation.clone());
        assert_rejected_without_mutation(&journal, observation, "unknown or duplicate attempt");
        let committed = ExecutionJournalEventKind::RunCommitted {
            slot_id: "slot".into(),
            run_id: "run".into(),
            artifact: evidence(output.path(), "run.json"),
        };
        append(&journal, committed.clone());
        assert_rejected_without_mutation(&journal, committed, "unknown or duplicate slot");
        let progress = append(
            &journal,
            ExecutionJournalEventKind::ExecutionFinalized {
                state: JournalTerminalState::Completed,
                reason: "completed".into(),
            },
        );
        assert_eq!(progress.committed_events, 8);
        assert_eq!(progress.attempts_started, 1);
        assert_eq!(progress.attempts_finished, 1);
        assert_eq!(progress.subject_observations_committed, 1);
        assert_eq!(progress.runs_committed, 1);
        assert_eq!(journal.replay().unwrap(), progress);
    }

    #[test]
    fn missing_and_invalid_artifacts_are_rejected_before_event_installation() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        admit_slot(&journal);
        append(&journal, start_attempt("attempt"));
        append(
            &journal,
            ExecutionJournalEventKind::AttemptFinished {
                attempt_id: "attempt".into(),
            },
        );
        for is_observation in [true, false] {
            let valid = evidence(
                output.path(),
                if is_observation {
                    "observation.json"
                } else {
                    "run.json"
                },
            );
            let mut missing = valid.clone();
            missing.path = "missing.json".into();
            let mut wrong_hash = valid.clone();
            wrong_hash.sha256 = "sha256:incorrect".into();
            let mut wrong_size = valid.clone();
            wrong_size.size_bytes += 1;
            let mut escaping = valid.clone();
            escaping.path = "../outside.json".into();
            let mut invalid_metadata = valid.clone();
            invalid_metadata.id.clear();
            let event = |artifact| {
                if is_observation {
                    ExecutionJournalEventKind::SubjectObservationCommitted {
                        slot_id: "slot".into(),
                        attempt_id: "attempt".into(),
                        artifact,
                    }
                } else {
                    ExecutionJournalEventKind::RunCommitted {
                        slot_id: "slot".into(),
                        run_id: "run".into(),
                        artifact,
                    }
                }
            };
            for (invalid, expected_error) in [
                (missing, "read artifact"),
                (wrong_hash, "hash does not match"),
                (wrong_size, "size does not match"),
                (escaping, "cannot contain parent"),
                (invalid_metadata, "metadata must be non-empty"),
            ] {
                assert_rejected_without_mutation(&journal, event(invalid), expected_error);
            }
            append(&journal, event(valid));
        }
        let progress = journal.replay().unwrap();
        assert_eq!(progress.committed_events, 6);
        assert_eq!(progress.subject_observations_committed, 1);
        assert_eq!(progress.runs_committed, 1);
    }

    #[test]
    fn append_revalidates_previously_committed_artifacts() {
        let output = tempfile::tempdir().unwrap();
        let journal = ExecutionJournal::initialize(output.path(), &header()).unwrap();
        admit_slot(&journal);
        let reference = evidence(output.path(), "run.json");
        append(
            &journal,
            ExecutionJournalEventKind::RunCommitted {
                slot_id: "slot".into(),
                run_id: "run".into(),
                artifact: reference.clone(),
            },
        );
        let before_bytes = journal_bytes(&journal);
        let artifact_path = output.path().join(&reference.path);
        let original = fs::read(&artifact_path).unwrap();
        fs::write(&artifact_path, vec![b'x'; original.len()]).unwrap();
        let event = ExecutionJournalEventKind::ExecutionFinalized {
            state: JournalTerminalState::Completed,
            reason: "completed".into(),
        };
        assert!(journal
            .append("2026-09-04T12:00:02Z".into(), event.clone())
            .unwrap_err()
            .to_string()
            .contains("hash does not match"));
        assert_eq!(journal_bytes(&journal), before_bytes);
        fs::write(&artifact_path, original).unwrap();
        assert!(append(&journal, event).terminal);
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
        assert_rejected_without_mutation(
            &journal,
            ExecutionJournalEventKind::ExecutionAdmitted,
            "finalized",
        );
    }
}
