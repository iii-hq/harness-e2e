use std::path::{Component, Path, PathBuf};

use anyhow::bail;
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::{
    common, ArtifactExpectation, CapturedDeliverable, CapturedInvariant, CleanupFuture,
    ComplexityProfile, DeliverableCaptureFuture, DeliverableContract, EvaluationFuture,
    ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence, ScenarioCase,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "shell_coder_sandbox";
const VERSION: u32 = 4;
const CODE_DELIVERABLE_ID: &str = "verified_code_file";
const EXECUTION_DELIVERABLE_ID: &str = "execution_evidence";
const DRAFT_NAME: &str = "draft_check.py";
const FINAL_NAME: &str = "checks/check.py";
const DRAFT_SCRIPT: &str = "values = [2, 3, 5, 7]\nprint(\"host-check:draft\")\n";
const FINAL_SCRIPT: &str = "values = [2, 3, 5, 7]\nprint(f\"host-check:{sum(values)}\")\n";
const HOST_STDOUT: &str = "host-check:17";
const SANDBOX_STDOUT: &str = "sandbox-check:35";
const EXPECTED_CORE_OPERATIONS: usize = 12;
const EXECUTION_QUALITY_WEIGHT: u8 = 45;
const WORKER_SETUP: AssessmentSpec = AssessmentSpec::hard_gated(
    "worker_setup",
    10,
    "Both registry workers are added and expose their required surfaces.",
);
const CODER_WORKFLOW: AssessmentSpec = AssessmentSpec::hard_gated(
    "coder_workflow",
    20,
    "Coder inspection, create, update, move, and read operations produce the exact file.",
);
const HOST_EXECUTION: AssessmentSpec = AssessmentSpec::hard_gated(
    "host_execution",
    10,
    "The final file runs successfully on the host with exact stdout.",
);
const SANDBOX_LIFECYCLE: AssessmentSpec = AssessmentSpec::hard_gated(
    "sandbox_lifecycle",
    10,
    "A named isolated sandbox is created, executed in, listed, and stopped.",
);
const COMPLETION_REPORT: AssessmentSpec = AssessmentSpec::score_only(
    "completion_report",
    5,
    "The final response includes both exact observed outputs.",
);
const EXECUTION_QUALITY: AssessmentSpec = AssessmentSpec::score_only(
    "execution_quality",
    EXECUTION_QUALITY_WEIGHT,
    "The workflow completes without function-call errors; recovered errors lower quality without overriding validated effects.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    WORKER_SETUP,
    CODER_WORKFLOW,
    HOST_EXECUTION,
    SANDBOX_LIFECYCLE,
    COMPLETION_REPORT,
    EXECUTION_QUALITY,
];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    scenario_for_case(run_id)
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = ScenarioCase::new(
        ID,
        VERSION,
        seed,
        json!({
            "draft_path": DRAFT_NAME,
            "final_path": FINAL_NAME,
            "draft_content": DRAFT_SCRIPT,
            "final_content": FINAL_SCRIPT,
            "host_stdout": HOST_STDOUT,
            "sandbox_stdout": SANDBOX_STDOUT,
            "sandbox_resources": {
                "cpus": 1,
                "memory_mb": 256,
                "network": false,
            },
        }),
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 1,
            external_systems: 2,
            state_transitions: 8,
            artifact_count: 2,
            ambiguity_level: 2,
            ..ComplexityProfile::default()
        },
        vec![
            "e2e::control-plane-v1".to_string(),
            "iii::functions".to_string(),
            "iii::registry".to_string(),
            "iii::coder".to_string(),
            "iii::shell".to_string(),
            "iii::sandbox".to_string(),
        ],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec: scenario_for_case(namespace),
        case,
        capture: Some(capture),
    })
}

