//! Read-only projection of committed journal evidence, independent of the final report.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::{self, ArtifactReference};
use crate::journal::{
    ExecutionJournal, ExecutionJournalEvent, ExecutionJournalEventKind, JournalProgress,
};
use crate::report::{
    CompletionState, TechnicalState, RESULT_CONTRACT_SHA256, SCORING_PROFILE_SHA256,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize)]
pub(super) struct LiveProgress {
    pub updated_at: String,
    #[serde(flatten)]
    pub journal: JournalProgress,
    pub active_attempt: Option<LiveAttempt>,
    pub slots: Vec<LiveSlot>,
    pub completed_runs: u64,
    pub task_incomplete_runs: u64,
    pub undetermined_runs: u64,
    pub technical_invalid_runs: u64,
    pub completion_rate: Option<f64>,
    pub quality_score_completed: Option<f64>,
    pub quality_scored_completed_runs: u64,
    pub observed_tokens: Option<u64>,
    pub token_observed_attempts: u64,
    pub observed_cost_usd: Option<f64>,
    pub cost_observed_runs: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct LiveAttempt {
    pub scenario_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub session_id: String,
    pub started_at: String,
}

#[derive(Debug, Serialize)]
pub(super) struct LiveSlot {
    pub slot_id: String,
    pub scenario_id: String,
    pub repetition: u32,
    pub state: &'static str,
    pub reason: Option<String>,
    pub run_id: Option<String>,
    pub completion: Option<CompletionState>,
    pub technical: Option<TechnicalState>,
    pub objective_score: Option<u8>,
    pub quality_score_completed: Option<u8>,
}

#[derive(Deserialize)]
struct Checkpoint {
    schema: String,
    slot_id: String,
    run_id: String,
    attempt_id: String,
    completion: CompletionState,
    technical: TechnicalState,
    objective_score: Option<u8>,
    quality_score_completed: Option<u8>,
    metrics: Option<Value>,
    cost: Value,
}

pub(super) fn read(root: &Path, execution_id: &str) -> Result<Option<LiveProgress>> {
    if !root.join("journal/header.json").is_file() {
        return Ok(None);
    }
    let journal = ExecutionJournal::open(root)?;
    let header = journal.read_header()?;
    if header.execution_id != execution_id
        || header.result_contract_sha256 != RESULT_CONTRACT_SHA256
        || header.scoring_profile_sha256 != SCORING_PROFILE_SHA256
    {
        bail!("live progress identity or contract mismatch");
    }
    let verified = journal.replay()?;
    // Read exactly the verified prefix. A writer may append while this read is
    // in flight; an uncommitted artifact must never become a displayed result.
    let mut paths = fs::read_dir(root.join("journal/events"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    let mut events = Vec::new();
    let mut previous_sha256 = None;
    for path in paths.into_iter().take(verified.committed_events as usize) {
        let bytes = fs::read(path)?;
        let event: ExecutionJournalEvent = serde_json::from_slice(&bytes)?;
        if event.sequence != events.len() as u64 + 1 || event.previous_sha256 != previous_sha256 {
            bail!("live progress journal changed while reading");
        }
        previous_sha256 = Some(artifact::sha256_bytes(&bytes));
        events.push(event);
    }
    if events.len() as u64 != verified.committed_events
        || previous_sha256 != verified.last_event_sha256
    {
        bail!("live progress journal changed while reading");
    }
    let mut live = LiveProgress {
        updated_at: events
            .last()
            .map(|event| event.at.clone())
            .unwrap_or(header.created_at),
        journal: verified,
        active_attempt: None,
        slots: Vec::new(),
        completed_runs: 0,
        task_incomplete_runs: 0,
        undetermined_runs: 0,
        technical_invalid_runs: 0,
        completion_rate: None,
        quality_score_completed: None,
        quality_scored_completed_runs: 0,
        observed_tokens: None,
        token_observed_attempts: 0,
        observed_cost_usd: None,
        cost_observed_runs: 0,
    };
    let mut slot_indices = HashMap::new();
    let mut quality = Vec::new();
    for event in events {
        match event.kind {
            ExecutionJournalEventKind::SlotInventoryCommitted { mut slots } => {
                slots.sort_by_key(|slot| slot.ordinal);
                for slot in slots {
                    slot_indices.insert(slot.slot_id.clone(), live.slots.len());
                    live.slots.push(LiveSlot {
                        slot_id: slot.slot_id,
                        scenario_id: slot.scenario_id,
                        repetition: slot.repetition,
                        state: "pending",
                        reason: None,
                        run_id: None,
                        completion: None,
                        technical: None,
                        objective_score: None,
                        quality_score_completed: None,
                    });
                }
            }
            ExecutionJournalEventKind::AttemptStarted {
                scenario_id,
                run_id,
                attempt_id,
                session_id,
            } => {
                live.active_attempt = Some(LiveAttempt {
                    scenario_id,
                    run_id,
                    attempt_id,
                    session_id,
                    started_at: event.at,
                });
            }
            ExecutionJournalEventKind::AttemptFinished { .. }
            | ExecutionJournalEventKind::ExecutionFinalized { .. } => live.active_attempt = None,
            ExecutionJournalEventKind::SubjectObservationCommitted {
                slot_id,
                attempt_id,
                artifact,
            } => {
                let checkpoint = read_checkpoint(
                    root,
                    &artifact,
                    "harness-e2e-subject-observation-checkpoint/v1",
                    &slot_id,
                )?;
                if checkpoint.attempt_id != attempt_id || !slot_indices.contains_key(&slot_id) {
                    bail!("live observation identity mismatch");
                }
                // Raw metrics belong to each physical attempt. Run efficiency
                // and cost can already include retries; summing both doubles usage.
                if let Some(tokens) = checkpoint.metrics.as_ref().and_then(tokens) {
                    live.observed_tokens = Some(
                        live.observed_tokens
                            .unwrap_or(0)
                            .checked_add(tokens)
                            .context("token counter overflow")?,
                    );
                    live.token_observed_attempts += 1;
                }
            }
            ExecutionJournalEventKind::RunCommitted {
                slot_id,
                run_id,
                artifact,
            } => {
                let checkpoint =
                    read_checkpoint(root, &artifact, "harness-e2e-run-checkpoint/v1", &slot_id)?;
                if checkpoint.run_id != run_id {
                    bail!("live run identity mismatch");
                }
                let slot =
                    &mut live.slots[*slot_indices.get(&slot_id).context("unknown live slot")?];
                slot.state = "committed";
                slot.run_id = Some(run_id);
                slot.completion = Some(checkpoint.completion);
                slot.technical = Some(checkpoint.technical);
                slot.objective_score = checkpoint.objective_score;
                slot.quality_score_completed = checkpoint.quality_score_completed;
                match checkpoint.completion {
                    CompletionState::Completed => {
                        live.completed_runs += 1;
                        quality.extend(checkpoint.quality_score_completed.map(f64::from));
                    }
                    CompletionState::TaskIncomplete => live.task_incomplete_runs += 1,
                    CompletionState::Undetermined => live.undetermined_runs += 1,
                }
                if checkpoint.technical == TechnicalState::TechnicalInvalid {
                    live.technical_invalid_runs += 1;
                }
                // A run's cost includes retries. Read it once per logical run.
                if let Some(cost) = checkpoint
                    .cost
                    .get("total_usd")
                    .and_then(Value::as_f64)
                    .filter(|cost| cost.is_finite() && *cost >= 0.0)
                {
                    live.observed_cost_usd = Some(live.observed_cost_usd.unwrap_or(0.0) + cost);
                    live.cost_observed_runs += 1;
                }
            }
            ExecutionJournalEventKind::SlotDeferred { slot_id, reason } => {
                let slot = &mut live.slots[*slot_indices
                    .get(&slot_id)
                    .context("unknown deferred slot")?];
                slot.state = "deferred";
                slot.reason = Some(reason);
            }
            _ => {}
        }
    }
    let determined = live.completed_runs + live.task_incomplete_runs;
    live.completion_rate = (determined > 0).then(|| live.completed_runs as f64 / determined as f64);
    quality.sort_by(f64::total_cmp);
    live.quality_scored_completed_runs = quality.len() as u64;
    if !quality.is_empty() {
        live.quality_score_completed =
            Some((quality[(quality.len() - 1) / 2] + quality[quality.len() / 2]) / 2.0);
    }
    Ok(Some(live))
}

fn tokens(metrics: &Value) -> Option<u64> {
    // Preserve unknown telemetry as unknown, including incomplete session trees.
    if metrics.get("complete")?.as_bool()? {
        metrics
            .pointer("/totals/input_tokens")?
            .as_u64()?
            .checked_add(metrics.pointer("/totals/output_tokens")?.as_u64()?)
    } else {
        None
    }
}

fn read_checkpoint(
    root: &Path,
    reference: &ArtifactReference,
    schema: &str,
    slot: &str,
) -> Result<Checkpoint> {
    reference.verify(root)?;
    let bytes = fs::read(root.join(&reference.path))?;
    if artifact::sha256_bytes(&bytes) != reference.sha256 {
        bail!("live checkpoint changed while reading");
    }
    let checkpoint: Checkpoint = serde_json::from_slice(&bytes)?;
    if checkpoint.schema != schema || checkpoint.slot_id != slot {
        bail!("live checkpoint identity mismatch");
    }
    if checkpoint.objective_score.is_some_and(|score| score > 100)
        || checkpoint
            .quality_score_completed
            .is_some_and(|score| score > 100)
    {
        bail!("live checkpoint score outside 0..100");
    }
    Ok(checkpoint)
}
