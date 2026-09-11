//! Build a data-driven trending-topics blog and validate the delivered Git revision.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use iii_sdk::{runtime::FunctionRef, RegisterFunction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

use super::*;
use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

pub const ID: &str = "trending_topics_build";
pub const VERSION: u32 = 1;

const FIXTURE_URL: &str = "git@github.com:iii-hq/e2e-fixture.git";
const FIXTURE_REVISION: &str = "3ee24f7ace3c014db35423f14939ad3f6ce0c3d2";
const APP_BASELINE: &str = "76a23abebff553a8ccf1397bd2f991c273bac03c";
const DELIVERABLE_ID: &str = "trending_topics_build_evidence";

const CRITERIA: [(&str, u8, &str); 10] = [
    ("B01", 20, "Is the final clean build branch a non-merge descendant of the immutable baseline, pushed to the attempt remote with only allowed paths changed?"),
    ("B02", 10, "Does the frozen install, build, and preview lifecycle complete successfully?"),
    ("B03", 15, "Does the home page show the edition and exactly the six input topics with their associated ranks and titles?"),
    ("B04", 10, "Are the topics presented in ascending numeric rank in both DOM and visual reading order?"),
    ("B05", 15, "Does every article route show its exact title and complete visible source body?"),
    ("B06", 10, "Do the home, article, All topics, and browser Back navigation flows work?"),
    ("B07", 5, "Do direct entry and reload work for every article URL?"),
    ("B08", 5, "Does every article expose a Source link with the exact input URL?"),
    ("B09", 5, "Do pages have the required semantic structure and keyboard-operable links with visible focus?"),
    ("B10", 5, "Do desktop and mobile layouts avoid horizontal overflow and keep required links unobstructed?"),
];

const RUNTIME_ASSETS: [(&str, &[u8]); 7] = [
    (
        "lifecycle.py",
        include_bytes!("../../tests/fixtures/trending-topics-build/lifecycle.py"),
    ),
    (
        "Dockerfile",
        include_bytes!("../../tests/fixtures/trending-topics-build/Dockerfile"),
    ),
    (
        "evaluate.mjs",
        include_bytes!("../../tests/fixtures/trending-topics-build/evaluate.mjs"),
    ),
    (
        "acceptance.spec.mjs",
        include_bytes!("../../tests/fixtures/trending-topics-build/acceptance.spec.mjs"),
    ),
    (
        "playwright.config.mjs",
        include_bytes!("../../tests/fixtures/trending-topics-build/playwright.config.mjs"),
    ),
    (
        "package.json",
        include_bytes!("../../tests/fixtures/trending-topics-build/package.json"),
    ),
    (
        "package-lock.json",
        include_bytes!("../../tests/fixtures/trending-topics-build/package-lock.json"),
    ),
];

fn root(run_id: &str) -> PathBuf {
    std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("harness-e2e-trending-topics-build")
        .join(crate::artifact::sha256_bytes(run_id.as_bytes()).replace("sha256:", ""))
}

fn assets(run_id: &str) -> PathBuf {
    root(run_id).with_extension("assets")
}

fn function_id(run_id: &str) -> String {
    format!(
        "{ID}_{}::exec",
        crate::artifact::sha256_bytes(run_id.as_bytes()).replace("sha256:", "")
    )
}

pub fn required_functions(run_id: &str) -> Vec<String> {
    vec![function_id(run_id)]
}

pub fn allowed_functions(run_id: &str) -> Vec<String> {
    vec![
        function_id(run_id),
        "engine::functions::list".into(),
        "engine::functions::info".into(),
    ]
}

