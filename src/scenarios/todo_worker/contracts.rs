use super::workspace::{safe_suffix, workspace_root};
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionSchemaContract {
    pub request: Value,
    pub response: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoTaskContract {
    pub scenario_version: u32,
    pub contract_sha256: String,
    pub worker_name: String,
    pub workspace_root: String,
    pub function_ids: BTreeMap<String, String>,
    pub request_response_schemas: BTreeMap<String, FunctionSchemaContract>,
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoImplementationTask {
    pub id: String,
    pub objective: String,
    pub completion_signal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoValidationCheck {
    pub id: String,
    pub probe_id: String,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repetitions: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoValidationPlan {
    pub scenario_version: u32,
    pub task_contract_sha256: String,
    pub summary: String,
    pub implementation_tasks: Vec<TodoImplementationTask>,
    pub validation_checks: Vec<TodoValidationCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompiledValidationPlan {
    pub scenario_version: u32,
    pub raw_plan_sha256: String,
    pub compiled_plan_sha256: String,
    pub task_contract: TodoTaskContract,
    pub implementation_tasks: Vec<TodoImplementationTask>,
    pub compiled_checks: Vec<TodoValidationCheck>,
    pub diagnostics: Vec<PlanDiagnostic>,
    pub ready_for_build: bool,
}

impl CompiledValidationPlan {
    pub fn selected_probe(&self, probe_id: &str) -> Option<&TodoValidationCheck> {
        self.compiled_checks
            .iter()
            .find(|check| check.probe_id == probe_id)
    }

    pub fn planning_gate(&self, id: &str) -> bool {
        match id {
            "plan_present" => !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "plan_missing"),
            "plan_schema_valid" => !self.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code.as_str(),
                    "plan_missing" | "plan_schema_invalid"
                )
            }),
            "plan_compilable" => !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code != "coverage_incomplete"),
            "plan_coverage_complete" => !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "coverage_incomplete"),
            _ => false,
        }
    }

    pub fn validate_integrity(&self) -> Result<()> {
        if self.scenario_version != VERSION || !self.ready_for_build || !self.diagnostics.is_empty()
        {
            bail!("compiled Todo validation plan is not ready for construction");
        }
        self.task_contract.validate()?;
        let mut unsigned = self.clone();
        let expected = unsigned.compiled_plan_sha256.clone();
        unsigned.compiled_plan_sha256.clear();
        if crate::artifact::sha256_value(&unsigned)? != expected {
            bail!("compiled Todo validation plan hash does not match its content");
        }
        let check_ids = self
            .compiled_checks
            .iter()
            .map(|check| check.id.as_str())
            .collect::<BTreeSet<_>>();
        let probe_ids = self
            .compiled_checks
            .iter()
            .map(|check| check.probe_id.as_str())
            .collect::<BTreeSet<_>>();
        if check_ids.len() != self.compiled_checks.len()
            || probe_ids.len() != self.compiled_checks.len()
            || !REQUIRED_PROBES
                .iter()
                .all(|probe| probe_ids.contains(probe))
        {
            bail!("compiled Todo validation plan has duplicate ids or incomplete coverage");
        }
        Ok(())
    }
}

pub fn compile_validation_plan(
    raw_plan: Option<&[u8]>,
    contract: &TodoTaskContract,
) -> Result<CompiledValidationPlan> {
    contract.validate()?;
    let raw_plan_sha256 = crate::artifact::sha256_bytes(raw_plan.unwrap_or_default());
    let mut diagnostics = Vec::new();
    let parsed = match raw_plan {
        Some(bytes) if bytes.len() > 64 * 1024 => {
            diagnostics.push(PlanDiagnostic {
                code: "plan_schema_invalid".into(),
                message: "validation plan exceeds the 64 KiB compiler limit".into(),
            });
            None
        }
        Some(bytes) if !bytes.is_empty() => {
            match serde_json::from_slice::<TodoValidationPlan>(bytes) {
                Ok(plan) => Some(plan),
                Err(error) => {
                    diagnostics.push(PlanDiagnostic {
                        code: "plan_schema_invalid".into(),
                        message: bounded_text(&error.to_string(), 512),
                    });
                    None
                }
            }
        }
        _ => {
            diagnostics.push(PlanDiagnostic {
                code: "plan_missing".into(),
                message: format!("{RAW_PLAN_FILE} was not produced"),
            });
            None
        }
    };

    let mut implementation_tasks = Vec::new();
    let mut compiled_checks = Vec::new();
    if let Some(plan) = parsed {
        validate_plan_fields(
            &plan,
            contract,
            &mut diagnostics,
            &mut implementation_tasks,
            &mut compiled_checks,
        );
    }

    let covered = compiled_checks
        .iter()
        .map(|check| check.probe_id.as_str())
        .collect::<BTreeSet<_>>();
    let omitted = REQUIRED_PROBES
        .iter()
        .filter(|probe| !covered.contains(**probe))
        .copied()
        .collect::<Vec<_>>();
    if !omitted.is_empty() {
        diagnostics.push(PlanDiagnostic {
            code: "coverage_incomplete".into(),
            message: format!("mandatory probes omitted: {}", omitted.join(", ")),
        });
    }
    diagnostics.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    diagnostics.dedup();

    let ready_for_build = diagnostics.is_empty();
    let mut compiled = CompiledValidationPlan {
        scenario_version: VERSION,
        raw_plan_sha256,
        compiled_plan_sha256: String::new(),
        task_contract: contract.clone(),
        implementation_tasks,
        compiled_checks,
        diagnostics,
        ready_for_build,
    };
    compiled.compiled_plan_sha256 = crate::artifact::sha256_value(&compiled)?;
    Ok(compiled)
}

