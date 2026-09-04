//! Offline, attempt-owned fallback for runs without a launcher-provided fixture.

use super::*;

const BUNDLE: &[u8] = include_bytes!("../../../tests/fixtures/campaign/engineering-ticket.bundle");
const PREPARATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(super) struct PreparedFixture {
    pub root: PathBuf,
    // None means the protected launcher owns the directory, not this process.
    pub owned: Option<tempfile::TempDir>,
}

pub(super) async fn prepare(
    revision: &str,
    override_path: Option<std::ffi::OsString>,
) -> Result<PreparedFixture> {
    if let Some(path) = override_path {
        return Ok(PreparedFixture {
            root: fixture_root(PathBuf::from(path))?,
            owned: None,
        });
    }

    let owned = tempfile::Builder::new()
        .prefix("harness-e2e-engineering-fixture-")
        .tempdir()
        .context("create isolated engineering fixture directory")?;
    prepare_owned(revision, owned).await
}

async fn prepare_owned(revision: &str, owned: tempfile::TempDir) -> Result<PreparedFixture> {
    let directory = owned.path().canonicalize()?;
    let bundle = directory.join("repository.bundle");
    std::fs::write(&bundle, BUNDLE).context("materialize embedded engineering fixture bundle")?;
    let root = directory.join("repository");
    std::fs::create_dir(&root)?;
    validate_fixture_root(&root)?;

    tokio::time::timeout(PREPARATION_TIMEOUT, async {
        // No remote or tracking refs: fetch only the pinned baseline from the
        // runner's own bundle. An empty template excludes host Git hooks.
        prepare_git(&root, &["init", "--quiet", "--template="]).await?;
        prepare_git(&root, &["config", "--local", "core.autocrlf", "false"]).await?;
        prepare_git(
            &root,
            &["config", "--local", "core.hooksPath", ".git/hooks"],
        )
        .await?;
        prepare_git(&root, &["config", "--local", "commit.gpgsign", "false"]).await?;
        prepare_git(&root, &["config", "--local", "user.name", "Harness E2E"]).await?;
        prepare_git(
            &root,
            &[
                "config",
                "--local",
                "user.email",
                "harness-e2e@example.invalid",
            ],
        )
        .await?;
        prepare_git(
            &root,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                bundle
                    .to_str()
                    .context("fixture bundle path must be UTF-8")?,
                revision,
            ],
        )
        .await?;
        prepare_git(
            &root,
            &["checkout", "--quiet", "-b", "e2e/fixture", revision],
        )
        .await
    })
    .await
    .context("automatic engineering fixture preparation timed out after 30 seconds")??;

    Ok(PreparedFixture {
        root,
        owned: Some(owned),
    })
}

