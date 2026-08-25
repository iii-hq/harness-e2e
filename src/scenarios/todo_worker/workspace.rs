use super::*;
pub async fn cleanup_contract(context: &E2eContext, contract: &TodoTaskContract) -> Result<()> {
    let mut failures = Vec::new();
    let remove_available = context
        .function_exists("worker::remove")
        .await
        .unwrap_or(false);
    let status_available = context
        .function_exists("worker::status")
        .await
        .unwrap_or(false);
    let info_available = context
        .function_exists("engine::functions::info")
        .await
        .unwrap_or(false);
    let mut withdrawn = false;
    if !remove_available || !status_available || !info_available {
        failures.push("Todo cleanup control plane is unavailable".into());
    } else {
        let initial_status = context
            .trigger_value("worker::status", json!({"name": contract.worker_name}))
            .await;
        let initial_functions_absent = todo_functions_absent(context, contract).await;
        match (initial_status, initial_functions_absent) {
            (Ok(status), Ok(true))
                if status.get("installed").and_then(Value::as_bool) == Some(false) =>
            {
                withdrawn = true;
            }
            (Ok(_), Ok(_)) => {
                if let Err(error) = remove_worker_with_retry(context, contract).await {
                    failures.push(format!(
                        "remove worker '{}': {error:#}",
                        contract.worker_name
                    ));
                } else {
                    for _ in 0..60 {
                        let status = context
                            .trigger_value("worker::status", json!({"name": contract.worker_name}))
                            .await;
                        let functions_absent = todo_functions_absent(context, contract).await;
                        if status.as_ref().is_ok_and(|value| {
                            value.get("installed").and_then(Value::as_bool) == Some(false)
                                && value.get("running").and_then(Value::as_bool) == Some(false)
                        }) && functions_absent.as_ref().is_ok_and(|absent| *absent)
                        {
                            withdrawn = true;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    if !withdrawn {
                        failures.push(format!(
                            "worker '{}' or its functions remained registered after removal",
                            contract.worker_name
                        ));
                    }
                }
            }
            (Err(error), _) => failures.push(format!(
                "query worker '{}' before cleanup: {error:#}",
                contract.worker_name
            )),
            (_, Err(error)) => {
                failures.push(format!("query Todo functions before cleanup: {error:#}"))
            }
        }
    }
    if withdrawn {
        if let Err(error) = remove_owned_workspace(contract) {
            failures.push(format!("remove Todo workspace: {error:#}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(failures.join("; "))
    }
}

async fn remove_worker_with_retry(context: &E2eContext, contract: &TodoTaskContract) -> Result<()> {
    const MAX_ATTEMPTS: usize = 240;
    for attempt in 1..=MAX_ATTEMPTS {
        match context
            .trigger_value(
                "worker::remove",
                json!({"names": [contract.worker_name], "all": false, "yes": true}),
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) if retryable_worker_lock(&error) && attempt < MAX_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the bounded worker removal loop always returns")
}

pub(super) fn retryable_worker_lock(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("W900")
        && (message.contains("project lock busy") || message.contains("worker operation is active"))
}

pub(super) async fn todo_functions_absent(
    context: &E2eContext,
    contract: &TodoTaskContract,
) -> Result<bool> {
    let response = context
        .trigger_value(
            "engine::functions::info",
            json!({
                "function_ids": contract.function_ids.values().collect::<Vec<_>>()
            }),
        )
        .await?;
    Ok(response
        .get("functions")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty))
}

pub fn prepare_owned_workspace(contract: &TodoTaskContract) -> Result<PathBuf> {
    contract.validate()?;
    let root = PathBuf::from(&contract.workspace_root);
    fs::create_dir_all(&root)
        .with_context(|| format!("create Todo workspace {}", root.display()))?;
    fs::write(
        root.join(OWNER_MARKER),
        format!("{}\n", contract.worker_name),
    )
    .with_context(|| format!("write Todo workspace marker in {}", root.display()))?;
    root.canonicalize()
        .with_context(|| format!("canonicalize Todo workspace {}", root.display()))
}

pub fn harness_workspace_base() -> Result<PathBuf> {
    let base = workspace_base();
    fs::create_dir_all(&base)
        .with_context(|| format!("create Todo scenario workspace base {}", base.display()))?;
    base.canonicalize().with_context(|| {
        format!(
            "canonicalize Todo scenario workspace base {}",
            base.display()
        )
    })
}

pub fn planned_validation_bundle_path(contract: &TodoTaskContract) -> PathBuf {
    Path::new(&contract.workspace_root)
        .join(".harness-e2e")
        .join("todo-validation-evidence.json")
}

pub fn persist_planned_validation_bundle(
    contract: &TodoTaskContract,
    bundle: &ValidationEvidenceBundle,
) -> Result<()> {
    persist_json(&planned_validation_bundle_path(contract), bundle)
}

pub(super) fn remove_owned_workspace(contract: &TodoTaskContract) -> Result<()> {
    let root = PathBuf::from(&contract.workspace_root);
    if !root.exists() {
        return Ok(());
    }
    let base = workspace_base();
    let canonical_root = root.canonicalize()?;
    let canonical_base = fs::canonicalize(&base).unwrap_or(base);
    if !canonical_root.starts_with(&canonical_base)
        || canonical_root.file_name().and_then(|value| value.to_str())
            != Some(contract.worker_name.as_str())
    {
        bail!("refusing to remove Todo workspace outside its run-scoped base");
    }
    let marker = fs::read_to_string(canonical_root.join(OWNER_MARKER))?;
    if marker.trim() != contract.worker_name {
        bail!("refusing to remove Todo workspace with mismatched ownership marker");
    }
    fs::remove_dir_all(&canonical_root)
        .with_context(|| format!("remove Todo workspace {}", canonical_root.display()))
}

pub(super) fn candidate_sha256(root: &Path) -> Result<Option<String>> {
    if !root.exists() {
        return Ok(None);
    }
    let mut files = Vec::new();
    collect_source_files(root, root, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    for path in files {
        let relative = path.strip_prefix(root)?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(&path)?);
        hasher.update([0]);
    }
    Ok(Some(format!("sha256:{:x}", hasher.finalize())))
}

pub(super) fn collect_source_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let first = relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str());
        if matches!(
            first,
            Some(".git" | ".harness-e2e" | "node_modules" | "target")
        ) {
            continue;
        }
        if path.is_dir() {
            collect_source_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn file_sha256(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(format!("sha256:{:x}", Sha256::digest(bytes)))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn persist_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("persist Todo validation evidence {}", path.display()))
}

pub(super) fn validation_bundle_path(contract: &TodoTaskContract) -> PathBuf {
    Path::new(&contract.workspace_root)
        .join(".harness-e2e")
        .join("validation-evidence.json")
}

pub(super) fn safe_suffix(value: &str) -> String {
    let mut suffix = value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(32)
        .collect::<String>();
    while suffix.len() < 8 {
        suffix.push('0');
    }
    suffix
}

pub(super) fn workspace_base() -> PathBuf {
    std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("scenario-workspaces")
}

pub(super) fn workspace_root(worker_name: &str) -> PathBuf {
    workspace_base().join(worker_name)
}