fn registrations() -> &'static Mutex<HashMap<String, FunctionRef>> {
    static FUNCTIONS: OnceLock<Mutex<HashMap<String, FunctionRef>>> = OnceLock::new();
    FUNCTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Build the application described by `/workspace/TASK.md`. Use `{}` for every file read, edit, Git operation, build, and test command. Each command starts in `/workspace` inside your private container. Clone `git://127.0.0.1:9418/origin.git` into `/workspace/app` on branch `build`, make and commit the complete implementation there, and push the final commit to `origin/build`. Do not publish or deploy outside this attempt. Supply `command` and `timeout_ms` (1..=120000). Use function discovery only to find this exact tool.",
            function_id(run_id)
        ),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 96,
            max_output_tokens: Some(32_768),
            max_total_tokens: Some(1_200_000),
            stuck_timeout_seconds: 1_200,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: CRITERIA
            .iter()
            .map(|(id, weight, description)| {
                CriterionSpec::scored(id, *weight, description, EvaluationDimension::Deliverable)
            })
            .collect(),
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
}

pub fn materialize(namespace: &str, _seed: u64) -> Result<MaterializedScenario> {
    let asset_hashes = RUNTIME_ASSETS
        .iter()
        .map(|(name, bytes)| {
            (
                (*name).to_string(),
                json!(crate::artifact::sha256_bytes(bytes)),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case: ScenarioCase::new(
            ID,
            VERSION,
            stable_seed(ID),
            json!({
                "fixture_url": FIXTURE_URL,
                "fixture_revision": FIXTURE_REVISION,
                "app_baseline": APP_BASELINE,
                "branch": "build",
                "runtime_assets": asset_hashes,
            }),
            ComplexityProfile {
                planning_depth: 4,
                dependency_depth: 2,
                external_systems: 2,
                state_transitions: 4,
                validation_loops: 2,
                artifact_count: 1,
                ..Default::default()
            },
            vec![
                "iii::functions".into(),
                "docker".into(),
                "git".into(),
                "playwright".into(),
            ],
            deliverable_contract(),
        )?,
        capture: Some(capture),
    })
}

#[derive(Deserialize, JsonSchema)]
struct ExecInput {
    command: String,
    timeout_ms: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct ExecOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn lifecycle(run_id: &str, action: &str) -> Command {
    let mut command = Command::new("python3");
    command.env("III_TELEMETRY_ENABLED", "false");
    command
        .arg(assets(run_id).join("lifecycle.py"))
        .arg(action)
        .arg("--root")
        .arg(root(run_id))
        .arg("--assets")
        .arg(assets(run_id));
    command
}

async fn checked(command: &mut Command) -> Result<Value> {
    command.kill_on_drop(true);
    let output = command.output().await?;
    if !output.status.success() {
        bail!(
            "trending topics fixture command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("trending topics fixture JSON output")
}

fn write_assets(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    for (name, bytes) in RUNTIME_ASSETS {
        std::fs::write(directory.join(name), bytes)?;
    }
    Ok(())
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let directory = assets(run_id);
        write_assets(&directory)?;
        checked(&mut lifecycle(run_id, "prepare")).await?;

        let task_root = root(run_id);
        let function = context.client().register_function(
            function_id(run_id),
            RegisterFunction::new_async(move |input: ExecInput| {
                let directory = directory.clone();
                let task_root = task_root.clone();
                async move {
                    if !(1..=120_000).contains(&input.timeout_ms) {
                        return Err(iii_sdk::errors::Error::Handler(
                            "timeout_ms must be 1..=120000".into(),
                        ));
                    }
                    let mut command = Command::new("python3");
                    command.env("III_TELEMETRY_ENABLED", "false");
                    command
                        .arg(directory.join("lifecycle.py"))
                        .args(["exec", "--root"])
                        .arg(task_root)
                        .args(["--assets"])
                        .arg(&directory)
                        .args(["--command", &input.command, "--timeout-ms"])
                        .arg(input.timeout_ms.to_string());
                    let value = checked(&mut command)
                        .await
                        .map_err(|error| iii_sdk::errors::Error::Handler(error.to_string()))?;
                    serde_json::from_value::<ExecOutput>(value)
                        .map_err(|error| iii_sdk::errors::Error::Handler(error.to_string()))
                }
            })
            .description(
                "Execute a bounded command in this attempt's private trending-topics workspace.",
            ),
        );
        registrations()
            .lock()
            .unwrap()
            .insert(function_id(run_id), function);
        Ok(())
    })
}

fn read_result(run_id: &str) -> Result<Value> {
    serde_json::from_slice(&std::fs::read(root(run_id).join("result.json"))?)
        .context("decode trending topics result.json")
}

fn attachment_path(directory: &Path, dataset: &str, value: &Value) -> Result<PathBuf> {
    let relative = value
        .as_str()
        .context("evidence attachment path missing")?
        .strip_prefix("/evidence/")
        .context("evidence attachment must be rooted at /evidence")?;
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("evidence attachment path is not normalized");
    }
    let base = directory.join("evaluation").join(dataset);
    let path = base.join(relative);
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        bail!("evidence attachment is not a regular file");
    }
    if !path.canonicalize()?.starts_with(base.canonicalize()?) {
        bail!("evidence attachment escapes its dataset directory");
    }
    Ok(path)
}

