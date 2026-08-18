use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::schemas::*;
use super::*;

pub const MAX_RESULT_BYTES: u64 = 128 * 1024;
pub const MAX_PATCH_BYTES: usize = 256 * 1024;
const MAX_SCAN_FILES: usize = 2_000;
const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn text_value(value: impl Into<String>) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::TextUtf8,
        value: Value::String(value.into()),
    }
}

pub(crate) fn bool_value(value: bool) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::Boolean,
        value: Value::Bool(value),
    }
}

pub(crate) fn json_value(value: Value) -> TypedPortValue {
    TypedPortValue {
        kind: PortValueKind::Json,
        value,
    }
}

pub(crate) fn assessment_value(value: WorkflowEvaluationResult) -> Result<TypedPortValue> {
    Ok(TypedPortValue {
        kind: PortValueKind::Assessment,
        value: serde_json::to_value(value)?,
    })
}

pub(crate) fn gate(
    id: &str,
    passed: bool,
    reason: impl Into<String>,
    evidence_ids: impl IntoIterator<Item = String>,
) -> WorkflowGateResult {
    WorkflowGateResult {
        id: id.into(),
        passed,
        reason: reason.into(),
        evidence_ids: evidence_ids.into_iter().collect(),
    }
}

pub(crate) fn evaluation(
    id: &str,
    passed: bool,
    summary: impl Into<String>,
    evidence_ids: impl IntoIterator<Item = String>,
) -> WorkflowEvaluationResult {
    WorkflowEvaluationResult {
        id: id.into(),
        outcome: if passed {
            WorkflowEvaluationOutcome::Passed
        } else {
            WorkflowEvaluationOutcome::Failed
        },
        summary: summary.into(),
        score: Some(if passed { 1.0 } else { 0.0 }),
        evidence_ids: evidence_ids.into_iter().collect(),
    }
}

pub(crate) fn asset(step_id: &str, id: &str, kind: &str, content: Value) -> CapturedWorkflowAsset {
    CapturedWorkflowAsset {
        id: id.into(),
        kind: kind.into(),
        media_type: "application/json".into(),
        content: WorkflowAssetContent::Json(content),
        provenance: vec![WorkflowProvenance {
            source_step_id: step_id.into(),
            relation: "captured_before_cleanup".into(),
        }],
    }
}

pub(crate) fn text_asset(
    step_id: &str,
    id: &str,
    kind: &str,
    media_type: &str,
    content: String,
) -> CapturedWorkflowAsset {
    CapturedWorkflowAsset {
        id: id.into(),
        kind: kind.into(),
        media_type: media_type.into(),
        content: WorkflowAssetContent::TextUtf8(content),
        provenance: vec![WorkflowProvenance {
            source_step_id: step_id.into(),
            relation: "captured_before_cleanup".into(),
        }],
    }
}