fn scenario_for_case(run_id: &str) -> ScenarioSpec {
    let sandbox_name = sandbox_name(run_id);
    ScenarioSpec {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Perform this verification entirely in the current workspace and in the stated order.\n\n\
             1. Add the `shell` worker from the public registry and wait for that add operation to \
             finish. Only after it finishes, add the `iii-sandbox` worker from the public registry \
             and wait for that operation too, even if either worker is already installed. Do not \
             launch the two add operations in parallel.\n\
             2. Invoke the dedicated code-file capability's own access-contract inspection \
             operation. Generic engine function discovery does not satisfy this step.\n\
             3. With dedicated code-file operations, create `{DRAFT_NAME}` with exactly:\n\n\
             ```python\n{DRAFT_SCRIPT}```\n\n\
             4. With the dedicated code-file update operation, change only the second line so the \
             file content becomes exactly:\n\n\
             ```python\n{FINAL_SCRIPT}```\n\n\
             5. With the dedicated code-file move operation, move it to `{FINAL_NAME}`.\n\
             6. Read `{FINAL_NAME}` back with the dedicated code-file read operation and verify \
             the exact final content. Do not create, edit, move, or read this code file with a \
             general shell filesystem operation or by running a command; these steps specifically \
             validate the code-file capabilities.\n\
             7. Run `{FINAL_NAME}` with Python 3 on the host workspace and verify stdout is exactly \
             `{HOST_STDOUT}`.\n\
             8. Create a short-lived isolated Python sandbox named `{sandbox_name}` with exactly \
             1 CPU, 256 MB of memory, and networking disabled. Use the sandbox's own command \
             execution capability to run Python inside it and produce stdout exactly \
             `{SANDBOX_STDOUT}`. Do not use the shell's sandbox target for this step.\n\
             9. List the sandboxes and confirm that named sandbox is present and active, then stop \
             it and wait for shutdown.\n\n\
             Only report completion after every operation succeeds. Include both exact output \
             lines in your final response."
        ),
        filesystem_root: Some(workspace_root(run_id)),
        execution: ExecutionPolicy {
            max_turns: 40,
            max_output_tokens: Some(8_192),
            max_total_tokens: 1_638_400,
            stuck_timeout_seconds: 600,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
        judge_reference: None,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
}

fn capture<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let root = workspace_root(run_id);
        let final_path = root.join(FINAL_NAME);
        let final_content = std::fs::read_to_string(&final_path).ok();
        let calls = common::function_calls(&observation.transcript);
        let coder_workflow_observed = calls.iter().any(|call| call.function_id == "coder::info")
            && calls.iter().any(|call| is_exact_create(call, &root))
            && calls.iter().any(|call| is_exact_update(call, &root))
            && calls.iter().any(|call| is_exact_move(call, &root))
            && calls.iter().any(|call| is_exact_read(call, &root))
            && calls_are_ordered(
                &calls,
                &[
                    "coder::info",
                    "coder::create-file",
                    "coder::update-file",
                    "coder::move",
                    "coder::read-file",
                ],
            );
        let host_stdout =
            observed_successful_output(&observation.transcript, "shell::exec", HOST_STDOUT);
        let sandbox_stdout =
            observed_successful_output(&observation.transcript, "sandbox::exec", SANDBOX_STDOUT);
        let sandbox = sandbox_evidence(&observation.transcript, &calls, &sandbox_name(run_id));

        Ok(vec![
            CapturedDeliverable {
                id: CODE_DELIVERABLE_ID.to_string(),
                kind: "code_file".to_string(),
                content: json!({
                    "path": FINAL_NAME,
                    "content": final_content,
                })
                .into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "exact_final_content".to_string(),
                        passed: final_content.as_deref() == Some(FINAL_SCRIPT),
                        reason: format!("captured {}", final_path.display()),
                    },
                    CapturedInvariant {
                        id: "coder_workflow_observed".to_string(),
                        passed: coder_workflow_observed,
                        reason: "dedicated coder operations were observed in the required order"
                            .to_string(),
                    },
                ],
                provenance: vec![ProvenanceEvidence {
                    kind: "filesystem_path".to_string(),
                    source_id: final_path.display().to_string(),
                    relation: "captured_before_workspace_cleanup".to_string(),
                }],
            },
            CapturedDeliverable {
                id: EXECUTION_DELIVERABLE_ID.to_string(),
                kind: "execution_result".to_string(),
                content: json!({
                    "host_stdout": host_stdout,
                    "sandbox_stdout": sandbox_stdout,
                    "sandbox": {
                        "sandbox_id": sandbox.sandbox_id,
                        "create_request": sandbox.create_request,
                        "created": sandbox.created,
                        "executed": sandbox.executed,
                        "listed": sandbox.listed,
                        "stopped": sandbox.stopped,
                        "ordered": sandbox.ordered,
                    },
                })
                .into(),
                invariants: vec![
                    CapturedInvariant {
                        id: "host_output_exact".to_string(),
                        passed: host_stdout.as_deref() == Some(HOST_STDOUT),
                        reason: format!("expected exact host stdout `{HOST_STDOUT}`"),
                    },
                    CapturedInvariant {
                        id: "sandbox_output_exact".to_string(),
                        passed: sandbox_stdout.as_deref() == Some(SANDBOX_STDOUT),
                        reason: format!("expected exact sandbox stdout `{SANDBOX_STDOUT}`"),
                    },
                    CapturedInvariant {
                        id: "sandbox_lifecycle_complete".to_string(),
                        passed: sandbox.complete(),
                        reason: sandbox.summary(),
                    },
                ],
                provenance: vec![ProvenanceEvidence {
                    kind: "sandbox".to_string(),
                    source_id: sandbox_name(run_id),
                    relation: "executed_and_stopped".to_string(),
                }],
            },
        ])
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![
            ArtifactExpectation {
                id: CODE_DELIVERABLE_ID.to_string(),
                kind: "code_file".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": { "const": FINAL_NAME },
                        "content": { "type": ["string", "null"] }
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 16_384,
            },
            ArtifactExpectation {
                id: EXECUTION_DELIVERABLE_ID.to_string(),
                kind: "execution_result".to_string(),
                media_type: "application/json".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["host_stdout", "sandbox_stdout", "sandbox"],
                    "properties": {
                        "host_stdout": { "type": ["string", "null"] },
                        "sandbox_stdout": { "type": ["string", "null"] },
                        "sandbox": { "type": "object" }
                    },
                    "additionalProperties": false
                }),
                max_size_bytes: 16_384,
            },
        ],
        invariants: vec![
            InvariantSpec {
                id: "exact_final_content".to_string(),
                description: "The final code file has the exact requested content.".to_string(),
            },
            InvariantSpec {
                id: "coder_workflow_observed".to_string(),
                description: "Dedicated coder operations created, updated, moved, and read it."
                    .to_string(),
            },
            InvariantSpec {
                id: "host_output_exact".to_string(),
                description: "Host execution produced the exact expected stdout.".to_string(),
            },
            InvariantSpec {
                id: "sandbox_output_exact".to_string(),
                description: "Sandbox execution produced the exact expected stdout.".to_string(),
            },
            InvariantSpec {
                id: "sandbox_lifecycle_complete".to_string(),
                description: "The isolated sandbox completed its full ordered lifecycle."
                    .to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let invocations = common::function_invocations(&observation.transcript);
        let calls = invocations
            .iter()
            .map(|invocation| invocation.call.clone())
            .collect::<Vec<_>>();
        let installed_shell = calls.iter().any(|call| is_registry_install(call, "shell"));
        let installed_sandbox = calls
            .iter()
            .any(|call| is_registry_install(call, "iii-sandbox"));

        let shell_surface_ready = context.function_exists("shell::exec").await?;
        let coder_surface_ready = context.function_exists("coder::create-file").await?;
        let sandbox_surface_ready = context.function_exists("sandbox::create").await?;
        let surfaces_ready = shell_surface_ready && coder_surface_ready && sandbox_surface_ready;
        let worker_setup = installed_shell && installed_sandbox && surfaces_ready;

        let root = workspace_root(run_id);
        let coder_info = calls.iter().any(|call| call.function_id == "coder::info");
        let coder_create = calls.iter().any(|call| is_exact_create(call, &root));
        let coder_update = calls.iter().any(|call| is_exact_update(call, &root));
        let coder_move = calls.iter().any(|call| is_exact_move(call, &root));
        let coder_read = calls.iter().any(|call| is_exact_read(call, &root));
        let coder_ordered = calls_are_ordered(
            &calls,
            &[
                "coder::info",
                "coder::create-file",
                "coder::update-file",
                "coder::move",
                "coder::read-file",
            ],
        );
        let coder_results_succeeded = correlated_call_succeeded(
            &observation.transcript,
            &invocations,
            |call| call.function_id == "coder::info",
            |_| true,
        ) && correlated_call_succeeded(
            &observation.transcript,
            &invocations,
            |call| is_exact_create(call, &root),
            batch_result_succeeded,
        ) && correlated_call_succeeded(
            &observation.transcript,
            &invocations,
            |call| is_exact_update(call, &root),
            batch_result_succeeded,
        ) && correlated_call_succeeded(
            &observation.transcript,
            &invocations,
            |call| is_exact_move(call, &root),
            batch_result_succeeded,
        ) && correlated_call_succeeded(
            &observation.transcript,
            &invocations,
            |call| is_exact_read(call, &root),
            successful_coder_read,
        );

        let draft_path = root.join(DRAFT_NAME);
        let final_path = root.join(FINAL_NAME);
        let final_read = std::fs::read_to_string(&final_path);
        let final_matches = final_read
            .as_deref()
            .is_ok_and(|content| content == FINAL_SCRIPT);
        let draft_removed = !draft_path.exists();
        let coder_workflow = coder_info
            && coder_create
            && coder_update
            && coder_move
            && coder_read
            && coder_ordered
            && coder_results_succeeded
            && final_matches
            && draft_removed;

        let host_exec = calls.iter().any(|call| is_exact_host_exec(call, &root));
        let host_output = correlated_call_succeeded(
            &observation.transcript,
            &invocations,
            |call| is_exact_host_exec(call, &root),
            |result| successful_output_result(result, HOST_STDOUT),
        );
        let host_execution = host_exec && host_output;

        let expected_sandbox_name = sandbox_name(run_id);
        let sandbox = sandbox_evidence(&observation.transcript, &calls, &expected_sandbox_name);
        let sandbox_lifecycle = sandbox.complete();

        let core_operations = [
            installed_shell,
            installed_sandbox,
            coder_info,
            coder_create,
            coder_update,
            coder_move,
            coder_read,
            host_exec,
            sandbox.created,
            sandbox.executed,
            sandbox.listed,
            sandbox.stopped,
        ]
        .into_iter()
        .filter(|observed| *observed)
        .count();
        let operation_volume = core_operations >= EXPECTED_CORE_OPERATIONS;
        let function_call_errors = observation.metrics.totals.function_call_errors;
        let response_reports_outputs = observation.response.contains(HOST_STDOUT)
            && observation.response.contains(SANDBOX_STDOUT);

        let mut evaluation = assessment::build_evaluation([
            WORKER_SETUP.full_or_zero(
                worker_setup,
                format!(
                    "shell add={installed_shell}, iii-sandbox add={installed_sandbox}, \
                     shell ready={shell_surface_ready}, coder ready={coder_surface_ready}, \
                     sandbox ready={sandbox_surface_ready}"
                ),
            ),
            CODER_WORKFLOW.full_or_zero(
                coder_workflow,
                match &final_read {
                    Ok(_) => format!(
                        "info={coder_info}, create={coder_create}, update={coder_update}, \
                         move={coder_move}, read={coder_read}, ordered={coder_ordered}, \
                         successful results={coder_results_succeeded}, exact final \
                         content={final_matches}, draft removed={draft_removed}"
                    ),
                    Err(error) => format!(
                        "coder operations info/create/update/move/read={coder_info}/{coder_create}/\
                         {coder_update}/{coder_move}/{coder_read}; could not read {}: {error}",
                        final_path.display()
                    ),
                },
            ),
            HOST_EXECUTION.full_or_zero(
                host_execution,
                format!(
                    "exact host execution call={host_exec}, exact successful stdout={host_output}"
                ),
            ),
            SANDBOX_LIFECYCLE.full_or_zero(sandbox_lifecycle, sandbox.summary()),
            COMPLETION_REPORT.full_or_zero(
                response_reports_outputs,
                "awarded when the final response includes both exact outputs",
            ),
            EXECUTION_QUALITY.award(
                execution_quality_award(function_call_errors),
                format!(
                    "observed {function_call_errors} function-call error(s); recovered errors reduce quality but validated outcomes remain authoritative"
                ),
            )?,
        ]);
        evaluation.hard_gates.push(common::gate(
            "operation_volume_reached",
            operation_volume,
            format!(
                "observed {core_operations} of {EXPECTED_CORE_OPERATIONS} required core operations"
            ),
        ));

        Ok(evaluation)
    })
}

