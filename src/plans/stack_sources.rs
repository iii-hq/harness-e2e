//! What the stack builder reads from outside this Console: the projects of
//! iii-hq/templates, with the workers each one's `worker-compose.yaml`
//! declares, and what the iii registry resolves a worker to. Both are public
//! and read without credentials (a `GITHUB_TOKEN` only raises GitHub's rate
//! limit); each answer is kept in memory for ten minutes.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackTemplates {
    /// `iii-hq/templates`.
    pub repository: String,
    /// The branch read: `main`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The commit `main` was at when it was read.
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

static TEMPLATES: Mutex<Option<(Instant, StackTemplates)>> = Mutex::new(None);
/// Each worker's resolution and until when it is kept.
static RESOLVED: Mutex<BTreeMap<String, (Instant, WorkerResolution)>> = Mutex::new(BTreeMap::new());

/// The projects of iii-hq/templates as `main` has them, kept ten minutes.
pub(crate) async fn templates() -> Result<StackTemplates> {
    if let Some((at, kept)) = TEMPLATES.lock().unwrap().as_ref() {
        if at.elapsed() < KEEP_FOR {
            return Ok(kept.clone());
        }
    }
    let read = read_templates().await?;
    *TEMPLATES.lock().unwrap() = Some((Instant::now(), read.clone()));
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
    let body = read_capped(response, "The iii registry's answer").await?;
    let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let resolved = resolution(worker, status, &body)?;
    remember(
        &mut RESOLVED.lock().unwrap(),
        worker,
        resolved.clone(),
        Instant::now(),
    );
    Ok(resolved)
}

/// Keep a resolution ten minutes: the expired ones go first, then, at
/// `MAX_RESOLVED`, the one that expires soonest.
fn remember(
    kept: &mut BTreeMap<String, (Instant, WorkerResolution)>,
    worker: &str,
    resolved: WorkerResolution,
    now: Instant,
) {
    kept.retain(|_, (until, _)| *until > now);
    while kept.len() >= MAX_RESOLVED && !kept.contains_key(worker) {
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

/// An answer's body, refused past `MAX_BYTES`: at once when it says its
/// length, else as the chunks arrive.
async fn read_capped(mut response: reqwest::Response, what: &str) -> Result<Vec<u8>> {
    let too_big = || anyhow!("{what} is past 1 MB; it was not read.");
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| anyhow!("{what} could not be read: {}", error.without_url()))?
    {
        ensure!(body.len() + chunk.len() <= MAX_BYTES, too_big());
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

async fn read_templates() -> Result<StackTemplates> {
    let client = client()?;
    let revision = main_revision(&client).await?;
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
        reference: TEMPLATES_REF.into(),
        revision,
        templates,
    })
}

/// The commit `main` of iii-hq/templates is at.
async fn main_revision(client: &reqwest::Client) -> Result<String> {
    let url =
        format!("https://api.github.com/repos/{TEMPLATES_REPOSITORY}/commits/{TEMPLATES_REF}");
    let mut request = client
        .get(&url)
        .header("accept", "application/vnd.github+json");
    let token = ["GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|token| !token.is_empty()));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| anyhow!("GitHub did not answer: {}", error.without_url()))?;
    let status = response.status();
    let body = read_capped(response, "GitHub's answer").await?;
    let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    if !status.is_success() {
        match body["message"].as_str() {
            Some(message) => bail!("GitHub answered {status} for {url}: {message}"),
            None => bail!("GitHub answered {status} for {url}."),
        }
    }
    body["sha"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("GitHub's answer for {url} has no sha."))
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
    let body = read_capped(response, url).await?;
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
        let fits = read_capped(answer(vec![b'x'; MAX_BYTES]), "It")
            .await
            .unwrap();
        assert_eq!(fits.len(), MAX_BYTES);
        assert_eq!(
            read_capped(answer(vec![b'x'; MAX_BYTES + 1]), "The answer")
                .await
                .unwrap_err()
                .to_string(),
            "The answer is past 1 MB; it was not read."
        );
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
            remember(&mut kept, &format!("w{index}"), found("1"), now);
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
        );
        assert_eq!(kept.len(), MAX_RESOLVED);
        assert_eq!(kept["w299"].1, found("2"));
        // Past ten minutes, everything else has expired.
        remember(
            &mut kept,
            "fresh",
            found("1"),
            start + KEEP_FOR + Duration::from_millis(350),
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
}
