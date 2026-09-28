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
    if let Some((at, kept)) = RESOLVED.lock().unwrap().get(worker) {
        if at.elapsed() < KEEP_FOR {
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
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    let resolved = resolution(status, &body)?;
    RESOLVED
        .lock()
        .unwrap()
        .insert(worker.to_owned(), (Instant::now(), resolved.clone()));
    Ok(resolved)
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
/// worker of its graph, or the error it gave.
fn resolution(status: u16, body: &Value) -> Result<WorkerResolution> {
    if let Some(code) = body["error"]["code"].as_str() {
        return Ok(WorkerResolution::Refused {
            error: RegistryRefusal {
                code: code.to_owned(),
                message: body["error"]["message"].as_str().unwrap_or(code).to_owned(),
            },
        });
    }
    ensure!(
        (200..300).contains(&status),
        "The iii registry answered {status}."
    );
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
    let reads = template_ids(&index)?.into_iter().map(|id| {
        let (client, raw) = (&client, &raw);
        async move {
            let Some(manifest) = fetch(client, &raw(&format!("{id}/template.yaml"))).await? else {
                return Ok(None);
            };
            let compose = fetch(client, &raw(&format!("{id}/worker-compose.yaml"))).await?;
            template(&id, &manifest, compose.as_deref()).map(Some)
        }
    });
    let templates = futures_util::future::try_join_all(reads)
        .await?
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
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
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
    Ok(Some(
        response
            .text()
            .await
            .with_context(|| format!("read {url}"))?,
    ))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("harness-e2e/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// The template ids `iii/template.yaml` lists.
fn template_ids(index: &str) -> Result<Vec<String>> {
    let index: serde_yaml::Value = serde_yaml::from_str(index)
        .map_err(|error| anyhow!("iii/template.yaml is not YAML: {error}"))?;
    let listed = index["templates"]
        .as_sequence()
        .context("iii/template.yaml lists no templates.")?;
    Ok(listed
        .iter()
        .filter_map(serde_yaml::Value::as_str)
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
    fn a_resolution_is_the_root_release_and_its_workers_without_the_engine() {
        let found = resolution(
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
        assert!(resolution(502, &Value::Null)
            .unwrap_err()
            .to_string()
            .contains("answered 502"));
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