fn execution_quality_award(function_call_errors: u64) -> u8 {
    if function_call_errors == 0 {
        EXECUTION_QUALITY_WEIGHT
    } else {
        0
    }
}

fn is_registry_install(call: &common::ObservedFunctionCall, worker: &str) -> bool {
    call.function_id == "worker::add"
        && call
            .arguments
            .pointer("/source/kind")
            .and_then(Value::as_str)
            == Some("registry")
        && call
            .arguments
            .pointer("/source/name")
            .and_then(Value::as_str)
            == Some(worker)
}

fn is_exact_create(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    call.function_id == "coder::create-file"
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/path")
                .and_then(Value::as_str),
            root,
            DRAFT_NAME,
        )
        && call
            .arguments
            .pointer("/files/0/content")
            .and_then(Value::as_str)
            == Some(DRAFT_SCRIPT)
        && call
            .arguments
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.len() == 1)
}

fn is_exact_update(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    call.function_id == "coder::update-file"
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/path")
                .and_then(Value::as_str),
            root,
            DRAFT_NAME,
        )
        && call
            .arguments
            .pointer("/files/0/ops")
            .and_then(Value::as_array)
            .is_some_and(|ops| !ops.is_empty())
        && call
            .arguments
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.len() == 1)
}

fn is_exact_move(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    call.function_id == "coder::move"
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/from")
                .and_then(Value::as_str),
            root,
            DRAFT_NAME,
        )
        && workspace_path_matches(
            call.arguments
                .pointer("/files/0/to")
                .and_then(Value::as_str),
            root,
            FINAL_NAME,
        )
        && call
            .arguments
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| files.len() == 1)
}

