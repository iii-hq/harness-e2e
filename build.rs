use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=HARNESS_E2E_BUILD_REPOSITORY");
    println!("cargo:rerun-if-env-changed=HARNESS_E2E_BUILD_REVISION");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let repository_root = Path::new(&manifest_dir).to_path_buf();
    watch_git_revision(&repository_root);
    let repository = env::var("HARNESS_E2E_BUILD_REPOSITORY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git(&repository_root, &["remote", "get-url", "origin"]))
        .map(|value| normalize_repository(&value))
        .unwrap_or_else(|| "iii-hq/harness-e2e".to_string());
    let revision = env::var("HARNESS_E2E_BUILD_REVISION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git(&repository_root, &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=HARNESS_E2E_BUILD_REPOSITORY={repository}");
    println!("cargo:rustc-env=HARNESS_E2E_BUILD_REVISION={revision}");
}

fn watch_git_revision(root: &Path) {
    if let Some(head) = git(root, &["rev-parse", "--git-path", "HEAD"]) {
        println!(
            "cargo:rerun-if-changed={}",
            resolve_git_path(root, &head).display()
        );
    }
    if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(root, &["rev-parse", "--git-path", &reference]) {
            println!(
                "cargo:rerun-if-changed={}",
                resolve_git_path(root, &path).display()
            );
        }
    }
}

fn resolve_git_path<'a>(root: &'a Path, value: &'a str) -> std::borrow::Cow<'a, Path> {
    let path = Path::new(value);
    if path.is_absolute() {
        path.into()
    } else {
        root.join(path).into()
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_repository(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(".git")
        .strip_prefix("git@github.com:")
        .or_else(|| {
            value
                .trim()
                .trim_end_matches(".git")
                .strip_prefix("https://github.com/")
        })
        .unwrap_or(value.trim().trim_end_matches(".git"))
        .to_string()
}
