//! Build visual SWE Workers whose domain behavior is projected through Canvas.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedDeliverableContent,
    CapturedInvariant, DeliverableContract, ExecutionPolicy, InvariantSpec, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const FORM_FLOW_ID: &str = "form_flow_build";
pub const STATE_MACHINE_ID: &str = "state_machine_canvas_build";
pub const FORM_FLOW_SUMMARY: &str = "Build a visual SWE issue-form Worker with deterministic bug and feature fields, live editing and preview, and a flowchart projection stored through Canvas.";
pub const STATE_MACHINE_SUMMARY: &str = "Build a visual CI state-machine Worker with deterministic simulation, live transition editing, and a stateDiagram-v2 projection stored through Canvas.";

const EVIDENCE_LIMIT: u64 = 24 * 1024 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 4 * 1024 * 1024;

const RUNTIME: AssessmentSpec = AssessmentSpec::scored_in(
    "runtime_contract",
    10,
    "The run-scoped Worker is ready and exposes its described domain, Canvas, and UI functions.",
    EvaluationDimension::Deliverable,
);
const DOMAIN_PRIMARY: AssessmentSpec = AssessmentSpec::scored(
    "domain_primary",
    20,
    "The primary deterministic domain path matches the independent Harness oracle.",
);
const DOMAIN_BRANCH: AssessmentSpec = AssessmentSpec::scored(
    "domain_branch",
    15,
    "The alternate deterministic domain path matches the independent Harness oracle.",
);
const INVALID_INPUTS: AssessmentSpec = AssessmentSpec::scored(
    "invalid_inputs",
    10,
    "Invalid domain input is rejected and the Worker remains healthy.",
);
const CANVAS_INITIAL: AssessmentSpec = AssessmentSpec::scored(
    "canvas_initial",
    10,
    "The Worker creates and reads the exact initial Mermaid projection through Canvas.",
);
const CANVAS_UPDATE: AssessmentSpec = AssessmentSpec::scored(
    "canvas_update",
    10,
    "The live editor updates the same Canvas id to the expected Mermaid projection.",
);
const CONSOLE: AssessmentSpec = AssessmentSpec::scored_in(
    "console_delivery",
    10,
    "The Console reports fresh, warning-free script and style assets for the Worker.",
    EvaluationDimension::Deliverable,
);
const INTERACTION: AssessmentSpec = AssessmentSpec::scored_in(
    "browser_interaction",
    10,
    "The real Console page completes the required live-preview interaction.",
    EvaluationDimension::Deliverable,
);
const EVIDENCE: AssessmentSpec = AssessmentSpec::scored_in(
    "evidence_complete",
    5,
    "Portable screenshots show the Worker and rendered Canvas graph in the full Console workspace.",
    EvaluationDimension::StructuralIntegrity,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    RUNTIME,
    DOMAIN_PRIMARY,
    DOMAIN_BRANCH,
    INVALID_INPUTS,
    CANVAS_INITIAL,
    CANVAS_UPDATE,
    CONSOLE,
    INTERACTION,
    EVIDENCE,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Form,
    Machine,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Self::Form => FORM_FLOW_ID,
            Self::Machine => STATE_MACHINE_ID,
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Form => FORM_FLOW_SUMMARY,
            Self::Machine => STATE_MACHINE_SUMMARY,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Form => "form_flow",
            Self::Machine => "state_machine",
        }
    }

    fn page_id(self) -> &'static str {
        match self {
            Self::Form => "form-flow",
            Self::Machine => "state-machine",
        }
    }

    fn domain_operation(self) -> &'static str {
        match self {
            Self::Form => "preview",
            Self::Machine => "transition",
        }
    }
}

pub struct FormFlowBuild;
pub struct StateMachineCanvasBuild;

macro_rules! scenario_impl {
    ($type:ty, $kind:expr) => {
        #[async_trait]
        impl Scenario for $type {
            fn id(&self) -> &'static str {
                $kind.id()
            }

            fn summary(&self) -> Option<&'static str> {
                Some($kind.summary())
            }

            fn case(&self, seed: u64) -> Result<ScenarioCase> {
                scenario_case($kind, seed)
            }

            fn spec(&self, run_id: &str) -> ScenarioSpec {
                scenario_spec($kind, run_id)
            }

            async fn setup(&self, _context: &E2eContext, run_id: &str) -> Result<()> {
                prepare_workspace($kind, run_id).await
            }

            async fn capture(
                &self,
                context: &E2eContext,
                _observation: &ScenarioObservation,
                run_id: &str,
            ) -> Result<Vec<CapturedDeliverable>> {
                let evidence = validate_candidate(context, $kind, run_id).await?;
                let invariants = [
                    "runtime_contract",
                    "canvas_initial",
                    "canvas_update",
                    "console_delivery",
                    "browser_interaction",
                    "evidence_complete",
                ]
                .into_iter()
                .map(|id| CapturedInvariant {
                    id: id.into(),
                    passed: passed(&evidence, id),
                    reason: reason(&evidence, id),
                })
                .collect();
                Ok(vec![CapturedDeliverable {
                    id: evidence_id($kind).into(),
                    kind: "visual_worker_audit".into(),
                    content: CapturedDeliverableContent::Json(evidence),
                    invariants,
                    provenance: vec![ProvenanceEvidence {
                        kind: "filesystem_path".into(),
                        source_id: workspace_root($kind, run_id).display().to_string(),
                        relation: "validated_before_cleanup".into(),
                    }],
                }])
            }

            async fn evaluate(
                &self,
                _context: &E2eContext,
                observation: &ScenarioObservation,
                _run_id: &str,
            ) -> Result<ObjectiveEvaluation> {
                let evidence = observation
                    .deliverables
                    .iter()
                    .find(|item| item.id == evidence_id($kind))
                    .and_then(|item| item.content.as_json())
                    .context("visual Worker evidence deliverable is missing")?;
                Ok(evaluate_evidence(evidence, observation.metrics.complete))
            }

            async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
                cleanup_workspace(context, $kind, run_id).await
            }
        }
    };
}

scenario_impl!(FormFlowBuild, Kind::Form);
scenario_impl!(StateMachineCanvasBuild, Kind::Machine);

#[derive(Clone)]
struct WorkerContract {
    worker: String,
    functions: BTreeMap<&'static str, String>,
}

impl WorkerContract {
    fn new(kind: Kind, run_id: &str) -> Self {
        let suffix: String = run_id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .take(14)
            .collect();
        let worker = format!(
            "{}_{}",
            kind.prefix(),
            if suffix.is_empty() { "run" } else { &suffix }
        );
        let functions = [kind.domain_operation(), "canvas", "ui-content"]
            .into_iter()
            .map(|name| (name, format!("{worker}::{name}")))
            .collect();
        Self { worker, functions }
    }

    fn script_path(&self) -> String {
        format!("{}/page.js", self.worker)
    }

    fn style_path(&self) -> String {
        format!("{}/styles.css", self.worker)
    }
}

fn evidence_id(kind: Kind) -> &'static str {
    match kind {
        Kind::Form => "form_flow_worker_evidence",
        Kind::Machine => "state_machine_worker_evidence",
    }
}

fn scenario_case(kind: Kind, seed: u64) -> Result<ScenarioCase> {
    ScenarioCase::new(
        kind.id(),
        seed,
        json!({
            "worker_kind": kind.prefix(),
            "domain_operation": kind.domain_operation(),
            "canvas_functions": ["canvas::validate", "canvas::create", "canvas::get", "canvas::update"],
            "diagram_family": if kind == Kind::Form { "flowchart" } else { "stateDiagram" },
        }),
        vec![
            Capability::E2eControlPlaneV1,
            Capability::IiiFunctions,
            Capability::IiiCoder,
            Capability::IiiShell,
            Capability::BrowserInteractive,
            Capability::IiiCompose,
            Capability::IiiWorkers,
            Capability::Node,
        ],
        deliverable_contract(kind),
    )
}