fn is_exact_read(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    call.function_id == "coder::read-file"
        && workspace_path_matches(
            call.arguments.get("path").and_then(Value::as_str),
            root,
            FINAL_NAME,
        )
        && call.arguments.get("paths").is_none()
}

fn is_exact_host_exec(call: &common::ObservedFunctionCall, root: &Path) -> bool {
    let host_target = call.arguments.get("target").is_none()
        || call
            .arguments
            .pointer("/target/kind")
            .and_then(Value::as_str)
            == Some("host");
    let python = call
        .arguments
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| matches!(command, "python" | "python3"));
    let final_arg = call
        .arguments
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.iter()
                .any(|arg| workspace_path_matches(arg.as_str(), root, FINAL_NAME))
        });
    call.function_id == "shell::exec" && host_target && python && final_arg
}

fn workspace_path_matches(value: Option<&str>, root: &Path, expected: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    let path = Path::new(value);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    let Some(resolved) = normalize_workspace_path(&resolved) else {
        return false;
    };
    let Some(expected) = normalize_workspace_path(&root.join(expected)) else {
        return false;
    };
    resolved == expected
}

fn normalize_workspace_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Normal(part) => normalized.push(part),
        }
    }
    Some(normalized)
}

fn calls_are_ordered(calls: &[common::ObservedFunctionCall], required: &[&str]) -> bool {
    let mut next = 0;
    for required_id in required {
        let Some(offset) = calls[next..]
            .iter()
            .position(|call| call.function_id == *required_id)
        else {
            return false;
        };
        next += offset + 1;
    }
    true
}