pub(crate) fn fixture_path() -> Result<PathBuf> {
    let path = std::env::var_os(crate::scenarios::incident_response::FIXTURE_PATH_ENV)
        .map(PathBuf::from)
        .with_context(|| {
            format!(
                "{} must point to an environment-prepared disposable clone",
                crate::scenarios::incident_response::FIXTURE_PATH_ENV
            )
        })?;
    if !path.is_absolute() {
        bail!("incident fixture path must be absolute: {}", path.display());
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize incident fixture {}", path.display()))?;
    if canonical != path {
        bail!("incident fixture path must already be canonical");
    }
    if !canonical.join(".git").is_dir() {
        bail!("incident fixture is not a standalone Git clone");
    }
    Ok(canonical)
}

pub(crate) async fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("run git {} in {}", args.join(" "), path.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

pub(crate) fn validate_sha(sha: &str) -> Result<()> {
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("'{sha}' is not a full 40-character Git SHA");
    }
    Ok(())
}

pub(crate) async fn ensure_clean(path: &Path) -> Result<()> {
    let status = git(path, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !status.is_empty() {
        bail!("incident fixture clone is not clean: {status}");
    }
    Ok(())
}

pub(crate) async fn production_changes(path: &Path) -> Result<Vec<String>> {
    let status = git(path, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    Ok(status
        .lines()
        .filter_map(|line| line.get(3..))
        .map(str::trim)
        .filter(|path| !path.starts_with(".harness-e2e/"))
        .map(str::to_string)
        .collect())
}

pub(crate) fn validate_fixture_tree(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("read fixture directory {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path == root.join(".git") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                let resolved = path.canonicalize()?;
                if !resolved.starts_with(root) {
                    bail!("fixture symlink escapes repository: {}", path.display());
                }
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                bail!("fixture contains unsupported entry: {}", path.display());
            }
            files += 1;
            bytes = bytes.saturating_add(metadata.len());
            if files > MAX_SCAN_FILES || bytes > MAX_SCAN_BYTES {
                bail!("fixture exceeds bounded preflight scan limits");
            }
            if metadata.len() <= 256 * 1024 {
                let content = fs::read(&path)?;
                let text = String::from_utf8_lossy(&content);
                for marker in [
                    "-----BEGIN PRIVATE KEY-----",
                    "-----BEGIN OPENSSH PRIVATE KEY-----",
                    "ghp_",
                    "AKIA",
                ] {
                    if text.contains(marker) {
                        bail!(
                            "fixture contains credential-shaped content at {}",
                            path.display()
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn result_root(workspace: &Path, run_id: &str, attempt_id: &str) -> PathBuf {
    workspace.join(".harness-e2e").join(run_id).join(attempt_id)
}

pub(crate) fn result_path(
    workspace: &Path,
    run_id: &str,
    attempt_id: &str,
    node_id: &str,
) -> Result<PathBuf> {
    if !node_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("unsafe result node id '{node_id}'");
    }
    Ok(result_root(workspace, run_id, attempt_id).join(format!("{node_id}.json")))
}

pub(crate) fn load_result<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("required result file is missing: {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("result path is not a regular file: {}", path.display());
    }
    if metadata.len() == 0 || metadata.len() > MAX_RESULT_BYTES {
        bail!("result file size is outside 1..={MAX_RESULT_BYTES} bytes");
    }
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("decode structured result {}", path.display()))
}

pub(crate) fn validate_relative_repo_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(crate) fn allowed_production_path(value: &str) -> bool {
    validate_relative_repo_path(value)
        && value.starts_with("src/")
        && !value.starts_with("src/tests/")
}

pub(crate) fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum
}

pub(crate) fn output_with_asset(
    outputs: BTreeMap<String, TypedPortValue>,
    captured: CapturedWorkflowAsset,
    hard_gates: Vec<WorkflowGateResult>,
    evaluations: Vec<WorkflowEvaluationResult>,
) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs,
        captured_assets: vec![captured],
        evaluation: StepEvaluation {
            hard_gates,
            evaluations,
        },
        ..StepExecutorOutput::default()
    }
}

fn schema_hash<T: JsonSchema>() -> Result<String> {
    let schema = schemars::schema_for!(T);
    crate::artifact::sha256_value(&schema)
}

pub(crate) fn required_contract(function_id: &str) -> Result<RequiredFunctionContract> {
    let (request, response) = match function_id {
        PREFLIGHT_FUNCTION => (
            schema_hash::<FixturePreflightRequest>()?,
            schema_hash::<FixturePreflightResponse>()?,
        ),
        BASELINE_FUNCTION => (
            schema_hash::<BaselineRequest>()?,
            schema_hash::<BaselineResponse>()?,
        ),
        ALERT_FUNCTION => (
            schema_hash::<AlertRequest>()?,
            schema_hash::<AlertResponse>()?,
        ),
        REPRODUCE_FUNCTION => (
            schema_hash::<ReproduceRequest>()?,
            schema_hash::<ReproduceResponse>()?,
        ),
        TELEMETRY_FUNCTION => (
            schema_hash::<TelemetryRequest>()?,
            schema_hash::<TelemetryResponse>()?,
        ),
        VALIDATE_FUNCTION => (
            schema_hash::<ValidateRequest>()?,
            schema_hash::<ValidateResponse>()?,
        ),
        DEPLOY_FUNCTION => (
            schema_hash::<DeployRequest>()?,
            schema_hash::<DeployResponse>()?,
        ),
        RECONCILE_FUNCTION => (
            schema_hash::<ReconcileRequest>()?,
            schema_hash::<ReconcileResponse>()?,
        ),
        RESET_FUNCTION => (
            schema_hash::<ResetRequest>()?,
            schema_hash::<ResetResponse>()?,
        ),
        other => bail!("unknown incident fixture function '{other}'"),
    };
    Ok(RequiredFunctionContract {
        function_id: function_id.into(),
        request_schema_sha256: Some(request),
        response_schema_sha256: Some(response),
    })
}

pub(crate) fn validate_contract_info(info: &Value) -> Result<()> {
    let functions = info
        .get("functions")
        .and_then(Value::as_array)
        .context("engine::functions::info response is missing functions[]")?;
    for function_id in FIXTURE_FUNCTIONS {
        let function = functions
            .iter()
            .find(|function| {
                function.get("function_id").and_then(Value::as_str) == Some(function_id)
            })
            .with_context(|| format!("functions::info omitted '{function_id}'"))?;
        let expected = required_contract(function_id)?;
        let request = function
            .get("request_schema")
            .filter(|value| value.is_object())
            .with_context(|| format!("function '{function_id}' omitted request_schema"))?;
        let response = function
            .get("response_schema")
            .filter(|value| value.is_object())
            .with_context(|| format!("function '{function_id}' omitted response_schema"))?;
        if expected.request_schema_sha256.as_deref()
            != Some(crate::artifact::sha256_value(request)?.as_str())
            || expected.response_schema_sha256.as_deref()
                != Some(crate::artifact::sha256_value(response)?.as_str())
        {
            bail!("function '{function_id}' schema differs from incident_fixture::v1");
        }
    }
    Ok(())
}

pub(crate) fn result_contract_value<T: JsonSchema>() -> Result<Value> {
    Ok(serde_json::to_value(schemars::schema_for!(T))?)
}