fn deliverable_contract(kind: Kind) -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: evidence_id(kind).into(),
            kind: "visual_worker_audit".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type":"object",
                "required":["identity","checks","files"],
                "properties":{"identity":{"type":"object"},"checks":{"type":"object"},"files":{"type":"object"}}
            }),
            max_size_bytes: EVIDENCE_LIMIT,
        }],
        invariants: [
            ("runtime_contract", "The run-scoped visual Worker is ready."),
            (
                "canvas_initial",
                "The Worker creates its initial diagram through Canvas.",
            ),
            (
                "canvas_update",
                "The live editor updates the same Canvas record.",
            ),
            (
                "console_delivery",
                "The Console loads the Worker page assets.",
            ),
            (
                "browser_interaction",
                "The live preview responds to a real browser interaction.",
            ),
            (
                "evidence_complete",
                "Screenshots show the Worker and Canvas graph in the full Console workspace.",
            ),
        ]
        .into_iter()
        .map(|(id, description)| InvariantSpec {
            id: id.into(),
            description: description.into(),
        })
        .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn scenario_spec(kind: Kind, run_id: &str) -> ScenarioSpec {
    let root = workspace_root(kind, run_id);
    let contract = WorkerContract::new(kind, run_id);
    let task = match kind {
        Kind::Form => form_task(&contract),
        Kind::Machine => machine_task(&contract),
    };
    ScenarioSpec {
        id: kind.id(),
        prompt: format!(
            r#"Build the {title} Worker inside `{root}`. Read README.md first. Read
`harness/ade-worker-design/index` through `directory::skills::get`, then its scoped
`console-injectable-ui` and `console-design` references. Inspect the installed
`@iii-dev/console-ui` types before using components or host APIs. The pinned UI
package, React, icons, TypeScript, and build driver are already installed.

The Harness provided `worker-compose.yaml` in this workspace with the run-scoped container
`{worker}`. Leave that file in place and do not edit another project's Compose file. The Harness
will start the Worker after your turn and first stops any process still running in this workspace,
so a Worker you start for your own checks is not the one evaluated. It already installed pinned
dependencies; do not change them.
You may add build scripts to package.json while keeping dependency versions fixed.
Register `{domain}`, `{canvas}`, and `{ui}` with non-empty descriptions and object JSON
schemas. Register console:script and console:style Message-path triggers backed by `{ui}` at
`{script}` and `{style}`. Build the ESM asset with `buildWorkerUi`; it must default-export
setup(host), register page id `{page}`, and call Worker functions through host.iii.trigger. Use
PageShell, PageHeader, PageMain, and appropriate shared controls. Scope CSS under
`[data-iii-ui="{worker}"]`; the Console manifest must have no asset warnings.

The Worker owns all domain decisions. Canvas is only the stored visual projection. `{canvas}`
accepts only optional `{{canvas_id: string, edit: string}}`; it must derive Mermaid from its domain
graph, call `canvas::validate`, then call `canvas::create` when
`canvas_id` is absent or `canvas::update` when it is present. Read the record back with
`canvas::get` and return `{{canvas_id, source, family}}`. Do not duplicate Canvas storage locally.
After the first create, a call without `canvas_id` must reuse that run-scoped active Canvas record;
the live Console page and domain functions therefore edit the same stable id.
An accepted edit must persist in the run-scoped Worker's domain state: later domain and Canvas calls
without `edit` and a browser reload must still reflect it. In-memory state is sufficient.

{task}

Create a Console page with a clear editing surface and live preview. Keep the primary action
usable in a wide pane, a narrow split pane, and on a phone; adapt to pane width, support both
themes and keyboard navigation, and avoid horizontal overflow. Show Canvas source in an element
carrying `data-testid="canvas-source"` (a disclosure is fine), domain output in
`data-testid="domain-result"`, and failures in `data-testid="error"`. Preserve the domain edit
and Canvas id after page reload. The page must remain usable after multiple interactions and must
use no external assets. Add a `data-testid="open-canvas"` button carrying the active id in
`data-canvas-id`; it must call `host.panels.open({{ pageId: 'canvas', context: {{ canvasId }} }})`.
Add focused local tests for domain behavior and verify the UI build before reporting completion."#,
            title = if kind == Kind::Form {
                "Form Flow Builder"
            } else {
                "State Machine Canvas"
            },
            root = root.display(),
            worker = contract.worker,
            domain = contract.functions[kind.domain_operation()],
            canvas = contract.functions["canvas"],
            ui = contract.functions["ui-content"],
            script = contract.script_path(),
            style = contract.style_path(),
            page = kind.page_id(),
        ),
        filesystem_root: Some(root),
        execution: ExecutionPolicy {
            max_turns: Some(256),
            max_output_tokens: Some(65_536),
            max_total_tokens: Some(6_000_000),
            stuck_timeout_seconds: 1_800,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: assessment::criteria(ASSESSMENTS),
    }
}

fn form_task(contract: &WorkerContract) -> String {
    format!(
        r#"`{preview}` accepts `{{values: object, edit?: string}}`. The SWE issue form has required base
fields `title` and `work_type`; work_type is `bug` or `feature`. Bug reveals required `reproduction`
and `expected_behavior`; feature reveals required `user_story` and `acceptance_criteria`. The edit
`add_environment` adds required `environment` to the bug branch. Return ordered `visible_fields`,
ordered `missing_required`, and `can_submit`. Reject unknown work types, edits, and non-object values.

The page is an issue-form design tool: edit field rules and fill a working preview. Start with
title and work type controls. Switching branches hides irrelevant fields without losing the
selected branch. Show which required fields are missing; incomplete input must not appear ready
to submit, and clearing a required value must disable readiness again. Provide an
`add_environment` editor control that changes the live preview and updates the same Canvas id.
Use `data-testid="work-type"`, `user-story`, `acceptance-criteria`, `reproduction`,
`expected-behavior`, `environment`, and `title`, plus `data-edit="add_environment"` on the edit
control. Put `data-can-submit="true|false"` on `domain-result` as its preview changes and put
the visible missing-field explanation in `data-testid="validation-message"`.
Persist this default flowchart
through `{canvas}`:
`flowchart TD\n  Intake --> Type{{Work item}}\n  Type -->|bug| Bug\n  Type -->|feature| Feature\n  Bug --> Ready\n  Feature --> Ready`.
After `add_environment`, insert `Environment` between Bug and Ready."#,
        preview = contract.functions["preview"],
        canvas = contract.functions["canvas"],
    )
}

fn machine_task(contract: &WorkerContract) -> String {
    format!(
        r#"`{transition}` accepts `{{state: string, event: string, edit?: string}}` and returns
`{{state: string}}`. The CI pipeline starts in queued: queued + start -> running; running + pass ->
passed; running + fail -> failed; failed + retry -> queued. The edit `add_cancel` adds running +
cancel -> cancelled. Reject every other pair and unknown edits.

The page is a CI state-machine workbench: edit transition rules, run a simulator, and inspect
history. Start at queued with Start, Pass, Fail, Retry, and a transition editor. Invalid events
must be visibly unavailable (`disabled` or `aria-disabled="true"`) without changing state or history. Exercise the pass and fail/retry
paths. Applying `add_cancel` must update the simulator and the same Canvas id. Show state in
`data-testid="current-state"` and retain visible history.
Use `data-event` for event buttons, `data-testid="reset"`, `data-testid="history"`, and
`data-edit="add_cancel"` on the edit control.
Persist this default diagram through `{canvas}`:
`stateDiagram-v2\n  [*] --> queued\n  queued --> running : start\n  running --> passed : pass\n  running --> failed : fail\n  failed --> queued : retry`.
After `add_cancel`, append `running --> cancelled : cancel`."#,
        transition = contract.functions["transition"],
        canvas = contract.functions["canvas"],
    )
}

async fn validate_candidate(context: &E2eContext, kind: Kind, run_id: &str) -> Result<Value> {
    let root = workspace_root(kind, run_id);
    let compose = root.join("worker-compose.yaml");
    let contract = WorkerContract::new(kind, run_id);
    let source_sha256 = directory_sha256(&root).ok();
    let compose_sha256 = fs::read(&compose)
        .ok()
        .map(|bytes| crate::artifact::sha256_bytes(&bytes));
    let mut checks = serde_json::Map::new();

    let local_contract = fs::read_to_string(&compose).ok()
        == Some(candidate_compose(&contract, &compose_namespace()));
    // The agent may start the Worker itself to check its work; a copy still
    // connected makes Compose refuse the Harness start (CONTAINER_NAME_TAKEN).
    let stopped_leftovers = super::common::kill_processes_under(&root).await;
    if !stopped_leftovers.is_empty() {
        for _ in 0..40 {
            if !context
                .function_exists(&contract.functions["canvas"])
                .await
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    let up = if local_contract {
        context
            .trigger_value(
                "compose::up",
                json!({"file":compose,"container":contract.worker}),
            )
            .await
    } else {
        Err(anyhow::anyhow!(
            "Harness-owned worker-compose.yaml is missing or changed"
        ))
    };
    if local_contract {
        ensure_remote_or_success(&up, "start candidate Compose container")?;
    }
    let mut ready = false;
    let mut status = Value::Null;
    if up.is_ok() {
        for _ in 0..120 {
            match context
                .trigger_value("compose::status", json!({"file":compose}))
                .await
            {
                Ok(value) => {
                    ready = value["containers"].as_array().is_some_and(|items| {
                        items.iter().any(|item| {
                            item["container"] == contract.worker && item["state"] == "ready"
                        })
                    });
                    status = value;
                    if ready {
                        break;
                    }
                }
                Err(error) if is_remote_failure(&error) => {
                    status = json!({"error":format!("{error:#}")});
                }
                Err(error) => return Err(error.context("query candidate Compose status")),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    let function_ids = contract.functions.values().cloned().collect::<Vec<_>>();
    let info = context
        .trigger_value(
            "engine::functions::info",
            json!({"function_ids":function_ids}),
        )
        .await;
    ensure_remote_or_success(&info, "inspect candidate function surface")?;
    let surface = info
        .as_ref()
        .ok()
        .and_then(|value| value["functions"].as_array())
        .is_some_and(|functions| {
            contract.functions.values().all(|id| {
                functions.iter().any(|function| {
                    function["function_id"] == *id
                        && function["description"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                        && function["request_schema"]["type"] == "object"
                        && function["response_schema"]["type"] == "object"
                })
            })
        });
    checks.insert("runtime_contract".into(), json!({
        "passed":local_contract && ready && surface,
        "reason":format!("compose_valid={local_contract}, worker_ready={ready}, function_surface={surface}"),
        "observed":{"stopped_leftover_processes":stopped_leftovers,"up":result_value(up),"status":status,"functions":result_value(info)}
    }));

    let source = if kind == Kind::Form {
        form_source()
    } else {
        machine_source()
    };
    let canvas = invoke(context.client(), &contract.functions["canvas"], json!({})).await;
    ensure_remote_or_success(&canvas, "invoke candidate Canvas projection")?;
    let canvas_id = canvas
        .as_ref()
        .ok()
        .and_then(|value| value["canvas_id"].as_str())
        .map(str::to_string);
    if let Some(canvas_id) = &canvas_id {
        let state_dir = root.join(".harness-e2e");
        fs::create_dir_all(&state_dir)?;
        fs::write(state_dir.join("canvas-id"), canvas_id)?;
    }
    // A missing canvas_id is the candidate's miss, scored below, not infrastructure.
    let first_get = if let Some(id) = &canvas_id {
        let get = invoke(context.client(), "canvas::get", json!({"id":id})).await;
        ensure_remote_or_success(&get, "read candidate Canvas record")?;
        get
    } else {
        Err(anyhow::anyhow!("candidate omitted canvas_id"))
    };
    let updated_source = edited_source(kind);
    let canvas_initial_ok = canvas_id.is_some()
        && canvas
            .as_ref()
            .ok()
            .is_some_and(|value| value["source"] == source)
        && first_get.as_ref().ok().is_some_and(|value| {
            value["id"] == canvas_id.as_deref().unwrap_or_default()
                && value["source"] == source
                && value["family"]
                    == if kind == Kind::Form {
                        "flowchart"
                    } else {
                        "stateDiagram"
                    }
        });

    let (console_delivery, console_port, manifest, script, style) =
        inspect_console(context, &contract, ready).await?;
    checks.insert("console_delivery".into(), json!({
        "passed":console_delivery,
        "reason":if console_delivery {"Console reports loadable, hashed, warning-free visual Worker assets"} else {"Console asset registration, content, status, or manifest contract failed"},
        "observed":{"manifest":bounded_value(manifest.clone()),"script":result_value(script),"style":result_value(style)}
    }));

    let identity = json!({
        "scenario":kind.id(),"worker":contract.worker,"functions":contract.functions,
        "source_sha256":source_sha256,"compose_sha256":compose_sha256,"canvas_id":canvas_id,
        "canvas_source_sha256":crate::artifact::sha256_bytes(updated_source.as_bytes()),"console_manifest":manifest,
    });
    let browser = if ready && console_delivery {
        capture_browser(
            context,
            kind,
            console_port.context("Console delivery omitted HTTP port")?,
            &identity,
        )
        .await?
    } else {
        json!({"passed":false,"status":"blocked","reason":format!("Not verified: ready={ready}, console={console_delivery}"),"captures":[]})
    };
    let browser_blocked = browser["status"] == "blocked";
    checks.insert("browser_interaction".into(), json!({"passed":browser["passed"],"status":if browser_blocked {"blocked"} else if browser["passed"] == true {"passed"} else {"failed"},"reason":browser["reason"],"observed":browser["interaction"]}));

    checks.insert("canvas_initial".into(), json!({
        "passed":canvas_initial_ok,
        "reason":if canvas_initial_ok {"Canvas created and read the exact initial projection"} else {"Canvas identity or initial source did not match"},
        "expected_source":source,
        "observed":{"create":result_value(canvas),"get":result_value(first_get)}
    }));
    if browser_blocked {
        checks.insert(
            "canvas_update".into(),
            json!({
                "passed":false,"status":"blocked",
                "reason":"Not verified: the browser was technically unavailable"
            }),
        );
    } else {
        let second_get = if let Some(id) = &canvas_id {
            let get = invoke(context.client(), "canvas::get", json!({"id":id})).await;
            ensure_remote_or_success(&get, "read browser-updated Canvas record")?;
            get
        } else {
            Err(anyhow::anyhow!("candidate omitted canvas_id"))
        };
        let canvas_update_ok = second_get.as_ref().ok().is_some_and(|value| {
            value["id"] == canvas_id.as_deref().unwrap_or_default()
                && value["source"] == updated_source
        });
        checks.insert("canvas_update".into(), json!({
            "passed":canvas_update_ok,
            "reason":if canvas_update_ok {"The live editor updated the same Canvas id to the exact edited projection"} else {"Browser-driven Canvas update or stable identity did not match"},
            "expected_source":updated_source,"observed":result_value(second_get)
        }));
    }

    let (primary, branch, invalid, health, _) = match kind {
        Kind::Form => form_probes(context.client(), &contract).await,
        Kind::Machine => machine_probes(context.client(), &contract).await,
    };
    for result in [&primary, &branch, &invalid, &health] {
        ensure_remote_or_success(result, "invoke visual Worker domain probe")?;
    }
    let (primary_ok, branch_ok, invalid_ok) =
        domain_results(kind, &primary, &branch, &invalid, &health);
    checks.insert("domain_primary".into(), json!({"passed":primary_ok,"reason":if primary_ok {"primary domain path matches the oracle"} else {"primary domain path differs from the oracle"},"observed":result_value(primary)}));
    checks.insert("domain_branch".into(), json!({"passed":branch_ok,"reason":if branch_ok {"edited domain path matches the oracle"} else {"edited domain path differs from the oracle"},"observed":result_value(branch)}));
    checks.insert("invalid_inputs".into(), json!({"passed":invalid_ok,"reason":if invalid_ok {"invalid input was rejected and the domain edit persisted into a later call"} else {"invalid-input rejection or edited-state persistence failed"},"observed":{"invalid":result_value(invalid),"health":result_value(health)}}));

    let mut files = serde_json::Map::new();
    insert_text_file(
        &mut files,
        "screenshots/captures.json",
        &serde_json::to_string_pretty(
            &json!({"identity":identity,"viewport":{"width":1280,"height":900},"captures":browser["captures"],"url":browser["url"]}),
        )?,
    );
    for name in ["before", "after", "canvas", "narrow_dark"] {
        if let Some(data) = browser[name]["data"].as_str() {
            insert_binary_file(
                &mut files,
                &format!("screenshots/{name}.png"),
                &base64::engine::general_purpose::STANDARD.decode(data)?,
            );
        }
    }
    let evidence_ok = browser["workspace_evidence"] == true
        && files.contains_key("screenshots/before.png")
        && files.contains_key("screenshots/after.png")
        && files.contains_key("screenshots/canvas.png")
        && files.contains_key("screenshots/narrow_dark.png")
        && source_sha256.is_some()
        && compose_sha256.is_some();
    checks.insert("evidence_complete".into(), json!({"passed":evidence_ok,"status":if browser_blocked {"blocked"} else if evidence_ok {"passed"} else {"failed"},"reason":if browser_blocked {"Not verified: browser prerequisites failed"} else if evidence_ok {"Worker and Canvas graph screenshots show the full Console workspace and are identity-bound"} else {"full Console screenshot or identity evidence is incomplete"}}));
    Ok(json!({"identity":identity,"checks":checks,"files":files}))
}

async fn form_probes(
    client: &IIIClient,
    contract: &WorkerContract,
) -> (
    Result<Value>,
    Result<Value>,
    Result<Value>,
    Result<Value>,
    &'static str,
) {
    let id = &contract.functions["preview"];
    let primary = invoke(client, id, json!({"values":{"title":"Login crashes","work_type":"feature","user_story":"As a user I can use passkeys","acceptance_criteria":"Passkey login succeeds"}})).await;
    let branch = invoke(client, id, json!({"edit":"add_environment","values":{"title":"Login crashes","work_type":"bug","reproduction":"Open login","expected_behavior":"Dashboard opens","environment":"Chrome"}})).await;
    let invalid = invoke(client, id, json!({"values":{"work_type":"chore"}})).await;
    let health = invoke(client, id, json!({"values":{"work_type":"bug"}})).await;
    (primary, branch, invalid, health, form_source())
}

async fn machine_probes(
    client: &IIIClient,
    contract: &WorkerContract,
) -> (
    Result<Value>,
    Result<Value>,
    Result<Value>,
    Result<Value>,
    &'static str,
) {
    let id = &contract.functions["transition"];
    let primary = invoke(client, id, json!({"state":"queued","event":"start"})).await;
    let branch = invoke(
        client,
        id,
        json!({"state":"running","event":"cancel","edit":"add_cancel"}),
    )
    .await;
    let invalid = invoke(client, id, json!({"state":"queued","event":"pass"})).await;
    let health = invoke(client, id, json!({"state":"running","event":"cancel"})).await;
    (primary, branch, invalid, health, machine_source())
}

fn domain_results(
    kind: Kind,
    primary: &Result<Value>,
    branch: &Result<Value>,
    invalid: &Result<Value>,
    health: &Result<Value>,
) -> (bool, bool, bool) {
    match kind {
        Kind::Form => {
            let matches = |result: &Result<Value>, visible: &[&str], missing: &[&str], submit| {
                result.as_ref().ok().is_some_and(|value| {
                    value["visible_fields"] == json!(visible)
                        && value["missing_required"] == json!(missing)
                        && value["can_submit"] == submit
                })
            };
            (
                matches(
                    primary,
                    &["title", "work_type", "user_story", "acceptance_criteria"],
                    &[],
                    true,
                ),
                matches(
                    branch,
                    &[
                        "title",
                        "work_type",
                        "reproduction",
                        "expected_behavior",
                        "environment",
                    ],
                    &[],
                    true,
                ),
                invalid.as_ref().err().is_some_and(is_remote_failure)
                    && matches(
                        health,
                        &[
                            "title",
                            "work_type",
                            "reproduction",
                            "expected_behavior",
                            "environment",
                        ],
                        &["title", "reproduction", "expected_behavior", "environment"],
                        false,
                    ),
            )
        }
        Kind::Machine => (
            primary
                .as_ref()
                .ok()
                .is_some_and(|value| value["state"] == "running"),
            branch
                .as_ref()
                .ok()
                .is_some_and(|value| value["state"] == "cancelled"),
            invalid.as_ref().err().is_some_and(is_remote_failure)
                && health
                    .as_ref()
                    .ok()
                    .is_some_and(|value| value["state"] == "cancelled"),
        ),
    }
}

fn form_source() -> &'static str {
    "flowchart TD\n  Intake --> Type{Work item}\n  Type -->|bug| Bug\n  Type -->|feature| Feature\n  Bug --> Ready\n  Feature --> Ready"
}

fn machine_source() -> &'static str {
    "stateDiagram-v2\n  [*] --> queued\n  queued --> running : start\n  running --> passed : pass\n  running --> failed : fail\n  failed --> queued : retry"
}

fn edited_source(kind: Kind) -> &'static str {
    match kind {
        Kind::Form => "flowchart TD\n  Intake --> Type{Work item}\n  Type -->|bug| Bug\n  Type -->|feature| Feature\n  Bug --> Environment\n  Environment --> Ready\n  Feature --> Ready",
        Kind::Machine => "stateDiagram-v2\n  [*] --> queued\n  queued --> running : start\n  running --> passed : pass\n  running --> failed : fail\n  failed --> queued : retry\n  running --> cancelled : cancel",
    }
}

async fn inspect_console(
    context: &E2eContext,
    contract: &WorkerContract,
    ready: bool,
) -> Result<(bool, Option<u16>, Value, Result<Value>, Result<Value>)> {
    let script = invoke(
        context.client(),
        &contract.functions["ui-content"],
        json!({"path":contract.script_path()}),
    )
    .await;
    let style = invoke(
        context.client(),
        &contract.functions["ui-content"],
        json!({"path":contract.style_path()}),
    )
    .await;
    ensure_remote_or_success(&script, "fetch Console script")?;
    ensure_remote_or_success(&style, "fetch Console style")?;
    let console_status = context.trigger_value("console::status", json!({})).await;
    ensure_remote_or_success(&console_status, "inspect Console status")?;
    let port = console_status
        .as_ref()
        .ok()
        .and_then(|value| value["http_port"].as_u64())
        .and_then(|port| u16::try_from(port).ok());
    let mut manifest = Value::Null;
    if ready {
        for _ in 0..40 {
            let observed = context
                .trigger_value("console::ui-manifest", json!({}))
                .await;
            ensure_remote_or_success(&observed, "inspect Console UI manifest")?;
            manifest = observed.unwrap_or(Value::Null);
            if console_assets_ok(&manifest, contract) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    let content_ok = script
        .as_ref()
        .ok()
        .and_then(|value| value["content"].as_str())
        .is_some_and(|text| !text.is_empty())
        && style
            .as_ref()
            .ok()
            .and_then(|value| value["content"].as_str())
            .is_some_and(|text| !text.is_empty());
    Ok((
        port.is_some() && console_assets_ok(&manifest, contract) && content_ok,
        port,
        manifest,
        script,
        style,
    ))
}

fn console_assets_ok(manifest: &Value, contract: &WorkerContract) -> bool {
    let expected = [
        (contract.script_path(), "script"),
        (contract.style_path(), "style"),
    ];
    let Some(assets) = manifest["assets"].as_array() else {
        return false;
    };
    let assets_ok = expected
        .iter()
        .chain(
            [
                ("canvas/page.js".to_string(), "script"),
                ("canvas/styles.css".to_string(), "style"),
                ("canvas/mermaid.js".to_string(), "script"),
            ]
            .iter(),
        )
        .all(|(path, kind)| {
            assets.iter().any(|asset| {
                asset["path"] == *path
                    && asset["kind"] == *kind
                    && asset["hash"].as_str().is_some_and(|hash| !hash.is_empty())
                    && asset["warnings"].as_array().is_some_and(Vec::is_empty)
            })
        });
    let worker_ok = manifest["workers"].as_array().is_some_and(|workers| {
        workers.iter().any(|worker| {
            worker["worker"] == contract.worker
                && worker["enabled"] == true
                && worker["assets"] == 2
        })
    });
    manifest["disabled"] == false && assets_ok && worker_ok
}

async fn capture_browser(
    context: &E2eContext,
    kind: Kind,
    port: u16,
    identity: &Value,
) -> Result<Value> {
    let screen = format!("ext:{}", kind.page_id());
    context
        .trigger_value("console::workspace::close", json!({"screen":"ext:canvas"}))
        .await
        .context("clear an earlier Canvas panel before visual evidence")?;
    let workspace = context
        .trigger_value(
            "console::workspace::open",
            json!({"screen":screen,"placement":"new-tab"}),
        )
        .await
        .context("open visual Worker in the full Console workspace")?;
    if workspace["screens"] != json!(["chat", screen]) {
        bail!("Console did not place the visual Worker beside chat: {workspace}");
    }
    let mut identity = identity.clone();
    identity["workspace"] = workspace;
    let url = format!("http://127.0.0.1:{port}/#/");
    let started = context
        .trigger_value(
            "browser::sessions::start",
            json!({"incognito":true,"ttl_ms":300000}),
        )
        .await?;
    let session = started["session_id"]
        .as_str()
        .context("browser session omitted session_id")?
        .to_string();
    let result = capture_browser_session(context, kind, &session, &url, &identity).await;
    let stopped = context
        .trigger_value("browser::sessions::stop", json!({"session_id":session}))
        .await;
    let canvas_closed = context
        .trigger_value("console::workspace::close", json!({"screen":"ext:canvas"}))
        .await;
    let worker_closed = context
        .trigger_value("console::workspace::close", json!({"screen":screen}))
        .await;
    stopped.context("stop visual Worker browser session")?;
    canvas_closed.context("close Canvas evidence panel")?;
    worker_closed.context("close visual Worker evidence panel")?;
    result
}

/// `browser::navigate` answers only after the page's `load` event, which its CDP
/// client caps at 30 seconds. The Console workspace renders and runs well before
/// a still-pending resource lets `load` fire, so that overrun is recorded on the
/// navigation instead of failing the capture; the readiness checks that follow
/// decide whether the page is usable.
async fn navigate(context: &E2eContext, session: &str, url: &str) -> Result<Value> {
    tolerate_load_timeout(
        context
            .trigger_value(
                "browser::navigate",
                json!({"session_id":session,"url":url,"timeout_ms":30000}),
            )
            .await,
    )
}

/// Navigating to the current `#/` URL is a same-document fragment navigation
/// that keeps the page as it is, so persistence is checked across a real reload.
async fn reload(context: &E2eContext, session: &str) -> Result<Value> {
    let reloaded = tolerate_load_timeout(
        context
            .trigger_value(
                "browser::history",
                json!({"session_id":session,"action":"reload"}),
            )
            .await,
    )?;
    Ok(json!({"ok":reloaded["moved"] == true || reloaded["timed_out"] == true,"reload":reloaded}))
}

fn tolerate_load_timeout(result: Result<Value>) -> Result<Value> {
    match result {
        Err(error) if is_load_timeout(&error) => {
            Ok(json!({"ok":true,"timed_out":true,"error":format!("{error:#}")}))
        }
        other => other,
    }
}

fn is_load_timeout(error: &anyhow::Error) -> bool {
    is_remote_failure(error) && format!("{error:#}").contains("Request timed out")
}

async fn capture_browser_session(
    context: &E2eContext,
    kind: Kind,
    session: &str,
    url: &str,
    identity: &Value,
) -> Result<Value> {
    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":1280,"height":900}),
        )
        .await?;
    let navigation = navigate(context, session, url).await?;
    if navigation["ok"] != true {
        return Ok(
            json!({"passed":false,"reason":format!("Worker Console page could not be rendered: {navigation}"),"captures":[],"url":url}),
        );
    }
    context.trigger_value("browser::execute", json!({"session_id":session,"timeout_ms":30000,"code":r#"return await (async()=>{for(let i=0;i<200;i++){if(document.querySelector('[data-testid="domain-result"]'))return true;await new Promise(r=>setTimeout(r,50));}return false})();"#})).await?;
    let before_state = inspect_ui(context, kind, session, "initial").await?;
    let before = screenshot_png(context, session).await?;
    let interaction_code = match kind {
        Kind::Form => {
            r#"return await (async()=>{
const wait=async f=>{for(let i=0;i<100;i++){const v=f();if(v)return v;await new Promise(r=>setTimeout(r,50))}return null};
const visible=e=>!!e&&e.getBoundingClientRect().width>20&&getComputedStyle(e).visibility!=='hidden'&&getComputedStyle(e).display!=='none';
const result=()=>document.querySelector('[data-testid="domain-result"]');
const ready=v=>result()?.dataset.canSubmit===String(v);
const set=(e,v)=>{const p=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),'value');if(!p?.set)return false;p.set.call(e,v);e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));return true};
const choose=async value=>{
  const pick=document.querySelector('[data-testid="work-type"]');if(!pick)return false;
  if(pick.tagName==='SELECT')set(pick,value);
  else {
    const radio=[...pick.querySelectorAll('input[type="radio"]')].find(e=>e.value.toLowerCase()===value||[...e.labels].some(l=>l.textContent.trim().toLowerCase()===value));
    const segment=radio||[...pick.querySelectorAll('button,[role="radio"]')].find(e=>e.textContent.trim().toLowerCase()===value);
    if(segment)segment.click();
    else {
      pick.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,cancelable:true,pointerId:1,pointerType:'mouse',isPrimary:true,button:0,buttons:1}));
      const option=await wait(()=>[...document.querySelectorAll('[role="option"],[role="menuitemradio"]')].find(e=>e.textContent.trim().toLowerCase().startsWith(value)));
      if(!option)return false;
      option.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,cancelable:true,pointerId:1,pointerType:'mouse',isPrimary:true,button:0}));
      option.click();
    }
  }
  return !!await wait(()=>value==='feature'?visible(document.querySelector('[data-testid="user-story"]')):visible(document.querySelector('[data-testid="reproduction"]')));
};
const feature=await choose('feature')&&!visible(document.querySelector('[data-testid="reproduction"]'))&&visible(document.querySelector('[data-testid="acceptance-criteria"]'));
const partial=!!await wait(()=>ready(false)&&visible(document.querySelector('[data-testid="validation-message"]'))&&document.querySelector('[data-testid="validation-message"]')?.textContent?.trim());
const bug=await choose('bug')&&!visible(document.querySelector('[data-testid="user-story"]'))&&visible(document.querySelector('[data-testid="expected-behavior"]'));
document.querySelector('[data-edit="add_environment"]')?.click();
const environment=await wait(()=>visible(document.querySelector('[data-testid="environment"]')));
const fields=[['title','Login crashes'],['reproduction','Open login'],['expected-behavior','Dashboard opens'],['environment','Chrome']];
for(const [id,value] of fields){const field=document.querySelector(`[data-testid="${id}"]`);if(!field||!set(field,value))return {error:`missing editable ${id}`}}
const complete=!!await wait(()=>ready(true));
const field=document.querySelector('[data-testid="environment"]');set(field,'');
const cleared=!!await wait(()=>ready(false));set(field,'Chrome');
const restored=!!await wait(()=>ready(true));
return feature&&partial&&bug&&environment&&complete&&cleared&&restored?{feature,partial,bug,edited:true,complete,cleared,restored}:{error:'branch visibility, validation, edited field, or recovery failed',feature,partial,bug,environment:!!environment,complete,cleared,restored};
})();"#
        }
        Kind::Machine => {
            r#"return await (async()=>{
const state=()=>document.querySelector('[data-testid="current-state"]')?.textContent?.trim();
const history=()=>document.querySelector('[data-testid="history"]')?.textContent?.trim()||'';
const wait=async expected=>{for(let i=0;i<100;i++){if(state()===expected)return true;await new Promise(r=>setTimeout(r,50))}return false};
const enabled=async event=>{for(let i=0;i<100;i++){const b=document.querySelector(`[data-event="${event}"]`);if(b&&!b.disabled&&b.getAttribute('aria-disabled')!=='true')return b;await new Promise(r=>setTimeout(r,50))}return null};
const click=async(event,expected)=>{const b=await enabled(event);if(!b)return false;b.click();return wait(expected)};
const invalid=document.querySelector('[data-event="pass"]');const before=history();invalid?.click();
const guarded=!!invalid&&(invalid.disabled||invalid.getAttribute('aria-disabled')==='true')&&state()==='queued'&&history()===before;
const pass=await click('start','running')&&await click('pass','passed')&&history()!==before;
document.querySelector('[data-testid="reset"]')?.click();await wait('queued');
const retry=await click('start','running')&&await click('fail','failed')&&await click('retry','queued');
document.querySelector('[data-edit="add_cancel"]')?.click();
const edited=!!await (async()=>{for(let i=0;i<100;i++){if(document.querySelector('[data-event="cancel"]'))return true;await new Promise(r=>setTimeout(r,50))}return false})();
const started=edited&&await click('start','running');
const cancel=started&&await click('cancel','cancelled');
const recorded=history().toLowerCase().includes('cancel');
return guarded&&pass&&retry&&cancel&&recorded?{guarded,pass,retry,cancel,recorded}:{error:'transition guard, CI paths, edit, or history failed',guarded,pass,retry,cancel,recorded};
})();"#
        }
    };
    let interaction = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"timeout_ms":30000,"code":interaction_code}),
        )
        .await?;
    let after_state = inspect_ui(context, kind, session, "edited").await?;
    let after = screenshot_png(context, session).await?;
    let expected_canvas_id = serde_json::to_string(&identity["canvas_id"])?;
    let open_canvas = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"timeout_ms":30000,"code":format!(r#"return await (async()=>{{
const expected={expected_canvas_id};
const editedLabel={};
const worker=document.querySelector('[data-testid="domain-result"]')?.closest('[data-workspace-pane-id]');
const button=document.querySelector('[data-testid="open-canvas"]');
const visible=!!button&&button.getBoundingClientRect().width>20&&button.getBoundingClientRect().height>20;
const sameId=button?.dataset.canvasId===expected;
if(visible&&sameId)button.click();
for(let i=0;i<200;i++){{
  const canvas=document.querySelector('[data-iii-ui="canvas"]');
  const canvasPane=canvas?.closest('[data-workspace-pane-id]');
  const graph=canvas?.querySelector('[aria-label^="diagram preview"] svg');
  if(worker?.isConnected&&canvasPane&&canvasPane!==worker&&graph?.textContent?.includes(editedLabel))
    return {{visible,same_id:sameId,rendered_graph:true,graph_label:editedLabel}};
  await new Promise(resolve=>setTimeout(resolve,100));
}}
return {{visible,same_id:sameId,rendered_graph:false}};
}})();"#, serde_json::to_string(if kind == Kind::Form { "Environment" } else { "cancelled" })?)}),
        )
        .await?;
    let canvas = screenshot_png(context, session).await?;
    context
        .trigger_value("console::workspace::close", json!({"screen":"ext:canvas"}))
        .await?;
    let reload = reload(context, session).await?;
    let reloaded_state = if reload["ok"] == true {
        inspect_ui(context, kind, session, "reloaded").await?
    } else {
        json!({"passed":false,"reason":"Console page did not reload"})
    };
    let persisted_canvas_id = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"code":format!("return document.querySelector('[data-testid=\"open-canvas\"]')?.dataset.canvasId === {}", expected_canvas_id)}),
        )
        .await?;
    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":480,"height":900}),
        )
        .await?;
    context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"code":"document.documentElement.dataset.theme='dark';document.documentElement.style.colorScheme='dark';return document.documentElement.dataset.theme"}),
        )
        .await?;
    let mobile = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"timeout_ms":30000,"code":r#"return await (async()=>{
