use super::schemas::{
    ReconcileRequest, ReconcileResponse, ResetRequest, ResetResponse, ValidateResponse,
};
use super::*;

#[derive(Default)]
pub(super) struct IncidentFixtureState {
    inner: Mutex<IncidentFixtureStateInner>,
}

#[derive(Default, Clone)]
pub(super) struct IncidentFixtureStateInner {
    pub path: Option<PathBuf>,
    pub initial_head: Option<String>,
    pub known_good_sha: Option<String>,
    pub incident_sha: Option<String>,
    pub baseline: Option<Value>,
    pub incident_id: Option<String>,
    pub alert_fingerprint: Option<String>,
    pub reproduction: Option<Value>,
    pub evidence_ids: BTreeSet<String>,
    pub triage: Option<Value>,
    pub diagnosis: Option<Value>,
    pub diagnosis_ready: bool,
    pub validation: Option<ValidateResponse>,
    pub terminal_action: Option<String>,
    pub terminal_revision: Option<String>,
    pub final_state: Option<Value>,
}

impl IncidentFixtureState {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, IncidentFixtureStateInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn snapshot(&self) -> IncidentFixtureStateInner {
        self.lock().clone()
    }
}

pub(super) struct IncidentResponseCleanup {
    pub(super) context: Arc<E2eContext>,
    pub(super) fixture: Arc<IncidentFixtureState>,
}

#[async_trait]
impl WorkflowCleanupHook for IncidentResponseCleanup {
    async fn cleanup(&self, context: &WorkflowCleanupContext) -> Result<()> {
        let state = self.fixture.snapshot();
        let Some(path) = state.path else {
            return Ok(());
        };
        let initial = state
            .initial_head
            .context("incident cleanup has no initial HEAD")?;

        if !self.context.function_exists(RESET_FUNCTION).await? {
            bail!("incident fixture reset function is unavailable during cleanup");
        }
        let reset: ResetResponse = self
            .context
            .trigger(
                RESET_FUNCTION,
                ResetRequest {
                    attempt_id: context.attempt_id.clone(),
                    initial_revision: initial.clone(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        let _ = helpers::git(&path, &["reset", "--hard", &initial]).await?;
        let result_root = helpers::result_root(&path, &context.run_id, &context.attempt_id);
        match std::fs::remove_dir_all(&result_root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove attempt-owned incident result files"),
        }
        let empty_parent = path.join(".harness-e2e");
        if empty_parent
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(empty_parent);
        }
        helpers::ensure_clean(&path).await?;
        let head = helpers::git(&path, &["rev-parse", "HEAD"]).await?;
        if head != initial
            || reset.restored_revision != initial
            || !reset.clean
            || reset.active_operations != 0
        {
            bail!("incident fixture cleanup did not restore the exact initial state");
        }
        let reconciled: ReconcileResponse = self
            .context
            .trigger(
                RECONCILE_FUNCTION,
                ReconcileRequest {
                    attempt_id: context.attempt_id.clone(),
                    _caller_worker_id: None,
                },
            )
            .await?;
        if reconciled.active_operations != 0 {
            bail!("incident fixture cleanup left active operations");
        }
        Ok(())
    }
}