fn correlated_call_succeeded(
    transcript: &Value,
    invocations: &[common::ObservedFunctionInvocation],
    call_matches: impl Fn(&common::ObservedFunctionCall) -> bool,
    result_matches: impl Fn(&Value) -> bool,
) -> bool {
    invocations
        .iter()
        .filter(|invocation| call_matches(&invocation.call))
        .any(|invocation| {
            common::function_result(transcript, invocation).is_some_and(&result_matches)
        })
}

fn function_results<'a>(transcript: &'a Value, function_id: &'a str) -> Vec<&'a Value> {
    transcript
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| {
            message.get("role").and_then(Value::as_str) == Some("function_result")
                && message.get("function_id").and_then(Value::as_str) == Some(function_id)
                && message.get("is_error").and_then(Value::as_bool) == Some(false)
        })
        .collect()
}

fn batch_result_succeeded(message: &Value) -> bool {
    message
        .pointer("/details/results")
        .and_then(Value::as_array)
        .is_some_and(|results| {
            !results.is_empty()
                && results
                    .iter()
                    .all(|result| result.get("success").and_then(Value::as_bool) == Some(true))
        })
}

fn successful_coder_read(message: &Value) -> bool {
    message.pointer("/details/content").and_then(Value::as_str) == Some(FINAL_SCRIPT)
}