const domain=document.querySelector('[data-testid="domain-result"]');
const control=document.querySelector('[data-testid="work-type"],[data-event="start"]');
const pane=domain?.closest('[data-workspace-pane-id]');
pane?.scrollIntoView({block:'nearest',inline:'nearest'});
await new Promise(requestAnimationFrame);
const bounds=pane?.getBoundingClientRect();
const fits=e=>{if(!e||!bounds)return false;const r=e.getBoundingClientRect(),s=getComputedStyle(e);return r.width>20&&r.left>=bounds.left-1&&r.right<=bounds.right+1&&s.display!=='none'&&s.visibility!=='hidden'};
return {passed:!!pane&&pane.scrollWidth<=pane.clientWidth+1&&document.documentElement.scrollWidth<=document.documentElement.clientWidth+1&&[domain,control].every(fits)&&document.documentElement.dataset.theme==='dark',viewport_width:innerWidth,pane_width:pane?.clientWidth,pane_scroll_width:pane?.scrollWidth,theme:document.documentElement.dataset.theme};
})();"#}),
        )
        .await?;
    let narrow_dark = screenshot_png(context, session).await?;
    let workspace_evidence = before_state["passed"] == true
        && after_state["passed"] == true
        && before["data"].is_string()
        && after["data"].is_string()
        && canvas["data"].is_string()
        && narrow_dark["data"].is_string()
        && open_canvas["result"]["visible"] == true
        && open_canvas["result"]["same_id"] == true
        && open_canvas["result"]["rendered_graph"] == true
        && reloaded_state["passed"] == true
        && persisted_canvas_id["result"] == true
        && mobile["result"]["passed"] == true;
    let passed = workspace_evidence && interaction["result"].get("error").is_none();
    let captures = json!([
        {"id":"before","caption":format!("{} in the full Console workspace before editing",kind.summary()),"url":url,"status":"captured","screenshot":"before.png","session_id":session,"identity":identity,"sha256":before["sha256"]},
        {"id":"after","caption":format!("{} in the full Console workspace after editing",kind.summary()),"url":url,"status":"captured","screenshot":"after.png","session_id":session,"identity":identity,"sha256":after["sha256"]},
        {"id":"canvas","caption":"The edited graph rendered by Canvas beside the Worker in the full Console workspace","url":url,"status":"captured","screenshot":"canvas.png","session_id":session,"identity":identity,"sha256":canvas["sha256"]},
        {"id":"narrow_dark","caption":format!("{} after reload in a narrow dark Console workspace",kind.summary()),"url":url,"status":"captured","screenshot":"narrow_dark.png","session_id":session,"identity":identity,"sha256":narrow_dark["sha256"]}
    ]);
    Ok(
        json!({"passed":passed,"workspace_evidence":workspace_evidence,"reason":if passed {"Worker, persisted edit, and Canvas graph rendered in the full Console workspace"} else {"Console layout, Worker interaction, reload persistence, Canvas graph, or narrow dark check failed"},"url":url,"captures":captures,"before":before,"after":after,"canvas":canvas,"narrow_dark":narrow_dark,"navigation":{"initial":navigation,"reload":reload},"interaction":{"domain":interaction["result"],"open_canvas_action":open_canvas["result"],"initial":before_state,"edited":after_state,"reloaded":reloaded_state,"persisted_canvas_id":persisted_canvas_id["result"],"workspace":identity["workspace"],"mobile":mobile["result"]}}),
    )
}