fn validate_plan_fields(
    plan: &TodoValidationPlan,
    contract: &TodoTaskContract,
    diagnostics: &mut Vec<PlanDiagnostic>,
    implementation_tasks: &mut Vec<TodoImplementationTask>,
    compiled_checks: &mut Vec<TodoValidationCheck>,
) {
    if plan.scenario_version != VERSION {
        plan_diagnostic(
            diagnostics,
            "scenario_version_mismatch",
            format!(
                "plan scenario_version={} but expected {VERSION}",
                plan.scenario_version
            ),
        );
    }
    if plan.task_contract_sha256 != contract.contract_sha256 {
        plan_diagnostic(
            diagnostics,
            "contract_hash_mismatch",
            "plan references a different Todo task contract",
        );
    }
    if plan.summary.trim().is_empty() || plan.summary.len() > 2_048 {
        plan_diagnostic(
            diagnostics,
            "summary_invalid",
            "summary must contain between 1 and 2048 bytes",
        );
    }
    if plan.implementation_tasks.is_empty() || plan.implementation_tasks.len() > 8 {
        plan_diagnostic(
            diagnostics,
            "implementation_task_limit",
            "implementation_tasks must contain between 1 and 8 entries",
        );
    }
    if plan.validation_checks.is_empty() || plan.validation_checks.len() > 12 {
        plan_diagnostic(
            diagnostics,
            "validation_check_limit",
            "validation_checks must contain between 1 and 12 entries",
        );
    }

    let mut task_ids = BTreeSet::new();
    for task in plan.implementation_tasks.iter().take(8) {
        if !valid_plan_id(&task.id)
            || !task_ids.insert(task.id.clone())
            || task.objective.trim().is_empty()
            || task.completion_signal.trim().is_empty()
            || task.objective.len() > 1_024
            || task.completion_signal.len() > 1_024
        {
            plan_diagnostic(
                diagnostics,
                "implementation_task_invalid",
                format!("implementation task '{}' is invalid or duplicated", task.id),
            );
        } else {
            implementation_tasks.push(task.clone());
        }
    }

    let allowed = REQUIRED_PROBES
        .iter()
        .chain(OPTIONAL_PROBES.iter())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut check_ids = BTreeSet::new();
    let mut probe_ids = BTreeSet::new();
    for check in plan.validation_checks.iter().take(12) {
        let mut valid = true;
        if !valid_plan_id(&check.id) || !check_ids.insert(check.id.clone()) {
            plan_diagnostic(
                diagnostics,
                "validation_check_invalid",
                format!(
                    "validation check '{}' has an invalid or duplicate id",
                    check.id
                ),
            );
            valid = false;
        }
        if !allowed.contains(check.probe_id.as_str()) {
            plan_diagnostic(
                diagnostics,
                "probe_unknown",
                format!(
                    "probe '{}' is not in the closed Todo catalog",
                    check.probe_id
                ),
            );
            valid = false;
        } else if !probe_ids.insert(check.probe_id.clone()) {
            plan_diagnostic(
                diagnostics,
                "probe_duplicate",
                format!("probe '{}' is selected more than once", check.probe_id),
            );
            valid = false;
        }
        if check.rationale.trim().is_empty() || check.rationale.len() > 1_024 {
            plan_diagnostic(
                diagnostics,
                "validation_check_invalid",
                format!("validation check '{}' has an invalid rationale", check.id),
            );
            valid = false;
        }
        match check.probe_id.as_str() {
            "todo_repeatability" => {
                if !matches!(check.repetitions, Some(1..=3)) || check.concurrency.is_some() {
                    plan_diagnostic(
                        diagnostics,
                        "probe_parameters_invalid",
                        "todo_repeatability requires repetitions between 1 and 3 and no concurrency",
                    );
                    valid = false;
                }
            }
            "todo_concurrent_create" => {
                if !matches!(check.concurrency, Some(1..=5)) || check.repetitions.is_some() {
                    plan_diagnostic(
                        diagnostics,
                        "probe_parameters_invalid",
                        "todo_concurrent_create requires concurrency between 1 and 5 and no repetitions",
                    );
                    valid = false;
                }
            }
            _ if check.repetitions.is_some() || check.concurrency.is_some() => {
                plan_diagnostic(
                    diagnostics,
                    "probe_parameters_invalid",
                    format!("probe '{}' accepts no parameters", check.probe_id),
                );
                valid = false;
            }
            _ => {}
        }
        if valid {
            compiled_checks.push(check.clone());
        }
    }
}

