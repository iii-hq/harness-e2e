use super::*;

pub(crate) fn output_with_asset(
    outputs: BTreeMap<String, TypedPortValue>,
    asset_id: &str,
    value: Value,
    step_id: &str,
) -> StepExecutorOutput {
    StepExecutorOutput {
        outputs,
        captured_assets: vec![CapturedWorkflowAsset {
            id: asset_id.into(),
            kind: "security_scan_evidence".into(),
            media_type: "application/json".into(),
            content: WorkflowAssetContent::Json(value),
            provenance: vec![WorkflowProvenance {
                source_step_id: step_id.into(),
                relation: "captured_before_cleanup".into(),
            }],
        }],
        ..StepExecutorOutput::default()
    }
}

pub(crate) fn output_with_internal_evaluation(
    mut output: StepExecutorOutput,
    mut gates: Vec<WorkflowGateResult>,
    mut evaluations: Vec<WorkflowEvaluationResult>,
) -> StepExecutorOutput {
    if let Some((asset, provenance)) = output.captured_assets.first().and_then(|asset| {
        asset
            .provenance
            .first()
            .map(|provenance| (asset, provenance))
    }) {
        let evidence_id = format!("{}.{}", provenance.source_step_id, asset.id);
        for gate in &mut gates {
            if gate.evidence_ids.is_empty() {
                gate.evidence_ids.push(evidence_id.clone());
            }
        }
        for evaluation in &mut evaluations {
            if evaluation.evidence_ids.is_empty() {
                evaluation.evidence_ids.push(evidence_id.clone());
            }
        }
    }
    output.evaluation = StepEvaluation {
        hard_gates: gates,
        evaluations,
    };
    output
}

pub(crate) fn operation_context(
    context: &StepExecutorContext,
    config: Value,
    inputs: BTreeMap<String, TypedPortValue>,
) -> StepExecutorContext {
    let mut operation = context.clone();
    operation.node.config = config;
    operation.inputs = inputs;
    operation
}

pub(crate) fn typed_inputs<const N: usize>(
    inputs: [(&str, TypedPortValue); N],
) -> BTreeMap<String, TypedPortValue> {
    inputs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

pub(crate) fn operation_output(output: &StepExecutorOutput, key: &str) -> Result<TypedPortValue> {
    output
        .outputs
        .get(key)
        .cloned()
        .with_context(|| format!("security review operation did not produce '{key}'"))
}

pub(crate) fn operation_output_string(output: &StepExecutorOutput, key: &str) -> Result<String> {
    operation_output(output, key)?
        .value
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("security review operation output '{key}' is not text"))
}

pub(crate) fn operation_output_bool(output: &StepExecutorOutput, key: &str) -> Result<bool> {
    operation_output(output, key)?
        .value
        .as_bool()
        .with_context(|| format!("security review operation output '{key}' is not boolean"))
}

pub(crate) fn operation_output_value(output: &StepExecutorOutput, key: &str) -> Result<Value> {
    Ok(operation_output(output, key)?.value)
}

pub(crate) fn append_operation(
    target: &mut StepExecutorOutput,
    mut operation: StepExecutorOutput,
    asset_id: &str,
) {
    for asset in &mut operation.captured_assets {
        let previous = asset.id.clone();
        asset.id = asset_id.to_string();
        let previous_evidence = asset
            .provenance
            .first()
            .map(|provenance| format!("{}.{}", provenance.source_step_id, previous));
        let current_evidence = asset
            .provenance
            .first()
            .map(|provenance| format!("{}.{}", provenance.source_step_id, asset_id));
        if let (Some(previous), Some(current)) = (previous_evidence, current_evidence) {
            for gate in &mut operation.evaluation.hard_gates {
                for evidence in &mut gate.evidence_ids {
                    if evidence == &previous {
                        *evidence = current.clone();
                    }
                }
            }
            for evaluation in &mut operation.evaluation.evaluations {
                for evidence in &mut evaluation.evidence_ids {
                    if evidence == &previous {
                        *evidence = current.clone();
                    }
                }
            }
        }
    }
    for gate in &mut operation.evaluation.hard_gates {
        gate.id = format!("{asset_id}.{}", gate.id);
    }
    for evaluation in &mut operation.evaluation.evaluations {
        evaluation.id = format!("{asset_id}.{}", evaluation.id);
    }
    target.captured_assets.extend(operation.captured_assets);
    target
        .evaluation
        .hard_gates
        .extend(operation.evaluation.hard_gates);
    target
        .evaluation
        .evaluations
        .extend(operation.evaluation.evaluations);
    if target.technical_failure.is_none() {
        target.technical_failure = operation.technical_failure;
    }
}

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