async fn inspect_ui(context: &E2eContext, kind: Kind, session: &str, phase: &str) -> Result<Value> {
    let code = format!(
        r#"const phase={};
const visible=e=>{{if(!e)return false;const r=e.getBoundingClientRect(),s=getComputedStyle(e);return r.width>20&&r.height>20&&r.bottom>0&&r.right>0&&r.top<innerHeight&&r.left<innerWidth&&s.display!=='none'&&s.visibility!=='hidden'}};
const domain=document.querySelector('[data-testid="domain-result"]'),error=document.querySelector('[data-testid="error"]');
const workspaceOk=location.hash==='#/'&&!!domain?.closest('[data-workspace-pane-id]')&&document.querySelectorAll('[data-workspace-pane-id]').length>=2;
const state=document.querySelector('[data-testid="current-state"]')?.textContent?.trim()||'';
const environment=document.querySelector('[data-testid="environment"]');
const branchOk={} ? (phase==='initial' ? !environment : phase==='edited' ? visible(environment) : true) : (phase==='initial' ? state==='queued' : phase==='edited' ? state==='cancelled' : state==='queued'||state==='cancelled');
const noOverflow=document.documentElement.scrollWidth<=document.documentElement.clientWidth+1;
return {{passed:workspaceOk&&visible(domain)&&branchOk&&noOverflow&&(!error||!error.textContent.trim()),workspace_ok:workspaceOk,state,branch_ok:branchOk,no_horizontal_overflow:noOverflow}};"#,
        serde_json::to_string(phase)?,
        if kind == Kind::Form { "true" } else { "false" },
    );
    let mut observed = Value::Null;
    for _ in 0..100 {
        let value = context
            .trigger_value(
                "browser::execute",
                json!({"session_id":session,"code":code,"timeout_ms":30000}),
            )
            .await?;
        observed = value["result"].clone();
        if observed["passed"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(observed)
}

async fn screenshot_png(context: &E2eContext, session: &str) -> Result<Value> {
    let value = context
        .trigger_value(
            "browser::screenshot",
            json!({"session_id":session,"full_page":true,"format":"png"}),
        )
        .await?;
    if value["details"]["session_id"] != session {
        bail!("browser screenshot session identity mismatch");
    }
    let block = value["content"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"] == "image" && item["mime"] == "image/png")
        })
        .context("browser screenshot omitted PNG")?;
    let data = block["data"].as_str().context("browser PNG data missing")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    if bytes.len() > MAX_SCREENSHOT_BYTES {
        return Ok(
            json!({"oversized":true,"size_bytes":bytes.len(),"maximum_bytes":MAX_SCREENSHOT_BYTES,"details":value["details"]}),
        );
    }
    Ok(
        json!({"data":data,"sha256":crate::artifact::sha256_bytes(&bytes),"details":value["details"]}),
    )
}

fn evaluate_evidence(evidence: &Value, complete: bool) -> ObjectiveEvaluation {
    assessment::build_evaluation(
        if complete {
            CompletionState::Completed
        } else {
            CompletionState::TaskIncomplete
        },
        ASSESSMENTS.iter().copied().map(|spec| {
            if evidence["checks"][spec.id()]["status"] == "blocked" {
                spec.unverified(reason(evidence, spec.id()))
            } else {
                spec.full_or_zero(passed(evidence, spec.id()), reason(evidence, spec.id()))
            }
        }),
    )
}

async fn prepare_workspace(kind: Kind, run_id: &str) -> Result<()> {
    let root = workspace_root(kind, run_id);
    remove_workspace(kind, &root)?;
    fs::create_dir_all(root.join("src"))?;
    let contract = WorkerContract::new(kind, run_id);
    fs::write(
        root.join("worker-compose.yaml"),
        candidate_compose(&contract, &compose_namespace()),
    )?;
    fs::write(root.join("README.md"), format!(
        "# {} task\n\nThe scenario prompt is authoritative. Build the run-scoped Worker `{}` here. Domain behavior belongs to this Worker; use canvas::validate/create/get/update only for its Mermaid projection. The Harness provided worker-compose.yaml here and owns its lifecycle.\n\nFor Console UI, read `harness/ade-worker-design/index` with `directory::skills::get`, then its authoring and design references. The installed `@iii-dev/console-ui` package is the exact component and build API. Use its `buildWorkerUi` driver for `ui/page.tsx` and scoped `ui/styles.css`, and serve the built assets from `dist/ui`.\n",
        if kind == Kind::Form { "Form Flow Builder" } else { "State Machine Canvas" }, contract.worker
    ))?;
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/visual-worker");
    for name in ["package.json", "package-lock.json"] {
        fs::copy(fixture.join(name), root.join(name))?;
    }
    let mut install = Command::new("npm");
    install
        .args([
            "ci",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--prefer-offline",
        ])
        .current_dir(&root)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(120), install.output())
        .await
        .context("materialize pinned Worker dependencies timed out after 120 seconds")??;
    if !output.status.success() {
        bail!(
            "could not materialize pinned Worker dependencies: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

// Compose derives `<namespace>-<container>` as the configuration id and refuses
// one over 64 characters, which a long campaign group namespace reaches; the
// run-scoped worker name is already unique and short, so name it explicitly.
fn candidate_compose(contract: &WorkerContract, namespace: &str) -> String {
    format!(
        "namespace: {namespace}\ncontainers:\n  {worker}:\n    worker: path://.\n    config_name: {worker}\n    scripts:\n      run: npm start\n",
        worker = contract.worker
    )
}

fn compose_namespace() -> String {
    std::env::var("III_NAMESPACE").unwrap_or_else(|_| "default".into())
}

async fn cleanup_workspace(context: &E2eContext, kind: Kind, run_id: &str) -> Result<()> {
    let root = workspace_root(kind, run_id);
    let compose = root.join("worker-compose.yaml");
    fs::create_dir_all(&root)?;
    match fs::remove_file(&compose) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::write(
        &compose,
        candidate_compose(&WorkerContract::new(kind, run_id), &compose_namespace()),
    )?;
    context
        .trigger_value("compose::down", json!({"file":compose}))
        .await
        .context("stop run-scoped visual Worker")?;
    super::common::kill_processes_under(&root).await;
    if let Ok(canvas_id) = fs::read_to_string(root.join(".harness-e2e/canvas-id")) {
        let canvas_id = canvas_id.trim();
        if !canvas_id.is_empty() {
            let deleted = invoke(context.client(), "canvas::delete", json!({"id":canvas_id})).await;
            ensure_remote_or_success(&deleted, "delete run-owned Canvas record")?;
        }
    }
    remove_workspace(kind, &root)
}

fn workspace_root(kind: Kind, run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::canonicalize(&base)
        .unwrap_or(base)
        .join("scenario-workspaces")
        .join(format!("{}-{run_id}", kind.id()))
}

fn remove_workspace(kind: Kind, root: &Path) -> Result<()> {
    let parent = root.parent().context("workspace path has no parent")?;
    let name = root
        .file_name()
        .and_then(OsStr::to_str)
        .context("workspace path has no name")?;
    if parent.file_name() != Some(OsStr::new("scenario-workspaces"))
        || !name.starts_with(&format!("{}-", kind.id()))
    {
        bail!("refusing to remove workspace outside the scenario-owned base");
    }
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn directory_sha256(root: &Path) -> Result<String> {
    fn collect(root: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_symlink() {
                bail!(
                    "candidate source fingerprint rejects symlink {}",
                    path.display()
                );
            }
            if entry.file_type()?.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("node_modules" | ".git" | ".iii" | "target" | ".harness-e2e")
                ) {
                    collect(root, &path, files)?;
                }
            } else if entry.file_type()?.is_file() {
                files.push(
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
                if files.len() > 256 {
                    bail!("candidate source fingerprint exceeds 256 files");
                }
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort();
    let mut bytes = Vec::new();
    let mut total = 0_u64;
    for relative in files {
        let content = fs::read(root.join(&relative))?;
        total = total.saturating_add(content.len() as u64);
        if total > 8 * 1024 * 1024 {
            bail!("candidate source fingerprint exceeds 8 MiB");
        }
        bytes.extend_from_slice(relative.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(
            crate::artifact::sha256_bytes(&content)
                .trim_start_matches("sha256:")
                .as_bytes(),
        );
        bytes.push(b'\n');
    }
    Ok(crate::artifact::sha256_bytes(&bytes))
}

async fn invoke(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    client
        .trigger(TriggerRequest {
            function_id: function_id.into(),
            payload,
            action: None,
            timeout_ms: Some(30_000),
        })
        .await
        .map_err(anyhow::Error::new)
        .with_context(|| format!("invoke {function_id}"))
}

fn ensure_remote_or_success(result: &Result<Value>, action: &str) -> Result<()> {
    if let Err(error) = result {
        if !is_remote_failure(error) {
            bail!("{action} infrastructure failure: {error:#}");
        }
    }
    Ok(())
}

fn is_remote_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Remote { .. })
    )
}

fn result_value(result: Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"value":bounded_value(value)}),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    }
}

