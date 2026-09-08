//! One click for Release Control executions.
//!
//! Release Control dispatches `exact-stack-e2e.yml`; every group of that run
//! is a native `e2e::*` execution whose complete evidence only survives in the
//! run's GitHub Actions artifacts. The root observation bundle carries each
//! group under `<campaign>/groups/<group>/native/<execution-id>/`, which is
//! exactly the run directory shape the dashboard reads. This module lists the
//! recent completed runs, downloads their bundles, and installs those native
//! directories into the runs directory unchanged. Nothing is converted: the
//! same reader that serves the dashboard validates each run before it lands,
//! and an incompatible result contract is reported, never hidden.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::SecondsFormat;
use futures_util::StreamExt as _;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::report::E2eReport;

const REPOSITORY: &str = "iii-hq/harness-e2e";
const WORKFLOW: &str = "exact-stack-e2e.yml";
const API_ROOT: &str = "https://api.github.com";
const CACHE_FILE: &str = "release-control-pulls.json";
const TITLE_PREFIX: &str = "E2E · ";
const ARTIFACT_PREFIX: &str = "e2e-observation-";
const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 50;
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_REASON_CHARS: usize = 400;
/// One click downloads for at most this long; whatever is left is reported as
/// remaining and picked up by the next click, which the cache makes incremental.
const PULL_BUDGET: Duration = Duration::from_secs(90);
const API_TIMEOUT: Duration = Duration::from_secs(120);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);
const TOKEN_VARIABLES: [&str; 3] = ["HARNESS_E2E_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"];

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(super) struct PullRequest {
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
    /// How many recent completed runs to consider (default 10, at most 50).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Pull only the newest run of this Release Control execution.
    #[serde(default)]
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct PullResponse {
    pub runs_dir: String,
    pub repository: String,
    pub workflow: String,
    pub executions: Vec<PulledExecution>,
    /// Runs that were selected but not downloaded within this click's budget.
    pub remaining_runs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(super) struct PulledExecution {
    pub execution_id: String,
    pub run_id: u64,
    pub run_attempt: u32,
    pub url: String,
    pub created_at: String,
    pub pulled_at: String,
    /// The reader that produced these outcomes; a different build re-pulls.
    #[serde(default)]
    pub reader: String,
    pub groups: Vec<PulledGroup>,
    /// The local plan this execution was mirrored into, once adopted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_error: Option<String>,
}

/// What the plan store needs to mirror a pulled execution: each group's round
/// (from its campaign id, `<profile>-rNN`) and the native run it produced.
pub(super) fn adoption_of(
    execution: &PulledExecution,
) -> super::plan_store::ReleaseControlAdoption {
    let groups = execution
        .groups
        .iter()
        .filter_map(|group| {
            let group_id = group.group_id.clone()?;
            let round = group
                .campaign_id
                .as_deref()
                .and_then(|campaign| campaign.rsplit_once("-r"))
                .and_then(|(_, round)| round.parse::<u32>().ok())
                .unwrap_or(1);
            let installed = matches!(group.outcome, Outcome::Imported | Outcome::Exists);
            Some(super::plan_store::AdoptedGroup {
                round,
                group_id,
                native_execution_id: installed
                    .then(|| group.native_execution_id.clone())
                    .flatten(),
                note: (!installed).then(|| group.reason.clone()).flatten(),
            })
        })
        .collect();
    super::plan_store::ReleaseControlAdoption {
        execution_id: execution.execution_id.clone(),
        run_attempt: execution.run_attempt,
        run_url: execution.url.clone(),
        groups,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Imported,
    Exists,
    Unreadable,
    NotImportable,
    Expired,
    /// The run could not be pulled this time (download or install error); it
    /// is not cached, so the next click retries it.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(super) struct PulledGroup {
    pub campaign_id: Option<String>,
    pub group_id: Option<String>,
    pub native_execution_id: Option<String>,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub runner_version: Option<String>,
    pub runner_revision: Option<String>,
    pub schema_version: Option<u64>,
}

impl PulledGroup {
    fn note(outcome: Outcome, reason: &str) -> Self {
        Self {
            campaign_id: None,
            group_id: None,
            native_execution_id: None,
            outcome,
            reason: Some(reason.into()),
            runner_version: None,
            runner_revision: None,
            schema_version: None,
        }
    }
}

pub(super) async fn pull(runs_dir: &Path, request: PullRequest) -> Result<PullResponse> {
    let wanted = request
        .execution_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    if let Some(wanted) = &wanted {
        if execution_id_from_title(&format!("{TITLE_PREFIX}{wanted}")).is_none() {
            bail!("'{wanted}' is not a Release Control execution id");
        }
    }
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let token = token().await.ok_or_else(|| {
        anyhow!(
            "GitHub artifact download needs a token: set HARNESS_E2E_GITHUB_TOKEN \
             (a GitHub token that can read Actions on {REPOSITORY})"
        )
    })?;
    let github = GitHub::new(&token)?;
    fs::create_dir_all(runs_dir).with_context(|| format!("create {}", runs_dir.display()))?;

    let per_page = if wanted.is_some() { MAX_LIMIT } else { limit };
    let runs = github.completed_runs(per_page).await?;
    let mut cache = Cache::load(runs_dir);
    let mut executions = Vec::new();
    let mut remaining_runs = 0;
    let started = std::time::Instant::now();
    for run in runs {
        if wanted
            .as_deref()
            .is_some_and(|wanted| wanted != run.execution_id)
        {
            continue;
        }
        let key = cache_key(&run);
        let cached = cache
            .executions
            .get(&key)
            .filter(|execution| still_installed(runs_dir, execution))
            .cloned();
        let execution = match cached {
            Some(execution) => replayed(execution),
            None if started.elapsed() > PULL_BUDGET => {
                remaining_runs += 1;
                continue;
            }
            None => {
                let pulled = pull_run(&github, runs_dir, &run).await;
                let execution = PulledExecution {
                    execution_id: run.execution_id.clone(),
                    run_id: run.id,
                    run_attempt: run.run_attempt,
                    url: run.url.clone(),
                    created_at: run.created_at.clone(),
                    pulled_at: now(),
                    reader: reader_identity(),
                    plan_id: None,
                    plan_execution_id: None,
                    plan_error: None,
                    groups: match &pulled {
                        Ok(groups) => groups.clone(),
                        Err(error) => vec![PulledGroup::note(
                            Outcome::Failed,
                            &format!("{error:#}")
                                .chars()
                                .take(MAX_REASON_CHARS)
                                .collect::<String>(),
                        )],
                    },
                };
                // A failed pull is reported but never cached, so the next
                // click retries it; settled outcomes (including unreadable,
                // which is deterministic for this reader) are remembered
                // until another build of the reader looks at them.
                if pulled.is_ok() && cacheable(&execution) {
                    cache.executions.insert(key, execution.clone());
                    cache.save(runs_dir)?;
                }
                execution
            }
        };
        executions.push(execution);
        if wanted.is_some() {
            break;
        }
    }
    if let Some(wanted) = &wanted {
        if executions.is_empty() {
            bail!(
                "no completed {WORKFLOW} run titled '{TITLE_PREFIX}{wanted}' \
                 among the last {MAX_LIMIT} runs"
            );
        }
    }
    Ok(PullResponse {
        runs_dir: runs_dir.display().to_string(),
        repository: REPOSITORY.into(),
        workflow: WORKFLOW.into(),
        executions,
        remaining_runs,
    })
}

async fn pull_run(github: &GitHub, runs_dir: &Path, run: &WorkflowRun) -> Result<Vec<PulledGroup>> {
    let artifacts = github.artifacts(run.id).await?;
    let selected = match choose_artifacts(&artifacts, &run.execution_id) {
        Selection::Root(artifact) => vec![artifact],
        Selection::Groups(list) => list,
        Selection::Expired => {
            return Ok(vec![PulledGroup::note(
                Outcome::Expired,
                "the observation artifacts expired (GitHub keeps them for 90 days)",
            )])
        }
        Selection::Missing => {
            return Ok(vec![PulledGroup::note(
                Outcome::NotImportable,
                "the run published no observation artifact",
            )])
        }
    };
    let mut groups = Vec::new();
    for artifact in selected {
        let file = github.download(&artifact, runs_dir).await?;
        let runs_dir = runs_dir.to_path_buf();
        let installed = tokio::task::spawn_blocking(move || install_from_zip(file, &runs_dir))
            .await
            .context("install task")?
            .with_context(|| format!("install {}", artifact.name))?;
        groups.extend(installed);
    }
    groups.sort_by(|left, right| {
        (&left.campaign_id, &left.group_id, &left.native_execution_id).cmp(&(
            &right.campaign_id,
            &right.group_id,
            &right.native_execution_id,
        ))
    });
    Ok(groups)
}

// ---------------------------------------------------------------------------
// GitHub
// ---------------------------------------------------------------------------

async fn token() -> Option<String> {
    for name in TOKEN_VARIABLES {
        if let Some(value) = std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    let output = tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[derive(Debug, Clone)]
struct WorkflowRun {
    id: u64,
    run_attempt: u32,
    execution_id: String,
    url: String,
    created_at: String,
}

#[derive(Debug, Clone)]
struct Artifact {
    name: String,
    expired: bool,
    download_url: String,
    size_bytes: u64,
}

enum Selection {
    Root(Artifact),
    Groups(Vec<Artifact>),
    Expired,
    Missing,
}

struct GitHub {
    client: reqwest::Client,
}

impl GitHub {
    fn new(token: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .context("the GitHub token is not a valid header value")?;
        authorization.set_sensitive(true);
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        let client = reqwest::Client::builder()
            .user_agent(concat!("harness-e2e/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .timeout(API_TIMEOUT)
            .build()
            .context("build the GitHub client")?;
        Ok(Self { client })
    }

    async fn json(&self, url: &str) -> Result<Value> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("read {url}"))?;
        if !status.is_success() {
            bail!(
                "GitHub answered {status} for {url}: {}",
                body.chars().take(MAX_REASON_CHARS).collect::<String>()
            );
        }
        serde_json::from_str(&body).with_context(|| format!("decode {url}"))
    }

    async fn completed_runs(&self, per_page: u32) -> Result<Vec<WorkflowRun>> {
        let url = format!(
            "{API_ROOT}/repos/{REPOSITORY}/actions/workflows/{WORKFLOW}/runs?status=completed&per_page={per_page}"
        );
        Ok(workflow_runs(&self.json(&url).await?))
    }

    async fn artifacts(&self, run_id: u64) -> Result<Vec<Artifact>> {
        let url =
            format!("{API_ROOT}/repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100");
        Ok(artifacts(&self.json(&url).await?))
    }

    /// Stream the artifact into an unnamed temporary file next to the runs:
    /// a bundle can weigh hundreds of megabytes and must not sit in memory.
    async fn download(&self, artifact: &Artifact, scratch_dir: &Path) -> Result<fs::File> {
        if artifact.size_bytes > MAX_ARTIFACT_BYTES {
            bail!(
                "{} is {} bytes; the dashboard downloads at most {MAX_ARTIFACT_BYTES}",
                artifact.name,
                artifact.size_bytes
            );
        }
        let response = self
            .client
            .get(&artifact.download_url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .with_context(|| format!("download {}", artifact.name))?;
        let status = response.status();
        if !status.is_success() {
            bail!("GitHub answered {status} downloading {}", artifact.name);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ARTIFACT_BYTES)
        {
            bail!("{} exceeds {MAX_ARTIFACT_BYTES} bytes", artifact.name);
        }
        let mut file = tempfile::tempfile_in(scratch_dir)
            .with_context(|| format!("create a temporary file in {}", scratch_dir.display()))?;
        let mut received: u64 = 0;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| {
                format!("read {} ({} bytes)", artifact.name, artifact.size_bytes)
            })?;
            received += chunk.len() as u64;
            if received > MAX_ARTIFACT_BYTES {
                bail!("{} exceeds {MAX_ARTIFACT_BYTES} bytes", artifact.name);
            }
            file.write_all(&chunk)
                .with_context(|| format!("spool {}", artifact.name))?;
        }
        file.seek(SeekFrom::Start(0))
            .with_context(|| format!("rewind {}", artifact.name))?;
        Ok(file)
    }
}

fn execution_id_from_title(title: &str) -> Option<String> {
    let id = title.strip_prefix(TITLE_PREFIX)?.trim();
    let shaped = id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-');
    shaped.then(|| id.to_lowercase())
}

fn workflow_runs(value: &Value) -> Vec<WorkflowRun> {
    value
        .get("workflow_runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|run| {
            let title = run.get("display_title").and_then(Value::as_str)?;
            Some(WorkflowRun {
                id: run.get("id").and_then(Value::as_u64)?,
                run_attempt: run
                    .get("run_attempt")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(1),
                execution_id: execution_id_from_title(title)?,
                url: text(run.get("html_url")),
                created_at: text(run.get("created_at")),
            })
        })
        .collect()
}

fn artifacts(value: &Value) -> Vec<Artifact> {
    value
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|artifact| {
            Some(Artifact {
                name: artifact.get("name").and_then(Value::as_str)?.to_string(),
                expired: artifact
                    .get("expired")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                download_url: artifact
                    .get("archive_download_url")
                    .and_then(Value::as_str)?
                    .to_string(),
                size_bytes: artifact
                    .get("size_in_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            })
        })
        .collect()
}

/// The root bundle of the newest workflow attempt wins; when the finalizer
/// never published one, every group artifact of the newest attempt is used.
fn choose_artifacts(artifacts: &[Artifact], execution_id: &str) -> Selection {
    let group_prefix = format!("{ARTIFACT_PREFIX}{execution_id}-");
    let root_prefix = format!("{group_prefix}gh-");
    let attempt_of = |name: &str| {
        name.rsplit_once("-gh-")
            .and_then(|(_, attempt)| attempt.parse::<u32>().ok())
    };
    let mut root: Option<(u32, Artifact)> = None;
    let mut groups: BTreeMap<u32, Vec<Artifact>> = BTreeMap::new();
    let mut expired = false;
    for artifact in artifacts {
        if !artifact.name.starts_with(&group_prefix) {
            continue;
        }
        let Some(attempt) = attempt_of(&artifact.name) else {
            continue;
        };
        if artifact.expired {
            expired = true;
            continue;
        }
        if artifact.name.starts_with(&root_prefix) {
            if root.as_ref().is_none_or(|(current, _)| attempt > *current) {
                root = Some((attempt, artifact.clone()));
            }
        } else {
            groups.entry(attempt).or_default().push(artifact.clone());
        }
    }
    if let Some((_, artifact)) = root {
        return Selection::Root(artifact);
    }
    if let Some((_, list)) = groups.into_iter().next_back() {
        return Selection::Groups(list);
    }
    if expired {
        Selection::Expired
    } else {
        Selection::Missing
    }
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

struct NativeEntry {
    campaign_id: Option<String>,
    group_id: Option<String>,
    id: String,
    rest: PathBuf,
}

struct NativeRun {
    campaign_id: Option<String>,
    group_id: Option<String>,
    tmp: PathBuf,
    fault: Option<String>,
}

fn is_native_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normal_parts(path: &Path) -> Option<Vec<&str>> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect()
}

/// `<campaign>/groups/<group>/native/<id>/<rest>` in a root bundle, or
/// `native/<id>/<rest>` in a single group artifact.
fn native_entry(path: &Path) -> Option<NativeEntry> {
    let parts = normal_parts(path)?;
    let position = parts.iter().position(|part| *part == "native")?;
    let id = *parts.get(position + 1)?;
    if !is_native_id(id) {
        return None;
    }
    let rest: PathBuf = parts[position + 2..].iter().collect();
    if rest.as_os_str().is_empty() {
        return None;
    }
    let (campaign_id, group_id) = match &parts[..position] {
        [campaign, groups, group] if *groups == "groups" => {
            (Some((*campaign).to_string()), Some((*group).to_string()))
        }
        _ => (None, None),
    };
    Some(NativeEntry {
        campaign_id,
        group_id,
        id: id.to_string(),
        rest,
    })
}

fn campaign_summary_entry(path: &Path) -> Option<String> {
    match normal_parts(path)?.as_slice() {
        [campaign, file] if *file == "campaign-summary.json" => Some((*campaign).to_string()),
        _ => None,
    }
}

fn install_from_zip<R: Read + Seek>(reader: R, runs_dir: &Path) -> Result<Vec<PulledGroup>> {
    let mut archive = zip::ZipArchive::new(reader).context("open the artifact zip")?;
    let mut natives: BTreeMap<String, NativeRun> = BTreeMap::new();
    let mut summaries: Vec<(String, Value)> = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("read a zip entry")?;
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        if let Some(campaign) = campaign_summary_entry(&path) {
            let mut text = String::new();
            if entry.read_to_string(&mut text).is_ok() {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    summaries.push((campaign, value));
                }
            }
            continue;
        }
        let Some(native) = native_entry(&path) else {
            continue;
        };
        if !natives.contains_key(&native.id) {
            let tmp = runs_dir.join(format!("{}.pull-tmp", native.id));
            let _ = fs::remove_dir_all(&tmp);
            natives.insert(
                native.id.clone(),
                NativeRun {
                    campaign_id: native.campaign_id.clone(),
                    group_id: native.group_id.clone(),
                    tmp,
                    fault: None,
                },
            );
        }
        let run = natives
            .get_mut(&native.id)
            .expect("the native run was just inserted");
        if run.fault.is_some() {
            continue;
        }
        let symlink = entry
            .unix_mode()
            .is_some_and(|mode| (mode & 0o170_000) == 0o120_000);
        if symlink {
            run.fault = Some("the artifact contains a symlink".into());
            continue;
        }
        let destination = run.tmp.join(&native.rest);
        if entry.is_dir() {
            fs::create_dir_all(&destination)
                .with_context(|| format!("create {}", destination.display()))?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut file = fs::File::create(&destination)
            .with_context(|| format!("write {}", destination.display()))?;
        std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("extract {}", destination.display()))?;
    }

    let mut groups: Vec<PulledGroup> = natives
        .into_iter()
        .map(|(id, run)| install_native(runs_dir, &id, run))
        .collect();
    for (campaign, summary) in summaries {
        for group in summary
            .get("groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(group_id) = group.get("group_id").and_then(Value::as_str) else {
                continue;
            };
            let listed = groups.iter().any(|candidate| {
                candidate.campaign_id.as_deref() == Some(campaign.as_str())
                    && candidate.group_id.as_deref() == Some(group_id)
            });
            if listed {
                continue;
            }
            let kind = group
                .get("execution_kind")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let reason = if kind == "fault_injection" {
                "fault injection groups have no native run"
            } else {
                // The finalizer keeps only what the campaign bundle references,
                // so a group that failed before its results leaves nothing here.
                "no native run in the observation bundle (the group left no results)"
            };
            let mut note = PulledGroup::note(Outcome::NotImportable, reason);
            note.campaign_id = Some(campaign.clone());
            note.group_id = Some(group_id.to_string());
            groups.push(note);
        }
    }
    Ok(groups)
}

struct Peek {
    schema_version: Option<u64>,
    runner_version: Option<String>,
    runner_revision: Option<String>,
}

fn peek_results(dir: &Path) -> Peek {
    let value: Option<Value> = fs::read(dir.join("results.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let runner = value
        .as_ref()
        .and_then(|value| value.pointer("/observation_contract/runner"));
    Peek {
        schema_version: value
            .as_ref()
            .and_then(|value| value.get("schema_version"))
            .and_then(Value::as_u64),
        runner_version: runner
            .and_then(|runner| runner.get("version"))
            .and_then(Value::as_str)
            .map(String::from),
        runner_revision: runner
            .and_then(|runner| runner.get("revision"))
            .and_then(Value::as_str)
            .map(String::from),
    }
}

fn install_native(runs_dir: &Path, id: &str, run: NativeRun) -> PulledGroup {
    let peek = peek_results(&run.tmp);
    let mut group = PulledGroup {
        campaign_id: run.campaign_id,
        group_id: run.group_id,
        native_execution_id: Some(id.to_string()),
        outcome: Outcome::Unreadable,
        reason: None,
        runner_version: peek.runner_version,
        runner_revision: peek.runner_revision,
        schema_version: peek.schema_version,
    };
    let destination = runs_dir.join(id);
    let verdict = match run.fault {
        Some(fault) => Err(anyhow!(fault)),
        None if destination.exists() => Ok(Verdict::Exists),
        None => inspect(&run.tmp),
    };
    match verdict {
        Ok(Verdict::Exists) => {
            let _ = fs::remove_dir_all(&run.tmp);
            group.outcome = Outcome::Exists;
        }
        Ok(Verdict::Discard(reason)) => {
            let _ = fs::remove_dir_all(&run.tmp);
            group.outcome = Outcome::NotImportable;
            group.reason = Some(reason.into());
        }
        Ok(Verdict::Import) => match fs::rename(&run.tmp, &destination) {
            Ok(()) => group.outcome = Outcome::Imported,
            Err(error) => {
                let _ = fs::remove_dir_all(&run.tmp);
                group.reason = Some(format!("install {}: {error}", destination.display()));
            }
        },
        Err(error) => {
            let _ = fs::remove_dir_all(&run.tmp);
            group.reason = Some(
                format!("{error:#}")
                    .chars()
                    .take(MAX_REASON_CHARS)
                    .collect(),
            );
        }
    }
    group
}

enum Verdict {
    Import,
    Exists,
    Discard(&'static str),
}

/// A run is installed when the dashboard's own reader accepts its report and
/// the report carries metrics. A group whose worker died before `results.json`,
/// or whose runs left no tokens or session metrics, is an infrastructure
/// failure without measurements and is discarded rather than shown empty.
fn inspect(tmp: &Path) -> Result<Verdict> {
    if !tmp.join("results.json").is_file() {
        return Ok(Verdict::Discard(
            "discarded: the worker ended before writing results.json (infrastructure failure, no metrics)",
        ));
    }
    let (report, _) = E2eReport::read_from(tmp)?;
    if !has_metrics(&report) {
        return Ok(Verdict::Discard(
            "discarded: infrastructure failure without metrics (no run left tokens or session metrics)",
        ));
    }
    Ok(Verdict::Import)
}

fn has_metrics(report: &E2eReport) -> bool {
    report.scenarios.iter().any(|scenario| {
        scenario
            .aggregate
            .total_tokens_consumed
            .is_some_and(|tokens| tokens > 0)
            || scenario.runs.iter().any(|run| {
                run.metrics.is_some()
                    || run
                        .efficiency
                        .as_ref()
                        .and_then(|efficiency| efficiency.total_tokens)
                        .is_some_and(|tokens| tokens > 0)
                    || run.cost.total_usd.is_some_and(|usd| usd > 0.0)
                    || run.cost.subject_usd.is_some_and(|usd| usd > 0.0)
            })
    })
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    executions: BTreeMap<String, PulledExecution>,
}

impl Cache {
    fn load(runs_dir: &Path) -> Self {
        let path = runs_dir.join(CACHE_FILE);
        let mut cache: Self = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(path = %path.display(), %error, "ignoring an unreadable pull cache");
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        // Entries written by an earlier reader may hold outcomes this reader
        // would not repeat; only settled ones are trusted from memory.
        let reader = reader_identity();
        cache
            .executions
            .retain(|_, execution| execution.reader == reader && cacheable(execution));
        cache
    }

    fn save(&self, runs_dir: &Path) -> Result<()> {
        let target = runs_dir.join(CACHE_FILE);
        let temporary = runs_dir.join(format!("{CACHE_FILE}.tmp"));
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        fs::write(&temporary, bytes).with_context(|| format!("write {}", temporary.display()))?;
        fs::rename(&temporary, &target).with_context(|| format!("replace {}", target.display()))
    }
}

fn cache_key(run: &WorkflowRun) -> String {
    format!("{}-{}", run.id, run.run_attempt)
}

/// Only executions whose every group settled (installed, already present, or
/// impossible to import) are remembered; unreadable or failed groups are
/// retried on the next click.
fn cacheable(execution: &PulledExecution) -> bool {
    execution
        .groups
        .iter()
        .all(|group| group.outcome != Outcome::Failed)
}

/// What decides an outcome: the binary and the result contract it reads. A
/// cache entry written by another reader is forgotten, so an upgrade retries
/// what the previous build could not read.
fn reader_identity() -> String {
    format!(
        "{}@{}#{}",
        env!("CARGO_PKG_VERSION"),
        env!("HARNESS_E2E_BUILD_REVISION"),
        crate::report::RESULT_CONTRACT_SHA256
    )
}

/// A cached execution is reported as it stands now: what an earlier click
/// installed is already present, so this click's "added" count stays honest.
fn replayed(mut execution: PulledExecution) -> PulledExecution {
    let pulled_at = execution.pulled_at.clone();
    for group in &mut execution.groups {
        if group.outcome == Outcome::Imported {
            group.outcome = Outcome::Exists;
            group.reason = Some(format!("imported {pulled_at}"));
        }
    }
    execution
}

/// A cached execution stands only while every run it installed is still on disk.
fn still_installed(runs_dir: &Path, execution: &PulledExecution) -> bool {
    execution
        .groups
        .iter()
        .filter(|group| matches!(group.outcome, Outcome::Imported | Outcome::Exists))
        .all(|group| {
            group
                .native_execution_id
                .as_ref()
                .is_some_and(|id| runs_dir.join(id).is_dir())
        })
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use serde_json::json;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::dashboard::tests::write_report;
    use crate::dashboard::{JobStatus, RunMetadata, RunRequest};

    const NATIVE_ID: &str = "4fea7a941bdcc85bb5f0eda5a68429ea";
    const OTHER_ID: &str = "2fb825cbbf5a2c89ba39aeab9eef66db";
    const EXECUTION_ID: &str = "5ad654c4-693e-4b8a-98f9-7df2c44e0640";

    fn artifact(name: &str, expired: bool) -> Artifact {
        Artifact {
            name: name.into(),
            expired,
            download_url: format!("https://example.test/{name}"),
            size_bytes: 1,
        }
    }

    fn add_tree(writer: &mut zip::ZipWriter<Cursor<Vec<u8>>>, prefix: &str, root: &Path) {
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut paths: Vec<PathBuf> = walk(root);
        paths.sort();
        for path in paths {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            if path.is_dir() {
                writer
                    .add_directory(format!("{prefix}{relative}/"), options)
                    .unwrap();
            } else {
                writer
                    .start_file(format!("{prefix}{relative}"), options)
                    .unwrap();
                writer.write_all(&fs::read(&path).unwrap()).unwrap();
            }
        }
    }

    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                paths.push(path.clone());
                paths.extend(walk(&path));
            } else {
                paths.push(path);
            }
        }
        paths
    }

    fn zip_with(build: impl FnOnce(&mut zip::ZipWriter<Cursor<Vec<u8>>>)) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        build(&mut writer);
        writer.finish().unwrap().into_inner()
    }

    fn native_prefix(group: &str, id: &str) -> String {
        format!("regression-r01/groups/{group}/native/{id}/")
    }

    fn metadata(id: &str, status: JobStatus) -> RunMetadata {
        RunMetadata {
            id: id.into(),
            label: "Regression · regression-r01 · case-minimal-path".into(),
            status,
            started_at: "2026-09-07T05:20:54Z".into(),
            completed_at: String::new(),
            returncode: None,
            error: String::new(),
            request: RunRequest {
                _caller_worker_id: None,
                label: "Regression".into(),
                url: "ws://127.0.0.1:49134".into(),
                model: "model".into(),
                provider: "provider".into(),
                judge_model: String::new(),
                judge_provider: String::new(),
                scenarios: vec!["minimal_path".into()],
                runs: 1,
                technical_retries: 1,
                seed: None,
            },
        }
    }

    fn root_bundle(source: &Path) -> Vec<u8> {
        zip_with(|writer| {
            let options = SimpleFileOptions::default();
            writer
                .start_file("regression-r01/campaign-summary.json", options)
                .unwrap();
            writer
                .write_all(
                    json!({
                        "campaign_id": "regression-r01",
                        "groups": [
                            {"group_id": "case-minimal-path", "execution_kind": "harness_turn"},
                            {"group_id": "fault-l2", "execution_kind": "fault_injection"},
                        ],
                    })
                    .to_string()
                    .as_bytes(),
                )
                .unwrap();
            writer
                .start_file(
                    "regression-r01/groups/case-minimal-path/results.json",
                    options,
                )
                .unwrap();
            writer.write_all(b"{}").unwrap();
            add_tree(
                writer,
                &native_prefix("case-minimal-path", NATIVE_ID),
                source,
            );
        })
    }

    #[test]
    fn reads_execution_ids_from_run_titles() {
        assert_eq!(
            execution_id_from_title(&format!("E2E · {EXECUTION_ID}")).as_deref(),
            Some(EXECUTION_ID)
        );
        assert_eq!(execution_id_from_title("E2E · not-an-id"), None);
        assert_eq!(execution_id_from_title("CI"), None);
    }

    #[test]
    fn prefers_the_newest_root_bundle_and_falls_back_to_group_artifacts() {
        let root_1 = format!("{ARTIFACT_PREFIX}{EXECUTION_ID}-gh-1");
        let root_2 = format!("{ARTIFACT_PREFIX}{EXECUTION_ID}-gh-2");
        let group_2 =
            format!("{ARTIFACT_PREFIX}{EXECUTION_ID}-regression-r01-case-minimal-path-gh-2");
        let contract = format!("e2e-contract-{EXECUTION_ID}-gh-2");
        let other = format!("{ARTIFACT_PREFIX}00000000-0000-0000-0000-000000000000-gh-9");

        let artifacts = [
            artifact(&root_1, false),
            artifact(&root_2, false),
            artifact(&group_2, false),
            artifact(&contract, false),
            artifact(&other, false),
        ];
        match choose_artifacts(&artifacts, EXECUTION_ID) {
            Selection::Root(chosen) => assert_eq!(chosen.name, root_2),
            _ => panic!("the newest root bundle should win"),
        }

        let artifacts = [
            artifact(&root_1, true),
            artifact(&group_2, false),
            artifact(
                &format!("{ARTIFACT_PREFIX}{EXECUTION_ID}-regression-r01-case-timer-wake-gh-1"),
                false,
            ),
        ];
        match choose_artifacts(&artifacts, EXECUTION_ID) {
            Selection::Groups(list) => {
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].name, group_2);
            }
            _ => panic!("group artifacts of the newest attempt should be used"),
        }

        assert!(matches!(
            choose_artifacts(&[artifact(&root_1, true)], EXECUTION_ID),
            Selection::Expired
        ));
        assert!(matches!(
            choose_artifacts(&[artifact(&contract, false)], EXECUTION_ID),
            Selection::Missing
        ));
    }

    #[test]
    fn recognises_native_entries_in_both_bundle_layouts() {
        let entry = native_entry(Path::new(
            "regression-r01/groups/case-minimal-path/native/4fea7a941bdcc85bb5f0eda5a68429ea/evidence/a/b.json",
        ))
        .unwrap();
        assert_eq!(entry.campaign_id.as_deref(), Some("regression-r01"));
        assert_eq!(entry.group_id.as_deref(), Some("case-minimal-path"));
        assert_eq!(entry.id, NATIVE_ID);
        assert_eq!(entry.rest, Path::new("evidence/a/b.json"));

        let entry = native_entry(Path::new(&format!("native/{NATIVE_ID}/results.json"))).unwrap();
        assert!(entry.campaign_id.is_none());
        assert_eq!(entry.rest, Path::new("results.json"));

        assert!(native_entry(Path::new(&format!("native/{NATIVE_ID}/"))).is_none());
        assert!(native_entry(Path::new("native/plan-store/plans/x.json")).is_none());
        assert!(native_entry(Path::new(&format!("groups/x/native/{NATIVE_ID}/../y"))).is_none());
        assert_eq!(
            campaign_summary_entry(Path::new("regression-r01/campaign-summary.json")).as_deref(),
            Some("regression-r01")
        );
        assert!(
            campaign_summary_entry(Path::new("regression-r01/groups/g/campaign-summary.json"))
                .is_none()
        );
    }

    #[test]
    fn installs_native_runs_and_reports_every_group() {
        let source = tempfile::tempdir().unwrap();
        write_report(source.path());
        let bytes = root_bundle(source.path());

        let runs = tempfile::tempdir().unwrap();
        let groups = install_from_zip(Cursor::new(bytes.as_slice()), runs.path()).unwrap();
        assert_eq!(groups.len(), 2, "{groups:?}");
        let imported = &groups[0];
        assert_eq!(imported.outcome, Outcome::Imported);
        assert_eq!(imported.campaign_id.as_deref(), Some("regression-r01"));
        assert_eq!(imported.group_id.as_deref(), Some("case-minimal-path"));
        assert_eq!(imported.native_execution_id.as_deref(), Some(NATIVE_ID));
        assert_eq!(
            imported.schema_version,
            Some(u64::from(crate::result_contract::RESULTS_SCHEMA_VERSION))
        );
        assert!(runs.path().join(NATIVE_ID).join("results.json").is_file());
        assert!(!runs.path().join(format!("{NATIVE_ID}.pull-tmp")).exists());
        assert!(
            !runs.path().join("regression-r01").exists(),
            "only native runs are installed"
        );
        let fault = &groups[1];
        assert_eq!(fault.outcome, Outcome::NotImportable);
        assert_eq!(fault.group_id.as_deref(), Some("fault-l2"));

        let again = install_from_zip(Cursor::new(bytes.as_slice()), runs.path()).unwrap();
        assert_eq!(again[0].outcome, Outcome::Exists);
        assert!(!runs.path().join(format!("{NATIVE_ID}.pull-tmp")).exists());
    }

    #[test]
    fn rejects_an_incompatible_result_contract_with_its_runner() {
        let source = tempfile::tempdir().unwrap();
        write_report(source.path());
        fs::write(
            source.path().join("results.json"),
            json!({
                "schema_version": 4,
                "observation_contract": {
                    "runner": {"name": "harness-e2e", "version": "0.8.6-experimental", "revision": "d1cc7a42"}
                }
            })
            .to_string(),
        )
        .unwrap();
        let bytes = root_bundle(source.path());

        let runs = tempfile::tempdir().unwrap();
        let groups = install_from_zip(Cursor::new(bytes.as_slice()), runs.path()).unwrap();
        let rejected = &groups[0];
        assert_eq!(rejected.outcome, Outcome::Unreadable);
        assert_eq!(rejected.schema_version, Some(4));
        assert_eq!(
            rejected.runner_version.as_deref(),
            Some("0.8.6-experimental")
        );
        assert_eq!(rejected.runner_revision.as_deref(), Some("d1cc7a42"));
        assert!(
            rejected
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("decode typed E2E report"),
            "{rejected:?}"
        );
        assert!(!runs.path().join(NATIVE_ID).exists());
        assert!(!runs.path().join(format!("{NATIVE_ID}.pull-tmp")).exists());
    }

    #[test]
    fn discards_groups_that_failed_without_metrics() {
        // A worker that died before results.json leaves metadata and a journal.
        let journal_only = tempfile::tempdir().unwrap();
        super::super::store::write_metadata(
            journal_only.path(),
            &metadata(OTHER_ID, JobStatus::Running),
        )
        .unwrap();
        fs::create_dir_all(journal_only.path().join("journal/events")).unwrap();
        fs::write(journal_only.path().join("journal/header.json"), b"{}").unwrap();

        // A report whose runs left no tokens, cost or session metrics.
        let no_metrics = tempfile::tempdir().unwrap();
        write_report(no_metrics.path());
        let results = no_metrics.path().join("results.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&results).unwrap()).unwrap();
        value["schema_version"] = json!(5);
        for scenario in value["scenarios"].as_array_mut().unwrap() {
            scenario["aggregate"]["total_tokens_consumed"] = Value::Null;
            for run in scenario["runs"].as_array_mut().unwrap() {
                run["metrics"] = Value::Null;
                run["efficiency"] = Value::Null;
                run["cost"] = json!({"subject_usd": null, "total_usd": null});
            }
        }
        fs::write(&results, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let bytes = zip_with(|writer| {
            add_tree(
                writer,
                &native_prefix("case-timer-wake", OTHER_ID),
                journal_only.path(),
            );
            add_tree(
                writer,
                &native_prefix("case-minimal-path", NATIVE_ID),
                no_metrics.path(),
            );
        });

        let runs = tempfile::tempdir().unwrap();
        let groups = install_from_zip(Cursor::new(bytes.as_slice()), runs.path()).unwrap();
        assert_eq!(groups.len(), 2, "{groups:?}");
        for group in &groups {
            assert_eq!(group.outcome, Outcome::NotImportable, "{group:?}");
            assert!(
                group
                    .reason
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("discarded:"),
                "{group:?}"
            );
        }
        assert!(!runs.path().join(OTHER_ID).exists());
        assert!(!runs.path().join(NATIVE_ID).exists());
        assert!(
            fs::read_dir(runs.path()).unwrap().next().is_none(),
            "nothing is installed"
        );
    }

    #[test]
    fn a_cached_execution_stands_only_while_its_runs_exist() {
        let runs = tempfile::tempdir().unwrap();
        fs::create_dir_all(runs.path().join(NATIVE_ID)).unwrap();
        let mut group = PulledGroup::note(Outcome::Imported, "");
        group.native_execution_id = Some(NATIVE_ID.into());
        let execution = PulledExecution {
            execution_id: EXECUTION_ID.into(),
            run_id: 1,
            run_attempt: 1,
            url: String::new(),
            created_at: String::new(),
            pulled_at: String::new(),
            reader: reader_identity(),
            plan_id: None,
            plan_execution_id: None,
            plan_error: None,
            groups: vec![group, PulledGroup::note(Outcome::NotImportable, "fault")],
        };
        assert!(still_installed(runs.path(), &execution));
        fs::remove_dir_all(runs.path().join(NATIVE_ID)).unwrap();
        assert!(!still_installed(runs.path(), &execution));

        assert!(cacheable(&execution));
        let replay = replayed(execution.clone());
        assert_eq!(replay.groups[0].outcome, Outcome::Exists);
        assert!(replay.groups[0]
            .reason
            .as_deref()
            .unwrap_or_default()
            .starts_with("imported "));
        assert_eq!(replay.groups[1].outcome, Outcome::NotImportable);
        let mut unreadable = execution.clone();
        unreadable.groups.push(PulledGroup::note(
            Outcome::Unreadable,
            "missing field asset_id",
        ));
        assert!(
            cacheable(&unreadable),
            "an unreadable outcome is deterministic for this reader"
        );
        let mut failed = execution.clone();
        failed
            .groups
            .push(PulledGroup::note(Outcome::Failed, "download timed out"));
        assert!(!cacheable(&failed));
        let mut other_reader = execution.clone();
        other_reader.reader = "0.0.0@0000000#sha256:old".into();

        let mut cache = Cache::default();
        cache.executions.insert("1-1".into(), execution);
        cache.executions.insert("2-1".into(), unreadable);
        cache.executions.insert("3-1".into(), failed);
        cache.executions.insert("4-1".into(), other_reader);
        cache.save(runs.path()).unwrap();
        let loaded = Cache::load(runs.path());
        assert!(loaded.executions.contains_key("1-1"));
        assert!(loaded.executions.contains_key("2-1"));
        assert!(
            !loaded.executions.contains_key("3-1"),
            "a failed pull is retried, not remembered"
        );
        assert!(
            !loaded.executions.contains_key("4-1"),
            "another reader's outcomes are re-pulled by this one"
        );
        fs::write(runs.path().join(CACHE_FILE), b"not json").unwrap();
        assert!(Cache::load(runs.path()).executions.is_empty());
    }
}
