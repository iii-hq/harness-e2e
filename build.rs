use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    ensure_dashboard_bundle();
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

fn ensure_dashboard_bundle() {
    for path in [
        "dashboard/src",
        "dashboard/public",
        "dashboard/index.html",
        "dashboard/package.json",
        "dashboard/pnpm-lock.yaml",
        "dashboard/vite.config.ts",
        "dashboard/tsconfig.app.json",
        "dashboard/tsconfig.json",
        "dashboard/tsconfig.node.json",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=SKIP_DASHBOARD_BUILD");
    println!("cargo:rerun-if-env-changed=PNPM");

    let dashboard_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dashboard");
    let dist_index = dashboard_dir.join("dist").join("index.html");
    if dist_index.exists() && dashboard_dist_is_fresh(&dist_index, &dashboard_dir) {
        return;
    }

    if env::var_os("SKIP_DASHBOARD_BUILD").is_some() {
        if !dist_index.exists() {
            panic!(
                "SKIP_DASHBOARD_BUILD is set but {} is missing; build the React app with `pnpm --dir dashboard install && pnpm --dir dashboard build`",
                dist_index.display()
            );
        }
        return;
    }

    let pnpm = locate_pnpm();
    run_pnpm(&pnpm, &dashboard_dir, &["install", "--frozen-lockfile"]);
    run_pnpm(&pnpm, &dashboard_dir, &["build"]);
    if !dist_index.exists() {
        panic!(
            "dashboard build completed without producing {}",
            dist_index.display()
        );
    }
}

fn run_pnpm(pnpm: &Path, dashboard_dir: &Path, args: &[&str]) {
    let status = Command::new(pnpm)
        .args(args)
        .current_dir(dashboard_dir)
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run `pnpm {}` in {}: {error}",
                args.join(" "),
                dashboard_dir.display()
            )
        });
    if !status.success() {
        panic!("`pnpm {}` exited with {status}", args.join(" "));
    }
}

fn dashboard_dist_is_fresh(dist_index: &Path, dashboard_dir: &Path) -> bool {
    let Ok(dist_time) = dist_index
        .metadata()
        .and_then(|metadata| metadata.modified())
    else {
        return false;
    };
    for relative in ["src", "public"] {
        let path = dashboard_dir.join(relative);
        if path.exists() && !subtree_older_than(&path, dist_time) {
            return false;
        }
    }
    for relative in [
        "index.html",
        "package.json",
        "pnpm-lock.yaml",
        "vite.config.ts",
        "tsconfig.app.json",
        "tsconfig.json",
        "tsconfig.node.json",
    ] {
        let path = dashboard_dir.join(relative);
        let Ok(modified) = path.metadata().and_then(|metadata| metadata.modified()) else {
            return false;
        };
        if modified > dist_time {
            return false;
        }
    }
    true
}

fn subtree_older_than(root: &Path, ceiling: SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            return false;
        };
        if metadata.is_dir() {
            if !subtree_older_than(&entry.path(), ceiling) {
                return false;
            }
        } else {
            let Ok(modified) = metadata.modified() else {
                return false;
            };
            if modified > ceiling {
                return false;
            }
        }
    }
    true
}

fn locate_pnpm() -> PathBuf {
    if let Some(explicit) = env::var_os("PNPM") {
        return PathBuf::from(explicit);
    }
    let candidates = if cfg!(windows) {
        ["pnpm.cmd", "pnpm.exe", "pnpm"].as_slice()
    } else {
        ["pnpm"].as_slice()
    };
    let path = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path) {
        for candidate in candidates {
            let resolved = directory.join(candidate);
            if resolved.is_file() {
                return resolved;
            }
        }
    }
    panic!(
        "pnpm was not found on PATH; install Node and pnpm, or set SKIP_DASHBOARD_BUILD=1 after building the React app"
    );
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