fn valid_plan_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn plan_diagnostic(
    diagnostics: &mut Vec<PlanDiagnostic>,
    code: impl Into<String>,
    message: impl Into<String>,
) {
    diagnostics.push(PlanDiagnostic {
        code: code.into(),
        message: message.into(),
    });
}

pub(super) fn bounded_text(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

impl TodoTaskContract {
    pub fn validate(&self) -> Result<()> {
        if self.scenario_version != VERSION
            || self.worker_name.trim().is_empty()
            || !Path::new(&self.workspace_root).is_absolute()
        {
            bail!("Todo task contract has an invalid worker name or workspace root");
        }
        let expected = contract_for_identity(&self.worker_name, Path::new(&self.workspace_root))?;
        if expected.contract_sha256 != self.contract_sha256
            || expected.function_ids != self.function_ids
            || expected.request_response_schemas != self.request_response_schemas
            || expected.required_capabilities != self.required_capabilities
        {
            bail!("Todo task contract differs from its canonical identity");
        }
        Ok(())
    }

    pub fn function_id(&self, operation: &str) -> Result<&str> {
        self.function_ids
            .get(operation)
            .map(String::as_str)
            .with_context(|| format!("Todo contract omits function '{operation}'"))
    }
}

pub fn task_contract(run_id: &str) -> Result<TodoTaskContract> {
    let suffix = safe_suffix(run_id);
    let worker_name = format!("todo-e2e-{suffix}");
    contract_for_identity(&worker_name, &workspace_root(&worker_name))
}

pub(super) fn contract_for_identity(
    worker_name: &str,
    workspace: &Path,
) -> Result<TodoTaskContract> {
    let prefix = format!("{worker_name}::");
    let todo_schema = todo_schema();
    let function_ids = ["create", "list", "update", "delete"]
        .into_iter()
        .map(|operation| (operation.to_string(), format!("{prefix}{operation}")))
        .collect::<BTreeMap<_, _>>();
    let request_response_schemas = BTreeMap::from([
        (
            "create".into(),
            FunctionSchemaContract {
                request: object_schema(
                    json!({"title": {"type": "string", "minLength": 1}}),
                    &["title"],
                ),
                response: object_schema(json!({"todo": todo_schema.clone()}), &["todo"]),
            },
        ),
        (
            "list".into(),
            FunctionSchemaContract {
                request: object_schema(json!({}), &[]),
                response: object_schema(
                    json!({"todos": {"type": "array", "items": todo_schema.clone()}}),
                    &["todos"],
                ),
            },
        ),
        (
            "update".into(),
            FunctionSchemaContract {
                request: json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": {"type": "string", "minLength": 1},
                        "title": {"type": "string", "minLength": 1},
                        "completed": {"type": "boolean"}
                    },
                    "anyOf": [{"required": ["title"]}, {"required": ["completed"]}],
                    "additionalProperties": false
                }),
                response: object_schema(json!({"todo": todo_schema.clone()}), &["todo"]),
            },
        ),
        (
            "delete".into(),
            FunctionSchemaContract {
                request: object_schema(json!({"id": {"type": "string", "minLength": 1}}), &["id"]),
                response: object_schema(
                    json!({"deleted": {"const": true}, "id": {"type": "string", "minLength": 1}}),
                    &["deleted", "id"],
                ),
            },
        ),
    ]);
    let workspace_root = workspace
        .to_str()
        .context("Todo workspace root is not UTF-8")?
        .to_string();
    let required_capabilities = vec![
        "worker.manifest".into(),
        "worker.lifecycle".into(),
        "todo.create".into(),
        "todo.list".into(),
        "todo.update".into(),
        "todo.delete".into(),
        "todo.isolation".into(),
        "todo.invalid_inputs".into(),
    ];
    let unsigned = json!({
        "scenario_version": VERSION,
        "worker_name": worker_name,
        "workspace_root": workspace_root,
        "function_ids": function_ids,
        "request_response_schemas": request_response_schemas,
        "required_capabilities": required_capabilities,
    });
    Ok(TodoTaskContract {
        scenario_version: VERSION,
        contract_sha256: crate::artifact::sha256_value(&unsigned)?,
        worker_name: worker_name.into(),
        workspace_root,
        function_ids,
        request_response_schemas,
        required_capabilities,
    })
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "required": required,
        "properties": properties,
        "additionalProperties": false
    })
}

fn todo_schema() -> Value {
    object_schema(
        json!({
            "id": {"type": "string", "minLength": 1},
            "title": {"type": "string", "minLength": 1},
            "completed": {"type": "boolean"}
        }),
        &["id", "title", "completed"],
    )
}
