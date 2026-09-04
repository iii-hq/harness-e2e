use super::*;
use crate::dashboard::{presenter, store, JobStatus, RunMetadata};
use crate::journal::{
    ExecutionJournalHeader, JournalSlot, JournalTerminalState, EXECUTION_JOURNAL_SCHEMA,
};
use serde_json::json;
use tempfile::TempDir;

const AT: &str = "2026-09-04T12:00:00Z";

fn fixture() -> (TempDir, ExecutionJournal) {
    let root = tempfile::tempdir().unwrap();
    let journal = ExecutionJournal::initialize(
        root.path(),
        &ExecutionJournalHeader {
            schema: EXECUTION_JOURNAL_SCHEMA.into(),
            execution_id: "execution-1".into(),
            request_sha256: "request-sha".into(),
            result_contract_sha256: RESULT_CONTRACT_SHA256.into(),
            scoring_profile_sha256: SCORING_PROFILE_SHA256.into(),
            created_at: AT.into(),
            request: json!({}),
            runner: json!({}),
        },
    )
    .unwrap();
    append(&journal, ExecutionJournalEventKind::ExecutionAdmitted);
    append(
        &journal,
        ExecutionJournalEventKind::SlotInventoryCommitted {
            slots: ["a", "b", "c"]
                .into_iter()
                .enumerate()
                .map(|(ordinal, id)| JournalSlot {
                    slot_id: id.into(),
                    ordinal: ordinal as u64,
                    scenario_id: format!("scenario-{id}"),
                    case_id: format!("case-{id}"),
                    seed: "42".into(),
                    repetition: 0,
                })
                .collect(),
        },
    );
    (root, journal)
}

fn append(journal: &ExecutionJournal, kind: ExecutionJournalEventKind) {
    journal.append(AT.into(), kind).unwrap();
}

fn start(journal: &ExecutionJournal, slot: &str, attempt: &str) {
    append(
        journal,
        ExecutionJournalEventKind::AttemptStarted {
            scenario_id: format!("scenario-{slot}"),
            run_id: format!("run-{slot}"),
            attempt_id: attempt.into(),
            session_id: format!("session-{attempt}"),
        },
    );
}

