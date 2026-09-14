//! Materialize reviewed Git bundles without network access or shared worktrees.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

pub(crate) const SHARED_BUNDLE: &[u8] =
    include_bytes!("../../tests/fixtures/campaign/shared-fixture.bundle");
const PREPARATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct PreparedFixture {
    pub root: PathBuf,
    // External engineering fixtures remain owned by the protected launcher.
    pub owned: Option<tempfile::TempDir>,
}

pub(crate) async fn prepare(bundle: &[u8], revision: &str) -> Result<PreparedFixture> {
    let owned = tempfile::Builder::new()
        .prefix("harness-e2e-fixture-")
        .tempdir()
        .context("create isolated fixture directory")?;
    prepare_owned(bundle, revision, owned).await
}

pub(crate) async fn prepare_owned(
    bytes: &[u8],
    revision: &str,
    owned: tempfile::TempDir,
) -> Result<PreparedFixture> {
    let directory = owned.path().canonicalize()?;
    let bundle = directory.join("repository.bundle");
    std::fs::write(&bundle, bytes).context("materialize embedded fixture bundle")?;
    let root = directory.join("repository");
    std::fs::create_dir(&root)?;

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
    .context("automatic fixture preparation timed out after 30 seconds")??;

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
        .with_context(|| format!("automatic fixture: start git {}", args[0]))?;
    if !output.status.success() {
        bail!(
            "automatic fixture: git {} failed: {}",
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
    async fn shared_bundle_checkouts_are_independent_and_removed_on_drop() {
        let revision = "16f6b9e05e34e09c824191eed0631d77f85be6a9";
        let (first, second) = tokio::join!(
            prepare(SHARED_BUNDLE, revision),
            prepare(SHARED_BUNDLE, revision),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_ne!(first.root, second.root);
        let relative = "trends/feed.json";
        let expected = std::fs::read(second.root.join(relative)).unwrap();
        std::fs::write(first.root.join(relative), b"modified").unwrap();
        assert_eq!(std::fs::read(second.root.join(relative)).unwrap(), expected);
        let first_directory = first.root.parent().unwrap().to_path_buf();
        let second_directory = second.root.parent().unwrap().to_path_buf();
        drop(first);
        assert!(!first_directory.exists());
        assert!(second.root.exists());
        drop(second);
        assert!(!second_directory.exists());
    }
}