fn add_file(
    files: &mut serde_json::Map<String, Value>,
    omitted: &mut Vec<Value>,
    name: String,
    path: &Path,
    used: &mut u64,
    limit: u64,
    required: bool,
) -> Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        bail!("capture source is not a regular file");
    }
    let raw_size = metadata.len();
    if raw_size > limit || used.saturating_add(raw_size) > limit {
        omitted.push(json!({"path": name, "reason": "capture_size_limit"}));
        if required {
            bail!("required screenshot exceeds the capture size limit");
        }
        return Ok(false);
    }
    let bytes = std::fs::read(path)?;
    let value = match std::str::from_utf8(&bytes) {
        Ok(content) => json!({
            "encoding": "utf8", "content": content,
            "size_bytes": bytes.len(), "sha256": crate::artifact::sha256_bytes(&bytes),
        }),
        Err(_) => json!({
            "encoding": "base64",
            "content": base64::engine::general_purpose::STANDARD.encode(&bytes),
            "size_bytes": bytes.len(), "sha256": crate::artifact::sha256_bytes(&bytes),
        }),
    };
    let size = serde_json::to_vec(&value)?.len() as u64 + name.len() as u64 + 8;
    if used.saturating_add(size) > limit {
        omitted.push(json!({"path": name, "reason": "capture_size_limit"}));
        if required {
            bail!("required screenshot exceeds the capture size limit");
        }
        return Ok(false);
    }
    *used += size;
    files.insert(name, value);
    Ok(true)
}