async fn prepare_git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("automatic engineering fixture: start git {}", args[0]))?;
    if !output.status.success() {
        bail!(
            "automatic engineering fixture: git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn automatic_fixture_reproduces_reviewed_baseline_and_supports_commits() {
        let task = task_case();
        let fixture = prepare(task.fixture_revision, None).await.unwrap();
        let root = fixture.root.clone();
        let directory = root.parent().unwrap().to_path_buf();
        assert_eq!(root.canonicalize().unwrap(), root);
        let baseline = preflight_fixture(task, &root).await.unwrap();
        assert_eq!(baseline.fixture_head, task.fixture_revision);
        assert_eq!(
            baseline.fixture_manifest_sha256,
            task.fixture_manifest_sha256
        );
        assert!(baseline.expected_failure_observed);
        assert!(git(&root, &["remote"]).await.unwrap().is_empty());
        assert_eq!(refs_snapshot(&root).await.unwrap().lines().count(), 1);
        assert_eq!(
            baseline.initial_symbolic_ref.as_deref(),
            Some("refs/heads/e2e/fixture")
        );
        std::fs::write(root.join(IMPLEMENTATION_PLAN_PATH), "# Plan\n").unwrap();
        git(&root, &["add", IMPLEMENTATION_PLAN_PATH])
            .await
            .unwrap();
        git(&root, &["commit", "-qm", "plan ticket repair"])
            .await
            .unwrap();
        assert_ne!(
            git(&root, &["rev-parse", "HEAD"]).await.unwrap(),
            task.fixture_revision
        );
        drop(fixture);
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn parallel_attempts_have_independent_fixtures() {
        let revision = task_case().fixture_revision;
        let (first, second) = tokio::join!(prepare(revision, None), prepare(revision, None));
        let first = first.unwrap();
        let second = second.unwrap();
        assert_ne!(first.root, second.root);
        std::fs::write(first.root.join("src/cancellation.py"), "broken\n").unwrap();
        assert!(preflight_fixture(task_case(), &first.root).await.is_err());
        preflight_fixture(task_case(), &second.root).await.unwrap();
        drop(first);
        assert!(second.root.exists());
    }

    #[tokio::test]
    async fn explicit_override_is_validated_but_never_owned() {
        let original = prepare(task_case().fixture_revision, None).await.unwrap();
        let override_fixture = prepare(
            task_case().fixture_revision,
            Some(original.root.clone().into_os_string()),
        )
        .await
        .unwrap();
        assert_eq!(override_fixture.root, original.root);
        assert!(override_fixture.owned.is_none());
        drop(override_fixture);
        preflight_fixture(task_case(), &original.root)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_overrides_do_not_silently_fall_back_or_create_directories() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("missing");
        for path in [
            PathBuf::from("relative"),
            PathBuf::new(),
            missing.clone(),
            PathBuf::from("/"),
        ] {
            assert!(
                prepare(task_case().fixture_revision, Some(path.into_os_string()))
                    .await
                    .is_err()
            );
        }
        assert!(!missing.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_override_is_rejected() {
        let original = prepare(task_case().fixture_revision, None).await.unwrap();
        let parent = tempfile::tempdir().unwrap();
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(&original.root, &alias).unwrap();
        assert!(
            prepare(task_case().fixture_revision, Some(alias.into_os_string()))
                .await
                .is_err()
        );
        assert!(original.root.exists());
    }

    #[tokio::test]
    async fn missing_pinned_revision_fails_and_removes_partial_fixture() {
        let owned = tempfile::tempdir().unwrap();
        let directory = owned.path().to_path_buf();
        let error = prepare_owned(&"0".repeat(40), owned).await.unwrap_err();
        assert!(error.to_string().contains("fetch failed"), "{error:#}");
        assert!(!directory.exists());
    }

    #[test]
    fn engineering_scope_fails_closed_without_successful_setup() {
        for scenario_id in [ID, GIT_HANDOFF_ID] {
            assert!(prepared_filesystem_root(scenario_id, "not-prepared").is_err());
        }
        assert!(prepared_filesystem_root("other-scenario", "not-prepared")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn both_scenarios_resolve_only_their_own_prepared_scope() {
        assert_runtime_lifecycle(false).await;
    }

    #[tokio::test]
    async fn both_scenarios_restore_but_do_not_delete_launcher_owned_fixtures() {
        assert_runtime_lifecycle(true).await;
    }

    async fn assert_runtime_lifecycle(launcher_owned: bool) {
        let run_id = uuid::Uuid::new_v4().to_string();
        let task = task_case();
        let mut launchers = Vec::new();
        if launcher_owned {
            launchers.push(prepare(task.fixture_revision, None).await.unwrap());
            launchers.push(prepare(task.fixture_revision, None).await.unwrap());
        }
        let first = prepare(
            task.fixture_revision,
            launchers
                .first()
                .map(|fixture| fixture.root.clone().into_os_string()),
        )
        .await
        .unwrap();
        let second = prepare(
            task.fixture_revision,
            launchers
                .get(1)
                .map(|fixture| fixture.root.clone().into_os_string()),
        )
        .await
        .unwrap();
        let first_root = first.root.clone();
        let second_root = second.root.clone();
        let first_baseline = preflight_fixture(task, &first_root).await.unwrap();
        let second_baseline = preflight_fixture(task, &second_root).await.unwrap();
        let baseline_refs = refs_snapshot(&second_root).await.unwrap();
        let first_evidence = owned_evidence_dir(&run_id).unwrap();
        let second_evidence = git_handoff_owned_evidence_dir(&run_id).unwrap();
        std::fs::create_dir_all(&first_evidence).unwrap();
        std::fs::create_dir_all(&second_evidence).unwrap();
        runtime_registry().lock().unwrap().insert(
            run_id.clone(),
            Arc::new(Mutex::new(RuntimeEvidence {
                root: first.root,
                owned_fixture: first.owned,
                case: task,
                baseline: first_baseline,
                evidence_dir: first_evidence.clone(),
                attempts: Vec::new(),
                infrastructure_error: None,
            })),
        );
        git_handoff_runtime_registry().lock().unwrap().insert(
            run_id.clone(),
            Arc::new(Mutex::new(GitHandoffRuntimeEvidence {
                root: second.root,
                owned_fixture: second.owned,
                case: task,
                baseline: second_baseline,
                baseline_refs,
                evidence_dir: second_evidence.clone(),
                plan_head: None,
                plan_sha256: None,
                attempts: Vec::new(),
                infrastructure_errors: Vec::new(),
            })),
        );
        assert_eq!(
            prepared_filesystem_root(ID, &run_id).unwrap(),
            Some(first_root.clone())
        );
        assert_eq!(
            prepared_filesystem_root(GIT_HANDOFF_ID, &run_id).unwrap(),
            Some(second_root.clone())
        );
        assert!(prepared_filesystem_root(ID, "other-attempt").is_err());
        // Registered auditors retain Arcs after cleanup. They must not retain
        // ownership of temporary workspaces once the attempt is finished.
        let retained_auditor = runtime_registry().lock().unwrap()[&run_id].clone();
        let retained_handoff_auditor =
            git_handoff_runtime_registry().lock().unwrap()[&run_id].clone();
        std::fs::write(first_root.join("src/cancellation.py"), "broken\n").unwrap();
        std::fs::write(second_root.join(IMPLEMENTATION_PLAN_PATH), "# Plan\n").unwrap();
        git(&second_root, &["add", IMPLEMENTATION_PLAN_PATH])
            .await
            .unwrap();
        git(&second_root, &["commit", "-qm", "plan repair"])
            .await
            .unwrap();
        cleanup_fixture(&run_id).await.unwrap();
        git_handoff_cleanup_fixture(&run_id).await.unwrap();
        cleanup_fixture(&run_id).await.unwrap();
        git_handoff_cleanup_fixture(&run_id).await.unwrap();
        assert!(!first_evidence.exists());
        assert!(!second_evidence.exists());
        assert_eq!(first_root.exists(), launcher_owned);
        assert_eq!(second_root.exists(), launcher_owned);
        assert!(retained_auditor.lock().unwrap().owned_fixture.is_none());
        assert!(retained_handoff_auditor
            .lock()
            .unwrap()
            .owned_fixture
            .is_none());
        if launcher_owned {
            preflight_fixture(task, &first_root).await.unwrap();
            preflight_fixture(task, &second_root).await.unwrap();
        }
        assert!(prepared_filesystem_root(ID, &run_id).is_err());
        assert!(prepared_filesystem_root(GIT_HANDOFF_ID, &run_id).is_err());
    }
}
