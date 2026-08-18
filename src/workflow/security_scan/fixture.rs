use super::*;

#[derive(Default)]
pub(super) struct FixtureState {
    inner: Mutex<FixtureStateInner>,
}

#[derive(Default)]
pub(super) struct FixtureStateInner {
    pub(super) path: Option<PathBuf>,
    pub(super) initial_head: Option<String>,
    pub(super) commit_b_ref: Option<String>,
    pub(super) commit_b_sha: Option<String>,
    pub(super) suggest_expected: bool,
}

pub(super) struct SecurityReviewCleanup {
    pub(super) fixture: Arc<FixtureState>,
}

#[async_trait]
impl WorkflowCleanupHook for SecurityReviewCleanup {
    async fn cleanup(&self, _context: &WorkflowCleanupContext) -> Result<()> {
        self.fixture.restore().await
    }
}

impl FixtureState {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, FixtureStateInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn path(&self) -> Result<PathBuf> {
        self.lock()
            .path
            .clone()
            .context("fixture preflight has not run")
    }

    pub(super) async fn restore(&self) -> Result<()> {
        let (path, initial, commit_b_ref) = {
            let fixture = self.lock();
            (
                fixture.path.clone(),
                fixture.initial_head.clone(),
                fixture.commit_b_ref.clone(),
            )
        };
        let (Some(path), Some(initial)) = (path, initial) else {
            return Ok(());
        };
        let current = git(&path, &["rev-parse", "HEAD"]).await?;
        if current != initial {
            git(&path, &["reset", "--hard", &initial]).await?;
        }
        if let Some(reference) = commit_b_ref {
            let _ = git(
                &path,
                &["update-ref", "-d", &format!("refs/heads/{reference}")],
            )
            .await;
        }
        let marker = path.join("security-scan-e2e-commit-b.txt");
        if marker.exists() {
            std::fs::remove_file(&marker)
                .with_context(|| format!("remove {}", marker.display()))?;
        }
        ensure_clean(&path).await
    }
}