fn collect_regular_files(directory: &Path) -> Result<Vec<PathBuf>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut pending = vec![directory.to_path_buf()];
    let mut files = Vec::new();
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn portable_evidence_with_limit(directory: &Path, result: &mut Value, limit: u64) -> Result<Value> {
    let mut files = serde_json::Map::new();
    let mut omitted = Vec::new();
    let mut verification_errors = Vec::new();
    let mut used = 0;
    let files_limit = limit
        .saturating_sub(serde_json::to_vec(result)?.len() as u64)
        .saturating_sub(64 * 1024);
    let mut metadata_files = Vec::new();
    let delivered = result["delivery"]["remote_sha"]
        .as_str()
        .unwrap_or_default();
    let evidence = result["evidence"].as_array().cloned().unwrap_or_default();

    for dataset in ["original", "varied"] {
        for (project, viewport) in [("desktop", (1440, 900)), ("mobile", (390, 844))] {
            if !result["evaluations"][dataset].is_object() {
                for view in ["home", "article"] {
                    omitted.push(
                        json!({"path":format!("screenshots/{dataset}/{project}/{view}.png"),
                        "reason":"evaluation_not_run"}),
                    );
                }
                continue;
            }
            let matches = evidence
                .iter()
                .filter(|entry| entry["dataset"] == dataset && entry["project"] == project)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                verification_errors.push(format!(
                    "{dataset}/{project}: expected one evidence record, found {}",
                    matches.len()
                ));
                for view in ["home", "article"] {
                    omitted.push(
                        json!({"path":format!("screenshots/{dataset}/{project}/{view}.png"),
                        "reason":"capture_verification_failed"}),
                    );
                }
                continue;
            }
            let entry = matches[0];
            if entry["status"] != "passed" {
                for view in ["home", "article"] {
                    omitted.push(
                        json!({"path":format!("screenshots/{dataset}/{project}/{view}.png"),
                        "reason":"acceptance_not_passed","status":entry["status"]}),
                    );
                }
                continue;
            }
            let verify = (|| -> Result<Vec<(String, PathBuf)>> {
                let attachments = entry["attachments"]
                    .as_array()
                    .context("attachments missing")?;
                let named = |name: &str| -> Result<&Value> {
                    let found = attachments
                        .iter()
                        .filter(|item| item["name"] == name)
                        .collect::<Vec<_>>();
                    if found.len() != 1 {
                        bail!("expected one {name} attachment, found {}", found.len());
                    }
                    Ok(found[0])
                };
                let metadata_path =
                    attachment_path(directory, dataset, &named("evidence")?["path"])?;
                let records: Value = serde_json::from_slice(&std::fs::read(&metadata_path)?)?;
                let records = records
                    .as_array()
                    .context("evidence metadata must be an array")?;
                if records.len() != 2 {
                    bail!("evidence metadata must contain home and article records");
                }
                let feed_hash = result["evaluations"][dataset]["feed_sha256"]
                    .as_str()
                    .context("evaluation feed hash missing")?;
                let input_path = directory.join("inputs").join(format!("{dataset}.json"));
                if !std::fs::symlink_metadata(&input_path)?
                    .file_type()
                    .is_file()
                {
                    bail!("frozen feed is not a regular file");
                }
                let input_bytes = std::fs::read(&input_path)?;
                if crate::artifact::sha256_bytes(&input_bytes).strip_prefix("sha256:")
                    != Some(feed_hash)
                {
                    bail!("frozen feed bytes do not match the evaluation hash");
                }
                let input: Value = serde_json::from_slice(&input_bytes)?;
                let first_id = input["topics"]
                    .as_array()
                    .context("topics missing")?
                    .iter()
                    .min_by_key(|topic| topic["rank"].as_i64().unwrap_or(i64::MAX))
                    .and_then(|topic| topic["id"].as_str())
                    .context("lowest-ranked topic id missing")?;
                let mut screenshots = Vec::new();
                for (view, route) in [
                    ("home", "/".to_string()),
                    ("article", format!("/posts/{first_id}")),
                ] {
                    let record = records
                        .iter()
                        .find(|record| record["view"] == view)
                        .with_context(|| format!("{view} evidence metadata missing"))?;
                    if record["dataset"] != dataset
                        || record["commit"] != delivered
                        || record["feed_sha256"] != feed_hash
                        || record["viewport"]["width"] != viewport.0
                        || record["viewport"]["height"] != viewport.1
                        || record["browser"].as_str().is_none_or(str::is_empty)
                    {
                        bail!("{view} evidence identity does not match the evaluation");
                    }
                    let url = record["url"].as_str().context("evidence URL missing")?;
                    if url.strip_prefix("http://127.0.0.1:4173") != Some(route.as_str()) {
                        bail!("{view} evidence route does not match {route}");
                    }
                    let attachment = named(view)?;
                    if attachment["contentType"] != "image/png" {
                        bail!("{view} attachment is not a PNG");
                    }
                    let path = attachment_path(directory, dataset, &attachment["path"])?;
                    if std::fs::metadata(&path)?.len() > crate::asset::DEFAULT_MAX_CAPTURE_BYTES {
                        bail!("{view} attachment exceeds the capture size limit");
                    }
                    let bytes = std::fs::read(&path)?;
                    if !bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
                        bail!("{view} attachment has an invalid PNG signature");
                    }
                    let expected_hash = record["screenshot_sha256"]
                        .as_str()
                        .context("screenshot hash missing")?;
                    if crate::artifact::sha256_bytes(&bytes).strip_prefix("sha256:")
                        != Some(expected_hash)
                    {
                        bail!("{view} attachment hash does not match its evidence metadata");
                    }
                    screenshots.push((format!("screenshots/{dataset}/{project}/{view}.png"), path));
                }
                metadata_files.push((format!("evidence/{dataset}/{project}.json"), metadata_path));
                Ok(screenshots)
            })();
            match verify {
                Ok(screenshots) => {
                    for (name, path) in screenshots {
                        if let Err(error) = add_file(
                            &mut files,
                            &mut omitted,
                            name,
                            &path,
                            &mut used,
                            files_limit,
                            dataset == "original",
                        ) {
                            verification_errors.push(format!("{dataset}/{project}: {error:#}"));
                        }
                    }
                }
                Err(error) => {
                    verification_errors.push(format!("{dataset}/{project}: {error:#}"));
                    for view in ["home", "article"] {
                        omitted.push(
                            json!({"path":format!("screenshots/{dataset}/{project}/{view}.png"),
                            "reason":"capture_verification_failed"}),
                        );
                    }
                }
            }
        }
    }

    if !verification_errors.is_empty() {
        result["complete"] = json!(false);
        result["evidence_complete"] = json!(false);
        let errors = result["infrastructure_errors"]
            .as_array_mut()
            .context("infrastructure_errors missing")?;
        errors.extend(
            verification_errors
                .iter()
                .map(|message| json!({"kind": "capture_verification", "message": message})),
        );
    }
    result["capture_verification_errors"] = json!(verification_errors);
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(result)?,
    )?;

    let mut supporting = ["original", "varied"]
        .into_iter()
        .map(|dataset| directory.join("inputs").join(format!("{dataset}.json")))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    for (name, path) in metadata_files {
        add_file(
            &mut files,
            &mut omitted,
            name,
            &path,
            &mut used,
            files_limit,
            false,
        )?;
    }
    for dataset in ["original", "varied"] {
        let evaluation = directory.join("evaluation").join(dataset);
        for name in [
            "results.json",
            "build.stdout.log",
            "build.stderr.log",
            "preview.stdout.log",
            "preview.stderr.log",
            "playwright.stdout.log",
            "playwright.stderr.log",
        ] {
            let path = evaluation.join(name);
            if path.is_file() {
                supporting.push(path);
            }
        }
        supporting.extend(
            collect_regular_files(&evaluation.join("test-results"))?
                .into_iter()
                .filter(|path| path.extension().is_some_and(|extension| extension == "zip")),
        );
    }
    supporting.extend(collect_regular_files(&directory.join("commands"))?);
    for path in supporting {
        let relative = path.strip_prefix(directory)?.to_string_lossy().into_owned();
        if files.contains_key(&relative) {
            continue;
        }
        add_file(
            &mut files,
            &mut omitted,
            relative,
            &path,
            &mut used,
            files_limit,
            false,
        )?;
    }
    Ok(json!({
        "format": "trending-topics-evidence-v1",
        "files": files,
        "omitted_files": omitted,
        "capture_verification_errors": result["capture_verification_errors"],
    }))
}