fn successful_output(transcript: &Value, function_id: &str, expected: &str) -> bool {
    function_results(transcript, function_id)
        .into_iter()
        .any(|message| successful_output_result(message, expected))
}

fn observed_successful_output(
    transcript: &Value,
    function_id: &str,
    expected: &str,
) -> Option<String> {
    function_results(transcript, function_id)
        .into_iter()
        .find(|message| successful_output_result(message, expected))?
        .pointer("/details/stdout")
        .and_then(Value::as_str)
        .map(|stdout| stdout.trim().to_string())
}

fn successful_output_result(message: &Value, expected: &str) -> bool {
    message
        .pointer("/details/exit_code")
        .and_then(Value::as_i64)
        == Some(0)
        && message
            .pointer("/details/stdout")
            .and_then(Value::as_str)
            .is_some_and(|stdout| stdout.trim() == expected)
        && message
            .pointer("/details/stderr")
            .and_then(Value::as_str)
            .is_some_and(|stderr| stderr.trim().is_empty())
        && message
            .pointer("/details/timed_out")
            .and_then(Value::as_bool)
            != Some(true)
}

#[derive(Debug, Default)]
struct SandboxEvidence {
    sandbox_id: Option<String>,
    create_request: bool,
    created: bool,
    executed: bool,
    listed: bool,
    stopped: bool,
    ordered: bool,
}

impl SandboxEvidence {
    fn complete(&self) -> bool {
        self.create_request
            && self.created
            && self.executed
            && self.listed
            && self.stopped
            && self.ordered
    }

    fn summary(&self) -> String {
        format!(
            "sandbox id={}, exact create request={}, created={}, executed with exact stdout={}, \
             listed active={}, stopped={}, lifecycle ordered={}",
            self.sandbox_id.as_deref().unwrap_or("missing"),
            self.create_request,
            self.created,
            self.executed,
            self.listed,
            self.stopped,
            self.ordered
        )
    }
}

