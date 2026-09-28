//! What the stack builder reads from outside this Console: the projects of
//! iii-hq/templates, with the workers each one's `worker-compose.yaml`
//! declares, the releases of the iii CLI (iii-hq/iii), and what the iii
//! registry resolves a worker to. All are public and read without
//! credentials (a `GITHUB_TOKEN` only raises GitHub's rate limit); each
//! answer is kept in memory for ten minutes.
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const TEMPLATES_REPOSITORY: &str = "iii-hq/templates";
const TEMPLATES_REF: &str = "main";
const REGISTRY_RESOLVE: &str = "https://api.workers.iii.dev/resolve";
/// The target `compose::add` asks the registry for (`prepare_execution.py`).
const TARGET: &str = "x86_64-unknown-linux-gnu";
const KEEP_FOR: Duration = Duration::from_secs(10 * 60);
const TIMEOUT: Duration = Duration::from_secs(10);
/// Past this an answer is refused before it is read further.
const MAX_BYTES: usize = 1024 * 1024;
/// Workers whose resolution is kept at once.
const MAX_RESOLVED: usize = 256;
/// Revisions of iii-hq/templates whose templates are kept at once.
const MAX_REVISIONS: usize = 16;
const GITHUB_API: &str = "https://api.github.com";
const III_REPOSITORY: &str = "iii-hq/iii";
/// The archive a group installs the iii CLI from (`prepare_execution.py`
/// `CLI_ASSET`, for `TARGET`).
const CLI_ASSET: &str = "iii-x86_64-unknown-linux-gnu.tar.gz";
/// Releases asked for at once: each lists some 70 assets, about 130 KB.
const RELEASES_PER_PAGE: usize = 20;
/// The list of releases is past 1 MB at any useful page size.
const MAX_RELEASES_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackTemplates {
    /// `iii-hq/templates`.
    pub repository: String,
    /// The revision asked for: `main` unless another commit, tag or branch was.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The commit it was at when it was read.
    pub revision: String,
    /// In the order `iii/template.yaml` lists them; a listed folder that does
    /// not exist is left out.
    pub templates: Vec<StackTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackTemplate {
    /// The folder under `iii/`, what a stack's `template:` names.
    pub id: String,
    pub name: String,
    pub description: String,
    /// The containers its `worker-compose.yaml` declares with a `worker:`.
    pub workers: Vec<TemplateWorker>,
    /// Why it declares no workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The oldest iii release it runs on, when its `template.yaml` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_iii_version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct StackTemplatesRequest {
    /// A commit, tag or branch of iii-hq/templates; `main` when absent.
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct TemplateWorker {
    /// The container's name.
    pub name: String,
    /// The worker as written: `package://…` or `path://…`.
    pub worker: String,
    /// The version it pins, when it pins one.
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct WorkerResolveRequest {
    /// A worker's name in the iii registry, e.g. `harness`.
    pub worker: String,
}

/// A worker's newest release and what it brings, or why the registry has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum WorkerResolution {
    Found {
        name: String,
        /// Its newest release.
        version: String,
        /// Every worker its graph brings, engine built-ins left out.
        dependencies: Vec<String>,
    },
    Refused {
        error: RegistryRefusal,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RegistryRefusal {
    /// The registry's code, e.g. `worker_not_found`.
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct IiiReleasesRequest {
    /// A version typed by hand, e.g. `0.24.2`: whether iii-hq/iii has the
    /// release `iii/v<version>` and it publishes the CLI.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct IiiReleases {
    /// What `iii: latest` installs: the newest `X.Y.Z-rc.N` tag.
    pub latest_candidate: Option<String>,
    /// The newest releases, stable and pre-release, as GitHub lists them;
    /// alphas (`iii-alpha/…`) and drafts left out.
    pub releases: Vec<IiiRelease>,
    /// The version asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<IiiVersionCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct IiiRelease {
    /// The tag without `iii/v`.
    pub version: String,
    pub prerelease: bool,
    pub published_at: Option<String>,
    /// Whether it publishes `iii-x86_64-unknown-linux-gnu.tar.gz`, the CLI a
    /// group installs.
    pub cli: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct IiiVersionCheck {
    pub version: String,
    /// Its release; none when iii-hq/iii has no `iii/v<version>`.
    pub release: Option<IiiRelease>,
}

/// The releases of iii-hq/iii and until when they are kept, held while read.
static RELEASES: tokio::sync::Mutex<Option<(Instant, IiiReleases)>> =
    tokio::sync::Mutex::const_new(None);
/// The templates of each revision asked for and until when they are kept.
/// Held while they are read, so the Consoles that open New stack at once
/// wait for one read.
static TEMPLATES: tokio::sync::Mutex<BTreeMap<String, (Instant, StackTemplates)>> =
    tokio::sync::Mutex::const_new(BTreeMap::new());
/// Each worker's resolution and until when it is kept.
static RESOLVED: Mutex<BTreeMap<String, (Instant, WorkerResolution)>> = Mutex::new(BTreeMap::new());

/// The projects of iii-hq/templates at `revision` (a commit, tag or branch;
/// `main` when none), kept ten minutes.
pub(crate) async fn templates(revision: Option<&str>) -> Result<StackTemplates> {
    let revision = revision.unwrap_or(TEMPLATES_REF);
    ensure!(
        template_revision(revision),
        "A revision is a commit, tag or branch of iii-hq/templates: letters, digits and ._/-, up to 100 characters."
    );
    let mut kept = TEMPLATES.lock().await;
    if let Some((until, read)) = kept.get(revision) {
        if *until > Instant::now() {
            return Ok(read.clone());
        }
    }
    let read = read_templates(revision).await?;
    remember(
        &mut kept,
        revision,
        read.clone(),
        Instant::now(),
        MAX_REVISIONS,
    );
    Ok(read)
}

/// `^[A-Za-z0-9._/-]{1,100}$`, without what no git ref holds (`..`, `//`,
/// a `/` at either end), so it stays a revision in GitHub's URLs.
fn template_revision(revision: &str) -> bool {
    (1..=100).contains(&revision.len())
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
        && !revision.contains("..")
        && !revision.contains("//")
        && !revision.starts_with('/')
        && !revision.ends_with('/')
}

/// The newest releases of iii-hq/iii and its newest release candidate, kept
/// ten minutes; with `version`, whether that release exists and publishes
/// the CLI.
pub(crate) async fn iii_releases(version: Option<&str>) -> Result<IiiReleases> {
    if let Some(version) = version {
        ensure!(
            release_version(version),
            "A version is digits, letters, ., + and -, up to 64 characters, starting with a digit, e.g. 0.24.2."
        );
    }
    let client = client()?;
    let mut read = {
        let mut kept = RELEASES.lock().await;
        match kept.as_ref() {
            Some((until, read)) if *until > Instant::now() => read.clone(),
            _ => {
                let read = read_releases(&client, GITHUB_API, github_token()).await?;
                *kept = Some((Instant::now() + KEEP_FOR, read.clone()));
                read
            }
        }
    };
    if let Some(version) = version {
        read.checked = Some(
            check_release(&client, GITHUB_API, github_token(), version, &read.releases).await?,
        );
    }
    Ok(read)
}

/// What the registry resolves `<worker>@latest` to, kept ten minutes.
pub(crate) async fn resolve(worker: &str) -> Result<WorkerResolution> {
    ensure!(
        worker_name(worker),
        "A worker's name is lowercase letters, digits and -, up to 64 characters, starting with a letter or a digit."
    );
    if let Some((until, kept)) = RESOLVED.lock().unwrap().get(worker) {
        if *until > Instant::now() {
            return Ok(kept.clone());
        }
    }
    let response = client()?
        .post(REGISTRY_RESOLVE)
        .json(&json!({"worker": worker, "version": "latest", "target": TARGET}))
        .send()
        .await
        .map_err(|error| anyhow!("The iii registry did not answer: {}", error.without_url()))?;
    let status = response.status().as_u16();
    let body = read_capped(response, "The iii registry's answer", MAX_BYTES).await?;
    let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let resolved = resolution(worker, status, &body)?;
    remember(
        &mut RESOLVED.lock().unwrap(),
        worker,
        resolved.clone(),
        Instant::now(),
        MAX_RESOLVED,
    );
    Ok(resolved)
}

/// Keep an answer ten minutes: the expired ones go first, then, at `max`,
/// the one that expires soonest.
fn remember<T>(
    kept: &mut BTreeMap<String, (Instant, T)>,
    worker: &str,
    resolved: T,
    now: Instant,
    max: usize,
) {
    kept.retain(|_, (until, _)| *until > now);
    while kept.len() >= max && !kept.contains_key(worker) {
        let Some(soonest) = kept
            .iter()
            .min_by_key(|(_, (until, _))| *until)
            .map(|(name, _)| name.clone())
        else {
            break;
        };
        kept.remove(&soonest);
    }
    kept.insert(worker.to_owned(), (now + KEEP_FOR, resolved));
}

/// An answer's body, refused past `max` bytes: at once when it says its
/// length, else as the chunks arrive.
async fn read_capped(mut response: reqwest::Response, what: &str, max: usize) -> Result<Vec<u8>> {
    let too_big = || anyhow!("{what} is past {} MB; it was not read.", max >> 20);
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| anyhow!("{what} could not be read: {}", error.without_url()))?
    {
        ensure!(body.len() + chunk.len() <= max, too_big());
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `^[a-z0-9][a-z0-9-]{0,63}$`.
fn worker_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && name.len() <= 64
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The registry's answer: the root's release and every other non-engine
/// worker of its graph, or that it has no such worker (a 404 or
/// `worker_not_found`). Anything else it answers (a rate limit, a server
/// error) is an error, never kept: the worker may well exist.
fn resolution(worker: &str, status: u16, body: &Value) -> Result<WorkerResolution> {
    let code = body["error"]["code"].as_str();
    let message = body["error"]["message"].as_str();
    if status == 404 || code == Some("worker_not_found") {
        return Ok(WorkerResolution::Refused {
            error: RegistryRefusal {
                code: code.unwrap_or("worker_not_found").to_owned(),
                message: message
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("The iii registry has no worker {worker}.")),
            },
        });
    }
    if !(200..300).contains(&status) || code.is_some() {
        match message.or(code) {
            Some(said) => bail!("The iii registry answered {status}: {said}"),
            None => bail!("The iii registry answered {status}."),
        }
    }
    let name = body["root"]["name"]
        .as_str()
        .context("The iii registry's answer names no root worker.")?;
    let version = body["root"]["version"]
        .as_str()
        .context("The iii registry's answer gives no version of its root worker.")?;
    let mut dependencies = body["graph"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["type"] != "engine")
        .filter_map(|node| node["name"].as_str())
        .filter(|dependency| *dependency != name)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    dependencies.sort();
    dependencies.dedup();
    Ok(WorkerResolution::Found {
        name: name.to_owned(),
        version: version.to_owned(),
        dependencies,
    })
}

async fn read_templates(reference: &str) -> Result<StackTemplates> {
    let client = client()?;
    let revision = commit_of(
        &client,
        &format!("{GITHUB_API}/repos/{TEMPLATES_REPOSITORY}/commits/{reference}"),
        github_token(),
        reference,
    )
    .await?;
    let raw = |path: &str| {
        format!("https://raw.githubusercontent.com/{TEMPLATES_REPOSITORY}/{revision}/iii/{path}")
    };
    let index = fetch(&client, &raw("template.yaml"))
        .await?
        .context("iii-hq/templates has no iii/template.yaml.")?;
    // Only the index and the revision take the list down; a template that
    // can't be read is listed with why.
    let reads = template_ids(&index)?.into_iter().map(|id| {
        let (client, raw) = (&client, &raw);
        async move {
            let read = async {
                let Some(manifest) = fetch(client, &raw(&format!("{id}/template.yaml"))).await?
                else {
                    return Ok(None);
                };
                let compose = fetch(client, &raw(&format!("{id}/worker-compose.yaml"))).await?;
                template(&id, &manifest, compose.as_deref()).map(Some)
            };
            read.await
                .unwrap_or_else(|error: anyhow::Error| Some(unreadable(&id, &error)))
        }
    });
    let templates = futures_util::future::join_all(reads)
        .await
        .into_iter()
        .flatten()
        .collect();
    Ok(StackTemplates {
        repository: TEMPLATES_REPOSITORY.into(),
        reference: reference.into(),
        revision,
        templates,
    })
}

/// The commit a revision of iii-hq/templates is at. GitHub answers 422 (or
/// 404) for one it does not hold.
async fn commit_of(
    client: &reqwest::Client,
    url: &str,
    token: Option<String>,
    reference: &str,
) -> Result<String> {
    let (status, body) = github_json(client, url, token, MAX_BYTES).await?;
    if matches!(status.as_u16(), 404 | 422) {
        bail!("iii-hq/templates has no commit, tag or branch {reference}.");
    }
    github_ok(status, body, url)?["sha"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("GitHub's answer for {url} has no sha."))
}

/// GitHub's JSON answer for `url`, read up to `max` bytes, and its status.
/// A token only raises GitHub's rate limit; one it refuses (401) is dropped
/// and the public repository asked again without it.
async fn github_json(
    client: &reqwest::Client,
    url: &str,
    token: Option<String>,
    max: usize,
) -> Result<(reqwest::StatusCode, Value)> {
    let ask = |token: Option<&str>| {
        let mut request = client
            .get(url)
            .header("accept", "application/vnd.github+json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        async move {
            request
                .send()
                .await
                .map_err(|error| anyhow!("GitHub did not answer: {}", error.without_url()))
        }
    };
    let mut response = ask(token.as_deref()).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && token.is_some() {
        tracing::warn!("GitHub refused GITHUB_TOKEN/GH_TOKEN; asking again without it");
        response = ask(None).await?;
    }
    let status = response.status();
    let body = read_capped(response, "GitHub's answer", max).await?;
    Ok((
        status,
        serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
    ))
}

/// A successful answer's body, else what GitHub said.
fn github_ok(status: reqwest::StatusCode, body: Value, url: &str) -> Result<Value> {
    if !status.is_success() {
        match body["message"].as_str() {
            Some(message) => bail!("GitHub answered {status} for {url}: {message}"),
            None => bail!("GitHub answered {status} for {url}."),
        }
    }
    Ok(body)
}

/// The newest releases of iii-hq/iii and, from its `iii/v*` tags, its
/// newest release candidate.
async fn read_releases(
    client: &reqwest::Client,
    api: &str,
    token: Option<String>,
) -> Result<IiiReleases> {
    let url = format!("{api}/repos/{III_REPOSITORY}/releases?per_page={RELEASES_PER_PAGE}");
    let (status, releases) = github_json(client, &url, token.clone(), MAX_RELEASES_BYTES).await?;
    let releases = github_ok(status, releases, &url)?;
    let url = format!("{api}/repos/{III_REPOSITORY}/git/matching-refs/tags/iii/v");
    let (status, refs) = github_json(client, &url, token, MAX_BYTES).await?;
    let refs = github_ok(status, refs, &url)?;
    Ok(IiiReleases {
        latest_candidate: newest_candidate(&refs),
        releases: releases
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(release)
            .collect(),
        checked: None,
    })
}

/// Whether `iii/v<version>` exists and publishes the CLI: from the list when
/// it holds it, else asked.
async fn check_release(
    client: &reqwest::Client,
    api: &str,
    token: Option<String>,
    version: &str,
    listed: &[IiiRelease],
) -> Result<IiiVersionCheck> {
    let found = match listed.iter().find(|listed| listed.version == version) {
        Some(listed) => Some(listed.clone()),
        None => {
            let url = format!("{api}/repos/{III_REPOSITORY}/releases/tags/iii/v{version}");
            match github_json(client, &url, token, MAX_BYTES).await? {
                (reqwest::StatusCode::NOT_FOUND, _) => None,
                (status, body) => release(&github_ok(status, body, &url)?),
            }
        }
    };
    Ok(IiiVersionCheck {
        version: version.to_owned(),
        release: found,
    })
}

/// A release of the iii CLI: tagged `iii/v…` and published.
fn release(release: &Value) -> Option<IiiRelease> {
    if release["draft"].as_bool() == Some(true) {
        return None;
    }
    Some(IiiRelease {
        version: release["tag_name"]
            .as_str()?
            .strip_prefix("iii/v")?
            .to_owned(),
        prerelease: release["prerelease"].as_bool().unwrap_or(false),
        published_at: release["published_at"].as_str().map(str::to_owned),
        cli: release["assets"]
            .as_array()
            .is_some_and(|assets| assets.iter().any(|asset| asset["name"] == CLI_ASSET)),
    })
}

/// What `iii: latest` means (`prepare_execution.py` newest_release_candidate):
/// the newest `X.Y.Z-rc.N` among the `iii/v*` tags, by core, then N. Stable
/// releases and other pre-releases are not candidates.
fn newest_candidate(refs: &Value) -> Option<String> {
    refs.as_array()?
        .iter()
        .filter_map(|tag| tag["ref"].as_str()?.strip_prefix("refs/tags/iii/v"))
        .filter_map(|version| Some((candidate(version)?, version)))
        .max()
        .map(|(_, version)| version.to_owned())
}

/// `^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-rc\.([1-9]\d*)$`, as numbers.
fn candidate(version: &str) -> Option<[u64; 4]> {
    let number = |part: &str| {
        let plain = !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'));
        plain.then(|| part.parse().ok()).flatten()
    };
    let (core, n) = version.split_once("-rc.")?;
    let mut core = core.split('.');
    let rank = [
        number(core.next()?)?,
        number(core.next()?)?,
        number(core.next()?)?,
        number(n)?,
    ];
    (core.next().is_none() && rank[3] > 0).then_some(rank)
}

/// A version that stays one segment of a URL: a digit, then up to 63 of
/// `A-Za-z0-9.+-`.
fn release_version(version: &str) -> bool {
    version.len() <= 64
        && version.starts_with(|first: char| first.is_ascii_digit())
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

fn github_token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|token| !token.is_empty()))
}

/// A file's text, or `None` when it does not exist.
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Option<String>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| anyhow!("GitHub did not answer: {}", error.without_url()))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    ensure!(status.is_success(), "GitHub answered {status} for {url}.");
    let body = read_capped(response, url, MAX_BYTES).await?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|_| anyhow!("{url} is not UTF-8 text."))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("harness-e2e/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// The template ids `iii/template.yaml` lists; one that is not a folder
/// name a stack could write (a worker's name) is left out, and logged.
fn template_ids(index: &str) -> Result<Vec<String>> {
    let index: serde_yaml::Value = serde_yaml::from_str(index)
        .map_err(|error| anyhow!("iii/template.yaml is not YAML: {error}"))?;
    let listed = index["templates"]
        .as_sequence()
        .context("iii/template.yaml lists no templates.")?;
    Ok(listed
        .iter()
        .filter_map(serde_yaml::Value::as_str)
        .filter(|id| {
            let valid = worker_name(id);
            if !valid {
                tracing::warn!(
                    id,
                    "iii/template.yaml lists an id that is not a template folder name"
                );
            }
            valid
        })
        .map(str::to_owned)
        .collect())
}

/// One template from its `template.yaml` and, when it ships one, its
/// `worker-compose.yaml`.
fn template(id: &str, manifest: &str, compose: Option<&str>) -> Result<StackTemplate> {
    let manifest: serde_yaml::Value = serde_yaml::from_str(manifest)
        .map_err(|error| anyhow!("iii/{id}/template.yaml is not YAML: {error}"))?;
    let text = |key: &str| manifest[key].as_str().unwrap_or_default().trim().to_owned();
    let workers = match compose {
        Some(compose) => {
            let compose: serde_yaml::Value = serde_yaml::from_str(compose)
                .map_err(|error| anyhow!("iii/{id}/worker-compose.yaml is not YAML: {error}"))?;
            compose["containers"]
                .as_mapping()
                .into_iter()
                .flatten()
                .filter_map(|(name, container)| {
                    Some(TemplateWorker {
                        name: name.as_str()?.to_owned(),
                        worker: container["worker"].as_str()?.to_owned(),
                        version: scalar(&container["version"]),
                    })
                })
                .collect()
        }
        None => Vec::new(),
    };
    let note = match (compose, workers.is_empty()) {
        (None, _) => Some("Ships no worker-compose.yaml, so a group would start nothing.".into()),
        (Some(_), true) => Some(
            "Its worker-compose.yaml declares no workers, so a group would start nothing.".into(),
        ),
        (Some(_), false) => None,
    };
    let name = match text("name") {
        name if name.is_empty() => id.to_owned(),
        name => name,
    };
    Ok(StackTemplate {
        id: id.into(),
        name,
        description: text("description"),
        workers,
        note,
        min_iii_version: scalar(&manifest["min_iii_version"]),
    })
}

/// A template whose files could not be read or parsed: listed without
/// workers, with the first line of why.
fn unreadable(id: &str, error: &anyhow::Error) -> StackTemplate {
    let why = error.to_string();
    let why = why.lines().next().unwrap_or_default();
    let why = match why.char_indices().nth(160) {
        Some((cut, _)) => format!("{}…", &why[..cut]),
        None => why.to_owned(),
    };
    StackTemplate {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        workers: Vec::new(),
        note: Some(format!("Couldn’t be read: {why}")),
        min_iii_version: None,
    }
}

/// A version as text, whether written quoted or not.
fn scalar(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(text) => Some(text.clone()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/stack-builder");

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap()
    }

    #[test]
    fn the_index_lists_the_template_folders_in_order() {
        assert_eq!(
            template_ids(&fixture("iii-template.yaml")).unwrap(),
            [
                "bare",
                "docker",
                "worker-bare",
                "quickstart",
                "starter",
                "harness",
                "harness-kanban",
                "linkly",
                "linkly-agentic"
            ]
        );
        assert!(template_ids("shared_files: []\n").is_err());
        // Only names a URL path and a stack's `template:` can hold.
        assert_eq!(
            template_ids("templates: [harness, ../evil, Bad, a/b, \"\", 7, ok-2]\n").unwrap(),
            ["harness", "ok-2"]
        );
    }

    #[test]
    fn a_template_lists_the_workers_its_compose_declares() {
        let harness = template(
            "harness",
            &fixture("harness-template.yaml"),
            Some(&fixture("harness-worker-compose.yaml")),
        )
        .unwrap();
        assert_eq!(
            (harness.name.as_str(), harness.description.as_str()),
            (
                "Harness",
                "Build with agents in the ADE, the iii agent workspace"
            )
        );
        assert_eq!(harness.note, None);
        // main has it commented out; a template that says it is read.
        assert_eq!(harness.min_iii_version, None);
        let declared =
            fixture("harness-template.yaml").replace("# min_iii_version", "min_iii_version");
        assert_eq!(
            template("harness", &declared, None)
                .unwrap()
                .min_iii_version
                .as_deref(),
            Some("0.22.0")
        );
        assert_eq!(
            harness
                .workers
                .iter()
                .map(|worker| worker.name.as_str())
                .collect::<Vec<_>>(),
            [
                "queue",
                "state",
                "session-manager",
                "llm-router",
                "provider-anthropic",
                "provider-openai",
                "provider-deepseek",
                "context-manager",
                "iii-directory",
                "cron",
                "ade",
                "ide",
                "harness",
                "browser"
            ]
        );
        assert_eq!(
            harness.workers[0],
            TemplateWorker {
                name: "queue".into(),
                worker: "package://queue".into(),
                version: Some("latest".into()),
            }
        );

        // A path worker has no version; the ones commented out are not declared.
        let linkly = template(
            "linkly",
            "name: Linkly\n",
            Some(&fixture("linkly-worker-compose.yaml")),
        )
        .unwrap();
        assert_eq!(
            linkly.workers,
            [
                TemplateWorker {
                    name: "http".into(),
                    worker: "package://http".into(),
                    version: Some("latest".into()),
                },
                TemplateWorker {
                    name: "state".into(),
                    worker: "package://state".into(),
                    version: Some("latest".into()),
                },
                TemplateWorker {
                    name: "link".into(),
                    worker: "path://./link".into(),
                    version: None,
                },
            ]
        );
    }

    #[test]
    fn a_template_without_workers_says_why() {
        let quickstart = template(
            "quickstart",
            "name: Quickstart\ndescription: Call a Python function\n",
            Some(&fixture("quickstart-worker-compose.yaml")),
        )
        .unwrap();
        assert!(quickstart.workers.is_empty());
        assert_eq!(
            quickstart.note.as_deref(),
            Some("Its worker-compose.yaml declares no workers, so a group would start nothing.")
        );
        let starter = template("starter", "description: A basic project\n", None).unwrap();
        assert_eq!(starter.name, "starter");
        assert_eq!(
            starter.note.as_deref(),
            Some("Ships no worker-compose.yaml, so a group would start nothing.")
        );
        // The answer leaves `note` out when there is none.
        let listed = serde_json::to_value(StackTemplates {
            repository: TEMPLATES_REPOSITORY.into(),
            reference: TEMPLATES_REF.into(),
            revision: "4077e670".into(),
            templates: vec![StackTemplate {
                note: None,
                ..starter
            }],
        })
        .unwrap();
        assert_eq!(listed["ref"], "main");
        assert!(listed["templates"][0].get("note").is_none());
    }

    #[test]
    fn a_template_that_cannot_be_read_is_listed_with_why() {
        let error = template("broken", "name: [", None).unwrap_err();
        let listed = unreadable("broken", &error);
        assert_eq!(
            (listed.id.as_str(), listed.name.as_str()),
            ("broken", "broken")
        );
        assert!(listed.workers.is_empty());
        let note = listed.note.unwrap();
        assert!(
            note.starts_with("Couldn’t be read: iii/broken/template.yaml is not YAML"),
            "{note}"
        );
        assert!(!note.contains('\n'));
        let long = unreadable("long", &anyhow!("{}\nmore", "x".repeat(400)));
        assert_eq!(
            long.note.unwrap(),
            format!("Couldn’t be read: {}…", "x".repeat(160))
        );
    }

    #[test]
    fn a_resolution_is_the_root_release_and_its_workers_without_the_engine() {
        let found = resolution(
            "harness",
            200,
            &serde_json::from_str(&fixture("resolve-harness.json")).unwrap(),
        )
        .unwrap();
        let WorkerResolution::Found {
            name,
            version,
            dependencies,
        } = &found
        else {
            panic!("{found:?}");
        };
        assert_eq!((name.as_str(), version.as_str()), ("harness", "1.8.36"));
        assert!(dependencies.contains(&"state".to_owned()));
        assert!(dependencies.contains(&"session-manager".to_owned()));
        for left_out in ["harness", "configuration", "iii-observability"] {
            assert!(!dependencies.contains(&left_out.to_owned()), "{left_out}");
        }
        assert!(dependencies.windows(2).all(|pair| pair[0] < pair[1]));
        let answer = serde_json::to_value(&found).unwrap();
        assert_eq!(answer["name"], "harness");
        assert!(answer.get("error").is_none());

        let refused = resolution(
            "nope-not-a-worker",
            404,
            &serde_json::from_str(&fixture("resolve-worker-not-found.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(refused).unwrap(),
            json!({"error": {
                "code": "worker_not_found",
                "message": "Worker 'nope-not-a-worker' was not found in the registry."
            }})
        );
        // A 404 without a body is still no such worker.
        assert_eq!(
            serde_json::to_value(resolution("gone", 404, &Value::Null).unwrap()).unwrap(),
            json!({"error": {
                "code": "worker_not_found",
                "message": "The iii registry has no worker gone."
            }})
        );
        // A rate limit or a server error is not an answer about the worker:
        // it is an error, with what the registry said, and never kept.
        for (status, code, message) in [
            (429, "rate_limited", "Too many requests, retry in 30s."),
            (503, "unavailable", "The registry is restarting."),
        ] {
            let error = resolution(
                "harness",
                status,
                &json!({"error": {"code": code, "message": message}}),
            )
            .unwrap_err()
            .to_string();
            assert_eq!(
                error,
                format!("The iii registry answered {status}: {message}")
            );
        }
        assert!(resolution("harness", 502, &Value::Null)
            .unwrap_err()
            .to_string()
            .contains("answered 502"));
    }

    #[tokio::test]
    async fn an_answer_past_1_mib_is_refused_as_it_arrives() {
        let answer = |body: Vec<u8>| reqwest::Response::from(axum::http::Response::new(body));
        let fits = read_capped(answer(vec![b'x'; MAX_BYTES]), "It", MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(fits.len(), MAX_BYTES);
        assert_eq!(
            read_capped(answer(vec![b'x'; MAX_BYTES + 1]), "The answer", MAX_BYTES)
                .await
                .unwrap_err()
                .to_string(),
            "The answer is past 1 MB; it was not read."
        );
    }

    #[tokio::test]
    async fn a_token_github_refuses_is_dropped_and_asked_again_without() {
        use std::sync::Arc;
        let asked = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new().route(
            "/commits/main",
            axum::routing::get({
                let asked = asked.clone();
                move |headers: axum::http::HeaderMap| {
                    let token = headers.contains_key("authorization");
                    asked.lock().unwrap().push(token);
                    async move {
                        if token {
                            (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(json!({"message": "Bad credentials"})),
                            )
                        } else {
                            (
                                axum::http::StatusCode::OK,
                                axum::Json(json!({"sha": "4077e670"})),
                            )
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/commits/main", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = client().unwrap();
        assert_eq!(
            commit_of(&client, &url, Some("stale".into()), "main")
                .await
                .unwrap(),
            "4077e670"
        );
        assert_eq!(*asked.lock().unwrap(), [true, false]);
        // Without a token, once.
        asked.lock().unwrap().clear();
        commit_of(&client, &url, None, "main").await.unwrap();
        assert_eq!(*asked.lock().unwrap(), [false]);
    }

    #[test]
    fn resolutions_are_kept_ten_minutes_and_at_most_256() {
        let found = |version: &str| WorkerResolution::Found {
            name: "w".into(),
            version: version.into(),
            dependencies: Vec::new(),
        };
        let start = Instant::now();
        let mut kept = BTreeMap::new();
        for index in 0..300 {
            let now = start + Duration::from_millis(index);
            remember(
                &mut kept,
                &format!("w{index}"),
                found("1"),
                now,
                MAX_RESOLVED,
            );
        }
        assert_eq!(kept.len(), MAX_RESOLVED);
        // The ones that expire soonest went first.
        assert!(!kept.contains_key("w0") && kept.contains_key("w299"));
        // Asking again for one kept replaces it without evicting another.
        remember(
            &mut kept,
            "w299",
            found("2"),
            start + Duration::from_millis(400),
            MAX_RESOLVED,
        );
        assert_eq!(kept.len(), MAX_RESOLVED);
        assert_eq!(kept["w299"].1, found("2"));
        // Past ten minutes, everything else has expired.
        remember(
            &mut kept,
            "fresh",
            found("1"),
            start + KEEP_FOR + Duration::from_millis(350),
            MAX_RESOLVED,
        );
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["fresh", "w299"]);
    }

    #[tokio::test]
    async fn a_name_that_is_not_a_worker_is_refused_before_asking() {
        for name in ["harness", "provider-zai", "0x", &"a".repeat(64)] {
            assert!(worker_name(name), "{name}");
        }
        for name in ["", "-x", "Harness", "a_b", "a/b", &"a".repeat(65)] {
            assert!(!worker_name(name), "{name}");
            assert!(resolve(name)
                .await
                .unwrap_err()
                .to_string()
                .contains("lowercase letters"));
        }
    }

    #[test]
    fn iii_releases_are_the_cli_tags_without_alphas_and_the_newest_candidate() {
        let listed: Value = serde_json::from_str(&fixture("iii-releases.json")).unwrap();
        let releases = listed
            .as_array()
            .unwrap()
            .iter()
            .filter_map(release)
            .collect::<Vec<_>>();
        assert_eq!(
            releases
                .iter()
                .map(|r| r.version.as_str())
                .collect::<Vec<_>>(),
            [
                "0.24.3",
                "0.24.3-rc.1",
                "0.24.2",
                "0.24.1",
                "0.24.2-rc.2",
                "0.24.2-rc.1",
                "0.24.0-rc.2",
                "0.24.0"
            ]
        );
        assert_eq!(
            releases[1],
            IiiRelease {
                version: "0.24.3-rc.1".into(),
                prerelease: true,
                published_at: Some("2026-09-25T13:59:34Z".into()),
                cli: true,
            }
        );
        assert!(!releases[0].prerelease);
        // A release without the CLI's archive is listed as one.
        let mut bare = listed[0].clone();
        bare["assets"] = json!([{"name": "iii-x86_64-apple-darwin.tar.gz"}]);
        assert!(!release(&bare).unwrap().cli);
        bare["draft"] = json!(true);
        assert_eq!(release(&bare), None);

        let refs: Value = serde_json::from_str(&fixture("iii-tag-refs.json")).unwrap();
        assert_eq!(newest_candidate(&refs).as_deref(), Some("0.24.3-rc.1"));
        let tags = |versions: &[&str]| {
            json!(versions
                .iter()
                .map(|version| json!({"ref": format!("refs/tags/iii/v{version}")}))
                .collect::<Vec<_>>())
        };
        // By core, then N as a number; stable and other pre-releases never.
        assert_eq!(
            newest_candidate(&tags(&[
                "0.23.0-rc.9",
                "0.23.0-rc.10",
                "0.23.0",
                "0.24.0-dry-run.1"
            ]))
            .as_deref(),
            Some("0.23.0-rc.10")
        );
        assert_eq!(
            newest_candidate(&tags(&[
                "01.2.3-rc.1",
                "1.2.3-rc.0",
                "1.2-rc.1",
                "1.2.3.4-rc.1"
            ])),
            None
        );
    }

    #[tokio::test]
    async fn a_typed_version_is_found_in_the_list_or_asked_for_by_its_tag() {
        use std::sync::Arc;
        let listed: Value = serde_json::from_str(&fixture("iii-releases.json")).unwrap();
        let refs: Value = serde_json::from_str(&fixture("iii-tag-refs.json")).unwrap();
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let app = axum::Router::new()
            .route(
                "/repos/iii-hq/iii/releases",
                axum::routing::get({
                    let (asked, listed) = (asked.clone(), listed.clone());
                    move |query: axum::extract::RawQuery| {
                        asked
                            .lock()
                            .unwrap()
                            .push(format!("list?{}", query.0.unwrap_or_default()));
                        let first = json!(listed.as_array().unwrap()[..2]);
                        async move { axum::Json(first) }
                    }
                }),
            )
            .route(
                "/repos/iii-hq/iii/git/matching-refs/tags/iii/v",
                axum::routing::get(move || async move { axum::Json(refs) }),
            )
            .route(
                "/repos/iii-hq/iii/releases/tags/iii/:tag",
                axum::routing::get({
                    let asked = asked.clone();
                    move |axum::extract::Path(tag): axum::extract::Path<String>| {
                        asked.lock().unwrap().push(tag.clone());
                        let found = listed
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|release| release["tag_name"] == format!("iii/{tag}"))
                            .cloned();
                        async move {
                            match found {
                                Some(release) => (axum::http::StatusCode::OK, axum::Json(release)),
                                None => (
                                    axum::http::StatusCode::NOT_FOUND,
                                    axum::Json(json!({"message": "Not Found", "status": "404"})),
                                ),
                            }
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = client().unwrap();

        let read = read_releases(&client, &api, None).await.unwrap();
        assert_eq!(read.latest_candidate.as_deref(), Some("0.24.3-rc.1"));
        assert_eq!(read.releases.len(), 2);
        assert_eq!(*asked.lock().unwrap(), ["list?per_page=20"]);

        asked.lock().unwrap().clear();
        let listed = check_release(&client, &api, None, "0.24.3-rc.1", &read.releases)
            .await
            .unwrap();
        assert_eq!(listed.release.as_ref(), Some(&read.releases[1]));
        let older = check_release(&client, &api, None, "0.24.1", &read.releases)
            .await
            .unwrap();
        assert_eq!(
            older.release.map(|release| (release.version, release.cli)),
            Some(("0.24.1".into(), true))
        );
        let missing = check_release(&client, &api, None, "9.9.9", &read.releases)
            .await
            .unwrap();
        assert_eq!((missing.version.as_str(), missing.release), ("9.9.9", None));
        // Only what the list does not hold is asked for.
        assert_eq!(*asked.lock().unwrap(), ["v0.24.1", "v9.9.9"]);
    }

    #[tokio::test]
    async fn a_typed_version_that_is_not_one_url_segment_is_refused_before_asking() {
        for version in ["0.24.2", "0.24.3-rc.1", "1.0.0+build.5"] {
            assert!(release_version(version), "{version}");
        }
        for version in [
            "",
            "latest",
            "v0.24.2",
            "0.24/../x",
            "0.24.2?x",
            &"1".repeat(65),
        ] {
            assert!(!release_version(version), "{version}");
            assert!(iii_releases(Some(version))
                .await
                .unwrap_err()
                .to_string()
                .contains("A version is"));
        }
    }

    #[tokio::test]
    async fn a_revision_is_checked_before_asking_and_one_github_lacks_says_so() {
        for revision in [
            "main",
            "v1.2.0",
            "feat/stack-sheet",
            "4077e670",
            "release_1.x",
        ] {
            assert!(template_revision(revision), "{revision}");
        }
        for revision in [
            "",
            "../iii",
            "a..b",
            "a//b",
            "/main",
            "main/",
            "a b",
            "main?x",
            &"a".repeat(101),
        ] {
            assert!(!template_revision(revision), "{revision}");
            assert!(templates(Some(revision))
                .await
                .unwrap_err()
                .to_string()
                .contains("A revision is"));
        }
        let app = axum::Router::new().route(
            "/commits/:reference",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(
                        json!({"message": "No commit found for SHA: nope", "status": "422"}),
                    ),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/commits/nope", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            commit_of(&client().unwrap(), &url, None, "nope")
                .await
                .unwrap_err()
                .to_string(),
            "iii-hq/templates has no commit, tag or branch nope."
        );
    }
}