fn portable_evidence(directory: &Path, result: &mut Value) -> Result<Value> {
    portable_evidence_with_limit(directory, result, crate::asset::DEFAULT_MAX_CAPTURE_BYTES)
}

fn awards(result: &Value) -> Result<Vec<CriterionAward>> {
    let observations = result["criteria"].as_array().context("criteria missing")?;
    if observations.len() != CRITERIA.len() {
        bail!(
            "validation returned {} of {} criteria",
            observations.len(),
            CRITERIA.len()
        );
    }
    CRITERIA
        .iter()
        .map(|(id, weight, _)| {
            let matches = observations
                .iter()
                .filter(|observation| observation["id"] == *id)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                bail!("missing or duplicate criterion {id}");
            }
            let observation = matches[0];
            let awarded = match observation["status"].as_str() {
                Some("passed") => Some(*weight),
                Some("failed") => Some(0),
                Some("unverified") => None,
                status => bail!("criterion {id} has invalid status {status:?}"),
            };
            Ok(CriterionAward {
                id: (*id).into(),
                awarded,
                reason: observation.to_string(),
            })
        })
        .collect()
}

fn objective_evaluation(subject_complete: bool, result: &Value) -> Result<ObjectiveEvaluation> {
    let result_complete = result["complete"].as_bool().context("complete missing")?;
    let errors = result["infrastructure_errors"]
        .as_array()
        .context("infrastructure_errors missing")?;
    Ok(ObjectiveEvaluation {
        completion: if !errors.is_empty() {
            CompletionState::Undetermined
        } else if subject_complete && result_complete {
            CompletionState::Completed
        } else {
            CompletionState::TaskIncomplete
        },
        awards: awards(result)?,
        infrastructure_error: (!errors.is_empty()).then(|| {
            format!(
                "validation infrastructure failed: {}",
                result["infrastructure_errors"]
            )
        }),
    })
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let mut result = checked(&mut lifecycle(run_id, "finish")).await?;
        let persisted = read_result(run_id)?;
        if result != persisted {
            bail!("finish output differs from persisted result.json");
        }
        let directory = root(run_id);
        let evidence = portable_evidence(&directory, &mut result)?;
        let content = json!({
            "run_id": run_id,
            "subject_complete": observation.metrics.complete,
            "result": result,
            "evidence": evidence,
        });
        if serde_json::to_vec(&content)?.len() as u64 > crate::asset::DEFAULT_MAX_CAPTURE_BYTES {
            bail!("trending topics evidence exceeds the capture size limit");
        }
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.into(),
            kind: "application_audit".into(),
            content: content.into(),
            invariants: vec![],
            provenance: vec![ProvenanceEvidence {
                kind: "filesystem_path".into(),
                source_id: "result.json".into(),
                relation: "validated_before_cleanup".into(),
            }],
        }])
    })
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(
        async move { objective_evaluation(observation.metrics.complete, &read_result(run_id)?) },
    )
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.into(),
            kind: "application_audit".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type": "object",
                "required": ["run_id", "subject_complete", "result", "evidence"]
            }),
            max_size_bytes: crate::asset::DEFAULT_MAX_CAPTURE_BYTES,
        }],
        invariants: vec![],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        registrations().lock().unwrap().remove(&function_id(run_id));
        if root(run_id).exists() {
            checked(&mut lifecycle(run_id, "cleanup")).await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3];

    fn result(status: &str, complete: bool) -> Value {
        json!({
            "criteria": CRITERIA.iter().map(|(id, _, _)| json!({"id":id,"status":status,"reason":"control"})).collect::<Vec<_>>(),
            "complete": complete,
            "infrastructure_errors": [],
            "delivery": {},
            "evaluations": {},
        })
    }

    fn capture_fixture(path: &Path, status: &str) -> Value {
        std::fs::create_dir_all(path.join("inputs")).unwrap();
        let mut evidence = Vec::new();
        let mut evaluations = serde_json::Map::new();
        for dataset in ["original", "varied"] {
            let feed = json!({"topics":[
                {"id":format!("{dataset}-later"),"rank":20},
                {"id":format!("{dataset}-first"),"rank":10}
            ]});
            let feed_bytes = serde_json::to_vec(&feed).unwrap();
            let feed_hash = crate::artifact::sha256_bytes(&feed_bytes)
                .trim_start_matches("sha256:")
                .to_string();
            std::fs::write(
                path.join("inputs").join(format!("{dataset}.json")),
                feed_bytes,
            )
            .unwrap();
            evaluations.insert(dataset.into(), json!({"feed_sha256":feed_hash}));
            for (project, width, height) in [("desktop", 1440, 900), ("mobile", 390, 844)] {
                let base = path
                    .join("evaluation")
                    .join(dataset)
                    .join("test-results")
                    .join(project);
                std::fs::create_dir_all(&base).unwrap();
                let hash = crate::artifact::sha256_bytes(PNG)
                    .trim_start_matches("sha256:")
                    .to_string();
                let records = json!([
                    {"view":"home","dataset":dataset,"commit":"delivered","feed_sha256":feed_hash,
                     "url":"http://127.0.0.1:4173/","viewport":{"width":width,"height":height},
                     "browser":"control","screenshot_sha256":hash},
                    {"view":"article","dataset":dataset,"commit":"delivered","feed_sha256":feed_hash,
                     "url":format!("http://127.0.0.1:4173/posts/{dataset}-first"),
                     "viewport":{"width":width,"height":height},"browser":"control","screenshot_sha256":hash}
                ]);
                std::fs::write(base.join("home.png"), PNG).unwrap();
                std::fs::write(base.join("article.png"), PNG).unwrap();
                std::fs::write(
                    base.join("evidence.json"),
                    serde_json::to_vec(&records).unwrap(),
                )
                .unwrap();
                evidence.push(json!({
                    "dataset":dataset,"project":project,"status":status,
                    "attachments":[
                        {"name":"home","contentType":"image/png","path":format!("/evidence/test-results/{project}/home.png")},
                        {"name":"article","contentType":"image/png","path":format!("/evidence/test-results/{project}/article.png")},
                        {"name":"evidence","contentType":"application/json","path":format!("/evidence/test-results/{project}/evidence.json")}
                    ]
                }));
            }
        }
        json!({
            "criteria":[], "complete":true, "infrastructure_errors":[],
            "delivery":{"remote_sha":"delivered"}, "evaluations":evaluations, "evidence":evidence,
        })
    }

    #[test]
    fn contract_weights_and_runtime_assets_are_exact() {
        assert_eq!(
            CRITERIA
                .iter()
                .map(|(_, weight, _)| u16::from(*weight))
                .sum::<u16>(),
            100
        );
        assert_eq!(
            CRITERIA.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
            ["B01", "B02", "B03", "B04", "B05", "B06", "B07", "B08", "B09", "B10"]
        );
        let names = RUNTIME_ASSETS
            .iter()
            .map(|(name, _)| *name)
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), RUNTIME_ASSETS.len());
        assert!(!names
            .iter()
            .any(|name| name.contains("reference") || *name == "validate-controls.mjs"));
    }

    #[test]
    fn allowlist_exposes_only_the_attempt_tool_and_discovery() {
        let allowed = allowed_functions("run");
        assert_eq!(allowed.len(), 3);
        assert_eq!(required_functions("run"), vec![function_id("run")]);
        assert!(allowed.contains(&"engine::functions::list".to_string()));
        assert!(allowed.contains(&"engine::functions::info".to_string()));
    }

    #[test]
    fn scoring_is_fail_closed_and_completion_needs_both_signals() {
        let passed = result("passed", true);
        assert_eq!(
            awards(&passed)
                .unwrap()
                .iter()
                .map(|award| u16::from(award.awarded.unwrap()))
                .sum::<u16>(),
            100
        );
        assert_eq!(
            objective_evaluation(true, &passed).unwrap().completion,
            CompletionState::Completed
        );
        assert_eq!(
            objective_evaluation(false, &passed).unwrap().completion,
            CompletionState::TaskIncomplete
        );
        assert_eq!(
            objective_evaluation(true, &result("passed", false))
                .unwrap()
                .completion,
            CompletionState::TaskIncomplete
        );

        let mut missing = passed.clone();
        missing["criteria"].as_array_mut().unwrap().pop();
        assert!(awards(&missing).is_err());
        assert!(awards(&result("unverified", false))
            .unwrap()
            .iter()
            .all(|award| award.awarded.is_none()));
    }

    #[test]
    fn case_identity_includes_fixture_baseline_and_asset_hashes() {
        let first = materialize("attempt-a", 1).unwrap();
        let retry = materialize("attempt-b", 99).unwrap();
        assert_eq!(first.case.case_id, retry.case.case_id);
        assert_eq!(first.case.inputs["fixture_revision"], FIXTURE_REVISION);
        assert_eq!(first.case.inputs["app_baseline"], APP_BASELINE);
        assert_eq!(
            first.case.inputs["runtime_assets"]
                .as_object()
                .unwrap()
                .len(),
            RUNTIME_ASSETS.len()
        );
    }

    #[test]
    fn portable_capture_bundles_verified_screenshot_bytes_and_hashes() {
        let temp = tempfile::tempdir().unwrap();
        let mut result = capture_fixture(temp.path(), "passed");
        let capture = portable_evidence_with_limit(temp.path(), &mut result, 1_000_000).unwrap();
        assert!(result["complete"].as_bool().unwrap());
        assert_eq!(capture["files"].as_object().unwrap().len(), 14);
        let image = &capture["files"]["screenshots/original/desktop/home.png"];
        assert_eq!(image["encoding"], "base64");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(image["content"].as_str().unwrap())
                .unwrap(),
            PNG
        );
        assert_eq!(image["sha256"], crate::artifact::sha256_bytes(PNG));
    }

    #[test]
    fn claimed_capture_missing_or_tampered_prevents_completion() {
        for mutation in ["missing", "tampered"] {
            let temp = tempfile::tempdir().unwrap();
            let mut result = capture_fixture(temp.path(), "passed");
            let image = temp
                .path()
                .join("evaluation/original/test-results/desktop/home.png");
            if mutation == "missing" {
                std::fs::remove_file(image).unwrap();
            } else {
                std::fs::write(image, [137, 80, 78, 71, 13, 10, 26, 10, 9]).unwrap();
            }
            let capture =
                portable_evidence_with_limit(temp.path(), &mut result, 1_000_000).unwrap();
            assert!(!result["complete"].as_bool().unwrap());
            assert!(!capture["capture_verification_errors"]
                .as_array()
                .unwrap()
                .is_empty());
            assert!(result["infrastructure_errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error["kind"] == "capture_verification"));
        }
    }

    #[test]
    fn attachment_paths_cannot_escape_or_follow_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let mut result = capture_fixture(temp.path(), "passed");
        result["evidence"][0]["attachments"][2]["path"] = json!("/evidence/../outside.json");
        portable_evidence_with_limit(temp.path(), &mut result, 1_000_000).unwrap();
        assert!(!result["complete"].as_bool().unwrap());
        assert!(result["capture_verification_errors"][0]
            .as_str()
            .unwrap()
            .contains("not normalized"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let mut result = capture_fixture(temp.path(), "passed");
            let path = temp
                .path()
                .join("evaluation/original/test-results/desktop/evidence.json");
            std::fs::remove_file(&path).unwrap();
            symlink(temp.path().join("inputs/original.json"), path).unwrap();
            portable_evidence_with_limit(temp.path(), &mut result, 1_000_000).unwrap();
            assert!(!result["complete"].as_bool().unwrap());
        }
    }

    #[test]
    fn unavailable_product_evidence_stays_explicit_without_becoming_infrastructure() {
        let temp = tempfile::tempdir().unwrap();
        let mut result = capture_fixture(temp.path(), "unverified");
        let capture = portable_evidence_with_limit(temp.path(), &mut result, 1_000_000).unwrap();
        assert!(capture["capture_verification_errors"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(capture["omitted_files"].as_array().unwrap().len(), 8);
        assert!(capture["omitted_files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["reason"] == "acceptance_not_passed"));
    }

    #[test]
    fn screenshot_budget_is_fail_closed_and_supporting_files_are_omitted() {
        let temp = tempfile::tempdir().unwrap();
        let mut result = capture_fixture(temp.path(), "passed");
        let capture = portable_evidence_with_limit(temp.path(), &mut result, 100).unwrap();
        assert!(!result["complete"].as_bool().unwrap());
        assert!(capture["files"].as_object().unwrap().is_empty());
        assert!(capture["omitted_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["reason"] == "capture_size_limit"));
    }
}