fn checkpoint(
    root: &Path,
    slot: &str,
    attempt: &str,
    observation: bool,
    overrides: Value,
) -> ArtifactReference {
    let kind = if observation {
        "subject-observation"
    } else {
        "run"
    };
    let mut value = json!({
        "schema": format!("harness-e2e-{kind}-checkpoint/v1"),
        "slot_id": slot, "run_id": format!("run-{slot}"), "attempt_id": attempt,
        "completion": "completed", "technical": "valid", "objective_score": 100,
        "quality_score_completed": 80, "metrics": null, "cost": null,
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    artifact::write_json(
        root,
        Path::new(&format!("journal/{kind}/{attempt}.json")),
        attempt,
        kind,
        &value,
    )
    .unwrap()
}

fn observe(root: &Path, journal: &ExecutionJournal, slot: &str, attempt: &str, metrics: Value) {
    start(journal, slot, attempt);
    append(
        journal,
        ExecutionJournalEventKind::AttemptFinished {
            attempt_id: attempt.into(),
        },
    );
    let artifact = checkpoint(root, slot, attempt, true, json!({ "metrics": metrics }));
    append(
        journal,
        ExecutionJournalEventKind::SubjectObservationCommitted {
            slot_id: slot.into(),
            attempt_id: attempt.into(),
            artifact,
        },
    );
}

fn commit(root: &Path, journal: &ExecutionJournal, slot: &str, attempt: &str, overrides: Value) {
    let artifact = checkpoint(root, slot, attempt, false, overrides);
    append(
        journal,
        ExecutionJournalEventKind::RunCommitted {
            slot_id: slot.into(),
            run_id: format!("run-{slot}"),
            artifact,
        },
    );
}

fn metrics(input: u64, output: u64) -> Value {
    json!({ "complete": true, "totals": { "input_tokens": input, "output_tokens": output } })
}

fn metadata(root: &Path, status: JobStatus) {
    let metadata: RunMetadata = serde_json::from_value(json!({
        "id": "execution-1", "label": "live test", "status": status, "started_at": AT,
        "completed_at": "", "returncode": null, "error": "",
        "request": { "url": "ws://localhost:49134", "model": "model", "provider": "provider",
            "scenarios": ["direct_answer"], "runs": 1, "technical_retries": 1 }
    }))
    .unwrap();
    store::write_metadata(root, &metadata).unwrap();
}

#[test]
fn live_projection_preserves_pending_slots_and_counts_retries_once() {
    let (root, journal) = fixture();
    observe(root.path(), &journal, "a", "a-retry", metrics(100, 20));
    observe(root.path(), &journal, "a", "a-final", metrics(200, 30));
    commit(
        root.path(),
        &journal,
        "a",
        "a-final",
        json!({ "cost": { "total_usd": 0.3 }, "metrics": metrics(200, 30) }),
    );
    observe(root.path(), &journal, "b", "b-final", metrics(40, 10));
    commit(
        root.path(),
        &journal,
        "b",
        "b-final",
        json!({
            "completion": "task_incomplete", "quality_score_completed": null,
            "objective_score": 20, "cost": { "total_usd": 0.1 }
        }),
    );
    start(&journal, "c", "c-active");
    append(
        &journal,
        ExecutionJournalEventKind::PhaseChanged {
            phase: "execute".into(),
            reason: "".into(),
        },
    );
    let live = read(root.path(), "execution-1").unwrap().unwrap();
    assert_eq!(
        (live.journal.runs_committed, live.journal.planned_slots),
        (2, 3)
    );
    assert_eq!(
        (
            live.journal.attempts_finished,
            live.journal.attempts_started
        ),
        (3, 4)
    );
    assert_eq!(live.observed_tokens, Some(400));
    assert_eq!(live.token_observed_attempts, 3);
    assert_eq!(live.observed_cost_usd, Some(0.4));
    assert_eq!(live.cost_observed_runs, 2);
    assert_eq!(live.completion_rate, Some(0.5));
    assert_eq!(live.quality_score_completed, Some(80.0));
    assert_eq!(live.quality_scored_completed_runs, 1);
    assert_eq!(live.active_attempt.unwrap().attempt_id, "c-active");
    assert_eq!(live.slots[2].state, "pending");
    assert!(live.slots[2].completion.is_none());
    assert_eq!(live.technical_invalid_runs, 0);
}

#[test]
fn unknown_telemetry_stays_null_and_uncommitted_artifacts_are_ignored() {
    let (root, journal) = fixture();
    observe(
        root.path(),
        &journal,
        "a",
        "a",
        json!({ "complete": false, "totals": { "input_tokens": 50, "output_tokens": 20 } }),
    );
    commit(
        root.path(),
        &journal,
        "a",
        "a",
        json!({ "completion": "undetermined", "technical": "technical_invalid", "objective_score": null, "quality_score_completed": null }),
    );
    checkpoint(
        root.path(),
        "b",
        "uncommitted",
        false,
        json!({ "cost": { "total_usd": 5 } }),
    );
    let live = read(root.path(), "execution-1").unwrap().unwrap();
    assert_eq!(live.observed_tokens, None);
    assert_eq!(live.token_observed_attempts, 0);
    assert_eq!(live.observed_cost_usd, None);
    assert_eq!(live.cost_observed_runs, 0);
    assert_eq!(live.completion_rate, None);
    assert_eq!(live.quality_score_completed, None);
    assert_eq!(live.undetermined_runs, 1);
    assert_eq!(live.technical_invalid_runs, 1);
    assert_eq!(live.journal.runs_committed, 1);
    assert_eq!(live.slots[1].state, "pending");
    assert_eq!(tokens(&metrics(0, 0)), Some(0));
    assert_eq!(
        tokens(&json!({"complete": true, "totals": {"input_tokens": 1}})),
        None
    );
}

#[test]
fn cancelled_execution_retains_partial_evidence_in_both_api_projections() {
    let (root, journal) = fixture();
    metadata(root.path(), JobStatus::Cancelled);
    commit(root.path(), &journal, "a", "a", json!({}));
    for id in ["b", "c"] {
        append(
            &journal,
            ExecutionJournalEventKind::SlotDeferred {
                slot_id: id.into(),
                reason: "cancelled".into(),
            },
        );
    }
    append(
        &journal,
        ExecutionJournalEventKind::ExecutionFinalized {
            state: JournalTerminalState::Cancelled,
            reason: "user_cancelled".into(),
        },
    );
    let stored = store::read_stored_run(root.path()).unwrap().unwrap();
    for value in [
        presenter::stored_execution_summary(&stored).unwrap(),
        presenter::stored_execution_detail(&stored).unwrap(),
    ] {
        assert_eq!(value["status"], "cancelled");
        assert_eq!(value["live_progress"]["runs_committed"], 1);
        assert_eq!(value["live_progress"]["slots_deferred"], 2);
        assert_eq!(value["live_progress"]["terminal"], true);
        assert_eq!(value["live_progress"]["terminal_reason"], "user_cancelled");
        assert!(value["live_progress_error"].is_null());
    }
}

#[test]
fn corrupt_checkpoint_is_not_displayed_but_execution_stays_visible() {
    let (root, journal) = fixture();
    metadata(root.path(), JobStatus::Running);
    commit(root.path(), &journal, "a", "a", json!({}));
    fs::write(root.path().join("journal/run/a.json"), "{}").unwrap();
    let stored = store::read_stored_run(root.path()).unwrap().unwrap();
    let value = presenter::stored_execution_detail(&stored).unwrap();
    assert_eq!(value["status"], "running");
    assert!(value["live_progress"].is_null());
    assert!(value["live_progress_error"]
        .as_str()
        .unwrap()
        .contains("verified"));
}

#[test]
fn executions_without_a_journal_remain_readable_and_wrong_identity_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    assert!(read(root.path(), "execution-1").unwrap().is_none());
    metadata(root.path(), JobStatus::Running);
    assert!(store::read_stored_run(root.path())
        .unwrap()
        .unwrap()
        .live_progress
        .is_none());
    let (root, _) = fixture();
    assert!(read(root.path(), "wrong-execution").is_err());
}