fn sandbox_evidence(
    transcript: &Value,
    calls: &[common::ObservedFunctionCall],
    expected_name: &str,
) -> SandboxEvidence {
    let sandbox_id = function_results(transcript, "sandbox::create")
        .into_iter()
        .find_map(|message| {
            (message.pointer("/details/image").and_then(Value::as_str) == Some("python"))
                .then(|| {
                    message
                        .pointer("/details/sandbox_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .flatten()
        });
    let create_request = calls.iter().any(|call| {
        call.function_id == "sandbox::create"
            && call.arguments.get("image").and_then(Value::as_str) == Some("python")
            && call.arguments.get("name").and_then(Value::as_str) == Some(expected_name)
            && call.arguments.get("cpus").and_then(Value::as_u64) == Some(1)
            && call.arguments.get("memory_mb").and_then(Value::as_u64) == Some(256)
            && call.arguments.get("network").and_then(Value::as_bool) == Some(false)
    });

    let Some(id) = sandbox_id.as_deref() else {
        return SandboxEvidence {
            sandbox_id,
            create_request,
            ..SandboxEvidence::default()
        };
    };
    let exec_call = calls.iter().any(|call| {
        call.function_id == "sandbox::exec"
            && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(id)
    });
    let executed = exec_call && successful_output(transcript, "sandbox::exec", SANDBOX_STDOUT);
    let list_call = calls.iter().any(|call| call.function_id == "sandbox::list");
    let listed = list_call
        && function_results(transcript, "sandbox::list")
            .into_iter()
            .any(|message| {
                message
                    .pointer("/details/sandboxes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|sandbox| {
                        sandbox.get("sandbox_id").and_then(Value::as_str) == Some(id)
                            && sandbox.get("name").and_then(Value::as_str) == Some(expected_name)
                            && sandbox.get("stopped").and_then(Value::as_bool) == Some(false)
                    })
            });
    let stop_call = calls.iter().any(|call| {
        call.function_id == "sandbox::stop"
            && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(id)
            && call.arguments.get("wait").and_then(Value::as_bool) == Some(true)
    });
    let stopped = stop_call
        && function_results(transcript, "sandbox::stop")
            .into_iter()
            .any(|message| {
                message
                    .pointer("/details/sandbox_id")
                    .and_then(Value::as_str)
                    == Some(id)
                    && message.pointer("/details/stopped").and_then(Value::as_bool) == Some(true)
            });
    let ordered = sandbox_calls_are_ordered(calls, expected_name, id);

    SandboxEvidence {
        sandbox_id,
        create_request,
        created: true,
        executed,
        listed,
        stopped,
        ordered,
    }
}

fn sandbox_calls_are_ordered(
    calls: &[common::ObservedFunctionCall],
    expected_name: &str,
    sandbox_id: &str,
) -> bool {
    let create = calls.iter().position(|call| {
        call.function_id == "sandbox::create"
            && call.arguments.get("name").and_then(Value::as_str) == Some(expected_name)
    });
    let exec = calls.iter().position(|call| {
        call.function_id == "sandbox::exec"
            && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(sandbox_id)
    });
    let list = calls
        .iter()
        .position(|call| call.function_id == "sandbox::list");
    let stop = calls.iter().position(|call| {
        call.function_id == "sandbox::stop"
            && call.arguments.get("sandbox_id").and_then(Value::as_str) == Some(sandbox_id)
    });
    matches!(
        (create, exec, list, stop),
        (Some(create), Some(exec), Some(list), Some(stop))
            if create < exec && exec < list && list < stop
    )
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let mut failures = Vec::new();
        if let Err(error) = cleanup_owned_sandbox(context, &sandbox_name(run_id)).await {
            failures.push(format!("sandbox cleanup: {error}"));
        }
        if let Err(error) = remove_workspace(&workspace_root(run_id)) {
            failures.push(format!("workspace cleanup: {error}"));
        }
        if !failures.is_empty() {
            bail!(failures.join("; "));
        }
        Ok(())
    })
}

async fn cleanup_owned_sandbox(context: &E2eContext, expected_name: &str) -> anyhow::Result<()> {
    if !context.function_exists("sandbox::list").await? {
        return Ok(());
    }
    let listed = context.trigger_value("sandbox::list", json!({})).await?;
    for sandbox_id in owned_running_sandbox_ids(&listed, expected_name) {
        let _: Value = context
            .trigger(
                "sandbox::stop",
                json!({ "sandbox_id": sandbox_id, "wait": true }),
            )
            .await?;
    }
    Ok(())
}

fn owned_running_sandbox_ids(listed: &Value, expected_name: &str) -> Vec<String> {
    listed
        .get("sandboxes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|sandbox| sandbox.get("name").and_then(Value::as_str) == Some(expected_name))
        .filter(|sandbox| sandbox.get("stopped").and_then(Value::as_bool) != Some(true))
        .filter_map(|sandbox| sandbox.get("sandbox_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn remove_workspace(root: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sandbox_name(run_id: &str) -> String {
    format!("e2e-shell-coder-{run_id}")
}

fn workspace_root(run_id: &str) -> PathBuf {
    super::kit::workspace_root(ID, run_id)
}