pub(crate) fn config_string(context: &StepExecutorContext, key: &str) -> Result<String> {
    required_string(&context.node.config, key)
}

pub(crate) fn config_bool(context: &StepExecutorContext, key: &str) -> Result<bool> {
    context
        .node
        .config
        .get(key)
        .and_then(Value::as_bool)
        .with_context(|| {
            format!(
                "node '{}' config is missing boolean '{key}'",
                context.node.id
            )
        })
}

pub(crate) fn config_u64(context: &StepExecutorContext, key: &str) -> Result<u64> {
    context
        .node
        .config
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| {
            format!(
                "node '{}' config is missing integer '{key}'",
                context.node.id
            )
        })
}

pub(crate) fn input_string(context: &StepExecutorContext, key: &str) -> Result<String> {
    context
        .inputs
        .get(key)
        .and_then(|value| value.value.as_str())
        .map(str::to_string)
        .with_context(|| format!("node '{}' input is missing string '{key}'", context.node.id))
}

pub(crate) fn input_value<'a>(context: &'a StepExecutorContext, key: &str) -> Result<&'a Value> {
    context
        .inputs
        .get(key)
        .map(|value| &value.value)
        .with_context(|| format!("node '{}' input is missing '{key}'", context.node.id))
}

pub(crate) fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("response is missing non-empty string '{key}'"))
}

pub(crate) fn validate_sha(sha: &str) -> Result<()> {
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("'{sha}' is not a full 40-character Git SHA");
    }
    Ok(())
}

pub(crate) fn fixture_path() -> Result<PathBuf> {
    let path = std::env::var_os(FIXTURE_PATH_ENV)
        .map(PathBuf::from)
        .with_context(|| format!("{FIXTURE_PATH_ENV} must point to the launcher-created clone"))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize fixture clone {}", path.display()))?;
    if !canonical.join(".git").exists() {
        bail!(
            "fixture path {} is not a standalone Git clone",
            canonical.display()
        );
    }
    Ok(canonical)
}

pub(crate) async fn ensure_clean(path: &Path) -> Result<()> {
    let status = git(path, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !status.is_empty() {
        bail!("fixture clone is not clean: {status}");
    }
    Ok(())
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

pub(crate) async fn git_success(path: &Path, args: &[&str]) -> Result<bool> {
    Ok(Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?
        .success())
}

pub(crate) fn validate_contract_info(info: &Value) -> Result<()> {
    let functions = info
        .get("functions")
        .and_then(Value::as_array)
        .context("engine::functions::info response is missing functions[]")?;
    for required in [
        REQUEST_FUNCTION,
        READ_FUNCTION,
        LIST_FUNCTION,
        RECONCILIATION_FUNCTION,
    ] {
        let function = functions
            .iter()
            .find(|function| function.get("function_id").and_then(Value::as_str) == Some(required))
            .with_context(|| format!("functions::info omitted '{required}'"))?;
        if !function.get("request_schema").is_some_and(Value::is_object)
            || !function
                .get("response_schema")
                .is_some_and(Value::is_object)
        {
            bail!("function '{required}' does not expose exact request/response JSON Schemas");
        }
        let expected = required_contract(required);
        let request_hash = crate::artifact::sha256_value(&function["request_schema"])?;
        let response_hash = crate::artifact::sha256_value(&function["response_schema"])?;
        if expected.request_schema_sha256.as_deref() != Some(request_hash.as_str())
            || expected.response_schema_sha256.as_deref() != Some(response_hash.as_str())
        {
            bail!("function '{required}' contract differs from the security-scan E2E v1 contract");
        }
    }
    Ok(())
}

pub(crate) fn object_schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect::<Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub(crate) fn port(
    kind: PortValueKind,
    optional: bool,
    control_source: Option<ControlSource>,
) -> StepPortDescriptor {
    StepPortDescriptor {
        kind,
        optional,
        control_source,
    }
}