fn bounded_value(value: Value) -> Value {
    let encoded = value.to_string();
    if encoded.len() <= 16 * 1024 {
        value
    } else {
        json!({"omitted":"response exceeded 16 KiB","sha256":crate::artifact::sha256_bytes(encoded.as_bytes()),"size_bytes":encoded.len()})
    }
}

fn passed(evidence: &Value, id: &str) -> bool {
    evidence["checks"][id]["passed"] == true
}
fn reason(evidence: &Value, id: &str) -> String {
    evidence["checks"][id]["reason"]
        .as_str()
        .unwrap_or("check did not produce a reason")
        .into()
}

fn insert_text_file(files: &mut serde_json::Map<String, Value>, name: &str, text: &str) {
    let bytes = text.as_bytes();
    files.insert(name.into(),json!({"encoding":"utf8","content":text,"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(bytes)}));
}

fn insert_binary_file(files: &mut serde_json::Map<String, Value>, name: &str, bytes: &[u8]) {
    files.insert(name.into(),json!({"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(bytes),"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(bytes)}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_places_candidate_compose_in_the_worker_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("worker");
        fs::create_dir(&root).unwrap();
        let contract = WorkerContract::new(Kind::Form, "attempt");
        fs::write(
            root.join("worker-compose.yaml"),
            candidate_compose(&contract, "scenario-test"),
        )
        .unwrap();

        let yaml: Value =
            serde_yaml::from_str(&fs::read_to_string(root.join("worker-compose.yaml")).unwrap())
                .unwrap();
        assert_eq!(yaml["namespace"], "scenario-test");
        assert_eq!(yaml["containers"].as_object().unwrap().len(), 1);
        assert_eq!(yaml["containers"][&contract.worker]["worker"], "path://.");
        assert_eq!(
            yaml["containers"][&contract.worker]["config_name"],
            contract.worker.as_str()
        );
        assert_eq!(
            yaml["containers"][&contract.worker]["scripts"]["run"],
            "npm start"
        );
        assert!(yaml.get("engine").is_none());
    }

    #[test]
    fn both_contracts_are_run_scoped_and_score_one_hundred() {
        for kind in [Kind::Form, Kind::Machine] {
            let first = WorkerContract::new(kind, "run-a");
            let second = WorkerContract::new(kind, "run-b");
            assert_ne!(first.worker, second.worker);
            assert!(first
                .functions
                .values()
                .all(|id| id.starts_with(&first.worker)));
            let spec = scenario_spec(kind, "run-a");
            spec.validate().unwrap();
            assert_eq!(
                spec.criteria
                    .iter()
                    .map(|item| u16::from(item.weight))
                    .sum::<u16>(),
                100
            );
        }
    }

    #[test]
    fn only_a_remote_load_timeout_is_tolerated_on_navigation() {
        let remote = |message: &str| {
            anyhow::Error::new(iii_sdk::errors::Error::Remote {
                code: "invocation_failed".into(),
                message: message.into(),
                stacktrace: None,
            })
            .context("invoke browser::navigate")
        };
        assert!(is_load_timeout(&remote(
            "handler error: navigation failed: Request timed out."
        )));
        assert!(!is_load_timeout(&remote("unknown session_id")));
        assert!(!is_load_timeout(
            &anyhow::Error::new(iii_sdk::errors::Error::Timeout)
                .context("invoke browser::navigate")
        ));
    }

    #[test]
    fn blocked_browser_preserves_domain_and_canvas_scores() {
        let checks=ASSESSMENTS.iter().map(|spec| {
            let blocked=matches!(spec.id(),"canvas_update"|"browser_interaction"|"evidence_complete");
            (spec.id().to_string(),json!({"passed":!blocked,"status":if blocked{"blocked"}else{"evaluated"},"reason":"probe"}))
        }).collect::<serde_json::Map<_,_>>();
        let evaluation = evaluate_evidence(&json!({"checks":checks}), true);
        assert_eq!(
            evaluation
                .awards
                .iter()
                .filter_map(|award| award.awarded)
                .sum::<u8>(),
            75
        );
        assert!(evaluation
            .awards
            .iter()
            .filter(|award| matches!(
                award.id.as_str(),
                "canvas_update" | "browser_interaction" | "evidence_complete"
            ))
            .all(|award| award.awarded.is_none()));
    }

    #[test]
    fn independent_oracles_cover_both_domain_branches() {
        let ok = Ok(
            json!({"visible_fields":["title","work_type","user_story","acceptance_criteria"],"missing_required":[],"can_submit":true}),
        );
        let branch = Ok(
            json!({"visible_fields":["title","work_type","reproduction","expected_behavior","environment"],"missing_required":[],"can_submit":true}),
        );
        let invalid = Err(anyhow::anyhow!(iii_sdk::errors::Error::Remote {
            code: "invalid_input".into(),
            message: "rejected".into(),
            stacktrace: None,
        }));
        let health = Ok(
            json!({"visible_fields":["title","work_type","reproduction","expected_behavior","environment"],"missing_required":["title","reproduction","expected_behavior","environment"],"can_submit":false}),
        );
        assert_eq!(
            domain_results(Kind::Form, &ok, &branch, &invalid, &health),
            (true, true, true)
        );
        assert!(form_source().starts_with("flowchart TD"));
        assert!(machine_source().starts_with("stateDiagram-v2"));
    }
}
