//! Evaluate a planned, Console-native incident board without asking the
//! subject to build or modify its Worker backend.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedDeliverableContent,
    CapturedInvariant, DeliverableContract, ExecutionPolicy, InvariantSpec, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "ade_incident_board";
pub const SUMMARY: &str = "Plan and build a Console-native incident-triage experience. The runner checks UI build quality, workspace panes, persisted edits, live updates, recovery, responsive behavior, and visual evidence.";

const EVIDENCE_ID: &str = "incident_board_evidence";
const EVIDENCE_LIMIT: u64 = 12 * 1024 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 1_500 * 1024;
const TAB_SWITCH_PROBE: &str = r#"return await (async () => {
  const tabs = [...document.querySelectorAll('[data-testid="incident-board"] [role="tab"]')];
  const visible = status => [...document.querySelectorAll('[data-status-lane]')]
    .filter(lane => lane.getClientRects().length > 0)
    .map(lane => lane.dataset.statusLane).join() === status;
  for (const status of ['resolved', 'new']) {
    const tab = tabs.find(tab => tab.textContent.trim().toLowerCase().includes(status));
    if (!tab) return false;
    tab.click();
    let switched = false;
    for (let i = 0; i < 40; i++) {
      if (visible(status)) { switched = true; break; }
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    if (!switched || tab.getAttribute('aria-selected') !== 'true') return false;
  }
  return true;
})();"#;
static DEPENDENCY_FINGERPRINTS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
const SCOPE: AssessmentSpec = AssessmentSpec::scored_in(
    "scope_integrity",
    8,
    "Only the page, scoped stylesheet, and design plan may change; generated build output is ignored.",
    EvaluationDimension::StructuralIntegrity,
);
const PLAN: AssessmentSpec = AssessmentSpec::scored_in(
    "design_plan",
    10,
    "The plan names the responder, problem, and outcome; covers pane, state, layout, theme, and keyboard decisions; and places a Verify: step beside each AC.",
    EvaluationDimension::StructuralIntegrity,
);
const BUILD: AssessmentSpec = AssessmentSpec::scored_in(
    "ui_build",
    8,
    "The page and CSS pass the pinned Console UI build, strict design lint, and TypeScript typecheck.",
    EvaluationDimension::Deliverable,
);
const CONSOLE: AssessmentSpec = AssessmentSpec::scored_in(
    "console_delivery",
    10,
    "The Console registers the page script and scoped stylesheet with no asset warnings.",
    EvaluationDimension::Deliverable,
);
const BOARD: AssessmentSpec = AssessmentSpec::scored_in(
    "board_search",
    14,
    "9 points: five incidents in the right lanes with counts. 5 points: searching for cache filters the records and updates every count.",
    EvaluationDimension::Deliverable,
);
const DETAIL: AssessmentSpec = AssessmentSpec::scored_in(
    "detail_and_persistence",
    18,
    "6 points: Enter opens a real detail pane. 6 points: each field saves before the next edit, without a Save button. 6 points: values and history survive reload.",
    EvaluationDimension::Deliverable,
);
const EXTERNAL: AssessmentSpec = AssessmentSpec::scored_in(
    "external_live_update",
    14,
    "An active incident subscription shows an external priority change and history event in the open detail pane without reloading.",
    EvaluationDimension::Robustness,
);
const RECOVERY: AssessmentSpec = AssessmentSpec::scored_in(
    "empty_and_error_recovery",
    8,
    "3 points: the empty state has a visible message. 5 points: a failed-load message and Retry restore all five incidents.",
    EvaluationDimension::Robustness,
);
const UX: AssessmentSpec = AssessmentSpec::scored_in(
    "responsive_theme_keyboard",
    6,
    "3 points: board and detail fit wide, phone, and narrow split panes, with working lane tabs. 1 point: both themes render. 2 points: Enter opens a card with visible focus.",
    EvaluationDimension::Deliverable,
);
const VISUAL_EVIDENCE: AssessmentSpec = AssessmentSpec::scored_in(
    "visual_evidence",
    4,
    "The artifact contains four size-bounded, hash-verified Console screenshots: board and detail at wide and phone sizes.",
    EvaluationDimension::StructuralIntegrity,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    SCOPE,
    PLAN,
    BUILD,
    CONSOLE,
    BOARD,
    DETAIL,
    EXTERNAL,
    RECOVERY,
    UX,
    VISUAL_EVIDENCE,
];

pub struct AdeIncidentBoard;

#[async_trait]
impl Scenario for AdeIncidentBoard {
    fn id(&self) -> &'static str {
        ID
    }

    fn summary(&self) -> Option<&'static str> {
        Some(SUMMARY)
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            self.canonical_seed(),
            json!({
                "task": "plan-and-build-console-incident-triage-ui",
                "worker_backend": "runner-owned-and-frozen",
                "allowed_candidate_files": ["ui/page.tsx", "ui/styles.css", "ui/design-plan.md"],
                "acceptance_criteria": ["AC-01", "AC-02", "AC-03", "AC-04", "AC-05"],
                "live_update_probe": "runner-mutates-open-record-without-reload",
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::BrowserInteractive,
                Capability::IiiCompose,
                Capability::IiiWorkers,
                Capability::IiiCoder,
                Capability::Node,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let contract = worker_contract(run_id);
        let root = workspace_root(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                r#"Build an incident-triage experience for the iii Console in `{root}`.
Follow the instructions available in your assigned agent profile. Read the package's installed
`@iii-dev/console-ui` types and README. Write
`ui/design-plan.md` before implementing the page.

The Worker, API, incident records, Compose file, dependencies, and start script are already
complete and frozen. Change only `ui/page.tsx`, `ui/styles.css`, and `ui/design-plan.md`.
Do not edit backend, Compose, package files, add dependencies, or launch the Worker directly.
The Harness builds, starts, and stops this run-scoped Worker after your turn. Use the existing
documented functions from the UI; never use the runner-only fixture function from candidate code.

The main page id is `{board_page}` and the contextual detail page id is `{detail_page}`.
The board must have four lanes: New, Investigating, Monitoring, and Resolved. It must show seeded
incidents, useful search, status counts, clear loading/empty/error/retry states, and keyboard
operable actions. Use shared Console UI components and tokens. At wide widths show horizontal
status lanes; at narrow pane widths use one visible lane selected with tabs. Selecting a record
must open the separate detail page through
`host.panels.open({{pageId: '{detail_page}', context: {{id}}}})`. Read the selected id from
`panelContext.context` in that page. Use `host.iii.trigger` for list, get, and update calls.
The detail page must show priority, current state, assignee, and event history; let a responder
change status and assignee with immediate per-field persistence and no global Save button. Use
`useWorkerLive` from `@iii-dev/console-ui/hooks` for the provided `{change_trigger}` trigger;
keep the selected incident open so an external change appears without reloading the browser.
Restore the selected incident after a browser reload. Preserve normal
Console behavior and support the Console's light/dark theme.

Design and implement against these acceptance criteria. Put the same ids in your plan and include
one concrete `Verify:` action for each:
- AC-01: Search and inspect incidents in all four lanes; each lane count matches visible records.
- AC-02: Open a record in its own Console pane, update status and assignee, then reload and see
  the saved values and history.
- AC-03: Keep a record open while an external update arrives; show its changed priority and new
  history event without a browser reload.
- AC-04: Show useful empty and recoverable error states, then restore the board with Retry.
- AC-05: Make the board adapt from horizontal lanes to one tab-selected lane in a narrow pane;
  make board and detail usable on phone and wide viewports, in both themes, with keyboard-operable
  controls and visible focus.

Before coding, the plan should identify the responder, the incident-triage problem and outcome,
the information hierarchy and pane decision, and how state, search, loading, empty, error, and
success behave. Include responsive, theme, keyboard, and overflow decisions. Keep the plan concise
and make its `Verify:` actions reproducible against this workspace and Console. Add these semantic
test hooks to the corresponding UI elements so the Harness can repeat the acceptance probes:
`data-testid="incident-board"`, `data-status-lane="new|investigating|monitoring|resolved"`,
`data-testid="lane-count"`, `aria-label="Search incidents"`, `data-incident-id`,
`data-testid="incident-detail"`, `aria-label="Status"`, `aria-label="Assignee"`,
`data-testid="refresh-incidents"`, `data-testid="empty-state"`, `data-testid="error-state"`,
and `data-testid="retry-incidents"`. Keep the hooks on meaningful
semantic elements; do not add hidden elements just for the probe."#,
                root = root.display(),
                board_page = contract.board_page,
                detail_page = contract.detail_page,
                change_trigger = contract.change_trigger,
            ),
            filesystem_root: Some(root),
            execution: ExecutionPolicy {
                max_turns: Some(60),
                max_output_tokens: Some(32_768),
                max_total_tokens: Some(500_000),
                stuck_timeout_seconds: 600,
                max_validation_retries: None,
            },
            denied_functions: &[
                "compose::*",
                "console::*",
                "browser::*",
                "engine::*",
                "worker::*",
                "harness::spawn",
            ],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(vec!["coder::*".into(), "directory::skills::get".into()])
    }

    fn required_functions(&self, _run_id: &str) -> Vec<String> {
        [
            "coder::read-file",
            "coder::update-file",
            "coder::create-file",
            "directory::skills::get",
            "compose::validate",
            "compose::up",
            "compose::status",
            "compose::down",
            "engine::functions::info",
            "console::status",
            "console::ui-manifest",
            "console::workspace::open",
            "console::workspace::list",
            "console::workspace::close",
            "browser::sessions::start",
            "browser::sessions::stop",
            "browser::resize",
            "browser::navigate",
            "browser::execute",
            "browser::act",
            "browser::screenshot",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    async fn setup(&self, _context: &E2eContext, run_id: &str) -> Result<()> {
        prepare_workspace(run_id).await
    }

    async fn capture(
        &self,
        context: &E2eContext,
        _observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let evidence = evaluate_candidate(context, run_id).await?;
        let invariants = ASSESSMENTS
            .iter()
            .map(|assessment| CapturedInvariant {
                id: assessment.id().to_string(),
                passed: evidence["checks"][assessment.id()]["passed"] == true,
                reason: evidence["checks"][assessment.id()]["reason"]
                    .as_str()
                    .unwrap_or("check did not produce a reason")
                    .to_string(),
            })
            .collect();
        Ok(vec![CapturedDeliverable {
            id: EVIDENCE_ID.into(),
            kind: "ade_incident_board_audit".into(),
            content: CapturedDeliverableContent::Json(evidence),
            invariants,
            provenance: vec![ProvenanceEvidence {
                kind: "filesystem_path".into(),
                source_id: workspace_root(run_id).display().to_string(),
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
            .find(|item| item.id == EVIDENCE_ID)
            .and_then(|item| item.content.as_json())
            .context("incident board evidence deliverable is missing")?;
        let infrastructure = evidence["runtime_prerequisite"]["status"] == "blocked"
            || evidence["checks"]["console_delivery"]["status"] == "blocked"
            || [
                "board_search",
                "detail_and_persistence",
                "external_live_update",
                "empty_and_error_recovery",
                "responsive_theme_keyboard",
            ]
            .iter()
            .any(|id| evidence["checks"][*id]["status"] == "blocked");
        let awards = ASSESSMENTS
            .iter()
            .copied()
            .map(|spec| {
                let check = &evidence["checks"][spec.id()];
                let reason = check["reason"].as_str().unwrap_or("check failed");
                if let Some(points) = check["awarded"].as_u64() {
                    spec.award(u8::try_from(points)?, reason)
                } else {
                    Ok(spec.full_or_zero(check["passed"] == true, reason))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let mut evaluation = assessment::build_evaluation(
            if infrastructure {
                CompletionState::Undetermined
            } else if observation.metrics.complete {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            awards,
        );
        for award in &mut evaluation.awards {
            if evidence["checks"][award.id.as_str()]["status"] == "blocked" {
                award.awarded = None;
            }
        }
        if infrastructure {
            evaluation.infrastructure_error =
                if evidence["runtime_prerequisite"]["status"] == "blocked" {
                    evidence["runtime_prerequisite"]["reason"]
                        .as_str()
                        .map(str::to_string)
                } else if evidence["checks"]["console_delivery"]["status"] == "blocked" {
                    evidence["checks"]["console_delivery"]["reason"]
                        .as_str()
                        .map(str::to_string)
                } else {
                    [
                        "board_search",
                        "detail_and_persistence",
                        "external_live_update",
                        "empty_and_error_recovery",
                        "responsive_theme_keyboard",
                    ]
                    .iter()
                    .find_map(|id| {
                        let check = &evidence["checks"][*id];
                        (check["status"] == "blocked")
                            .then(|| check["reason"].as_str().map(str::to_string))
                            .flatten()
                    })
                };
        }
        Ok(evaluation)
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let root = workspace_root(run_id);
        let contract = worker_contract(run_id);
        for page in [&contract.board_page, &contract.detail_page] {
            let _ = context
                .trigger_value(
                    "console::workspace::close",
                    json!({"screen": format!("ext:{page}")}),
                )
                .await;
        }
        let compose = root.join("worker-compose.yaml");
        if fs::read_to_string(&compose).ok().as_deref() != Some(compose_file(&contract).as_str()) {
            bail!("refusing to remove the workspace because its run-scoped Compose file changed; inspect and stop that Compose project first");
        }
        context
            .trigger_value("compose::down", json!({"file": compose}))
            .await
            .context("stop the run-scoped incident Worker before removing its workspace")?;
        if let Ok(mut fingerprints) = dependency_fingerprints().lock() {
            fingerprints.remove(run_id);
        }
        remove_workspace(&root)
    }
}

#[derive(Clone)]
struct WorkerContract {
    worker: String,
    functions: BTreeMap<&'static str, String>,
    board_page: String,
    detail_page: String,
    change_trigger: String,
}

fn worker_contract(run_id: &str) -> WorkerContract {
    let suffix: String = run_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(12)
        .collect();
    let suffix = if suffix.is_empty() { "run" } else { &suffix };
    let worker = format!("incident_{suffix}");
    let functions = ["list", "get", "update", "fixture", "ui-content"]
        .into_iter()
        .map(|name| (name, format!("{worker}::{name}")))
        .collect();
    WorkerContract {
        board_page: format!("incident-{suffix}-board"),
        detail_page: format!("incident-{suffix}-detail"),
        change_trigger: format!("incident-{suffix}:change"),
        worker,
        functions,
    }
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: EVIDENCE_ID.into(),
            kind: "ade_incident_board_audit".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type": "object",
                "required": ["identity", "checks", "files"],
                "properties": {
                    "identity": {"type": "object"},
                    "checks": {"type": "object"},
                    "files": {"type": "object"}
                }
            }),
            max_size_bytes: EVIDENCE_LIMIT,
        }],
        invariants: ASSESSMENTS
            .iter()
            .map(|assessment| InvariantSpec {
                id: assessment.id().to_string(),
                description: assessment.description().to_string(),
            })
            .collect(),
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn worker_source(contract: &WorkerContract) -> String {
    r#"import { readFile } from 'node:fs/promises'
import { registerWorker, TriggerAction } from 'iii-sdk'

const worker = '{{worker}}'
if (process.env.ADE_INCIDENT_BOARD_COMPOSE_RUN !== worker) throw new Error('Start this frozen Worker through its run-scoped Compose project.')
const files = {
  '{{script_path}}': 'dist/ui/page.js',
  '{{style_path}}': 'dist/ui/styles.css',
}
const records = new Map([
  ['INC-104', { id: 'INC-104', title: 'Elevated 5xx rate in orders API', service: 'Orders API', priority: 'high', status: 'investigating', assignee: 'Mira Chen', summary: 'Checkout requests are failing after the latest gateway rollout.', started_at: '2026-09-23T09:14:00Z', history: [{ at: '09:14', actor: 'Monitor', text: '5xx rate exceeded 8% for five minutes.' }, { at: '09:20', actor: 'Mira Chen', text: 'Started tracing the gateway deployment.' }] }],
  ['INC-105', { id: 'INC-105', title: 'Cache miss rate rising in catalog', service: 'Catalog', priority: 'critical', status: 'new', assignee: 'Unassigned', summary: 'A regional cache pool is serving stale shard mappings.', started_at: '2026-09-23T09:38:00Z', history: [{ at: '09:38', actor: 'Monitor', text: 'Cache misses reached 42% in eu-west.' }] }],
  ['INC-106', { id: 'INC-106', title: 'Delayed webhook deliveries', service: 'Events', priority: 'medium', status: 'monitoring', assignee: 'Ana Costa', summary: 'Delivery latency is falling after queue workers were scaled up.', started_at: '2026-09-23T08:52:00Z', history: [{ at: '08:52', actor: 'Monitor', text: 'p95 delivery latency passed two minutes.' }, { at: '09:05', actor: 'Ana Costa', text: 'Added workers; watching queue depth.' }] }],
  ['INC-107', { id: 'INC-107', title: 'Search indexing backlog cleared', service: 'Search', priority: 'low', status: 'resolved', assignee: 'Leo Martin', summary: 'The indexer caught up after replaying a delayed partition.', started_at: '2026-09-23T07:10:00Z', history: [{ at: '07:10', actor: 'Monitor', text: 'Index lag crossed the alert threshold.' }, { at: '08:01', actor: 'Leo Martin', text: 'Backlog cleared and search freshness recovered.' }] }],
  ['INC-108', { id: 'INC-108', title: 'Elevated latency in billing preview', service: 'Billing', priority: 'medium', status: 'new', assignee: 'Unassigned', summary: 'Preview requests are slower than their normal baseline.', started_at: '2026-09-23T09:47:00Z', history: [{ at: '09:47', actor: 'Monitor', text: 'p95 latency increased to 1.8 seconds.' }] }],
])
let empty = false
let failList = false
const copy = (value) => structuredClone(value)
const iii = registerWorker(process.env.III_ENGINE_URL ?? process.env.III_URL, { workerName: worker })
const subscribers = new Map()
iii.registerTriggerType({ id: '{{change_trigger}}', description: 'Fires when an incident changes.' }, {
  async registerTrigger(binding) { subscribers.set(binding.id, binding) },
  async unregisterTrigger(binding) { subscribers.delete(binding.id) },
})
function emitChange(incident) {
  for (const binding of subscribers.values()) {
    if (binding.config?.id && binding.config.id !== incident.id) continue
    iii.trigger({
      function_id: binding.function_id,
      namespace: binding.namespace,
      payload: { incident: copy(incident) },
      ...(binding.metadata ? { metadata: binding.metadata } : {}),
      action: TriggerAction.Void(),
    }).catch(() => undefined)
  }
}

iii.registerFunction('{{list}}', async () => {
  if (failList) throw new Error('Incident service is temporarily unavailable.')
  return { incidents: empty ? [] : [...records.values()].map(copy) }
}, { description: 'List current incidents for the Console triage board.', request_format: { type: 'object', properties: {} }, response_format: { type: 'object', required: ['incidents'], properties: { incidents: { type: 'array', items: { type: 'object' } } } } })

iii.registerFunction('{{get}}', async ({ id }) => {
  const incident = records.get(id)
  if (!incident) throw new Error(`Unknown incident: ${id}`)
  return { incident: copy(incident) }
}, { description: 'Read one incident and its timeline.', request_format: { type: 'object', required: ['id'], properties: { id: { type: 'string' } } }, response_format: { type: 'object', required: ['incident'], properties: { incident: { type: 'object' } } } })

iii.registerFunction('{{update}}', async ({ id, status, assignee }) => {
  const incident = records.get(id)
  if (!incident) throw new Error(`Unknown incident: ${id}`)
  if (!['new', 'investigating', 'monitoring', 'resolved'].includes(status)) throw new Error('Invalid incident status.')
  if (typeof assignee !== 'string' || !assignee.trim()) throw new Error('Assignee is required.')
  incident.status = status
  incident.assignee = assignee.trim()
  incident.history.push({ at: new Date().toISOString().slice(11, 16), actor: 'Responder', text: `Assigned to ${incident.assignee}; status changed to ${status}.` })
  emitChange(incident)
  return { incident: copy(incident) }
}, { description: 'Persist an incident status and assignee update.', request_format: { type: 'object', required: ['id', 'status', 'assignee'], properties: { id: { type: 'string' }, status: { type: 'string', enum: ['new', 'investigating', 'monitoring', 'resolved'] }, assignee: { type: 'string' } } }, response_format: { type: 'object', required: ['incident'], properties: { incident: { type: 'object' } } } })

iii.registerFunction('{{fixture}}', async ({ action, enabled, id }) => {
  if (action === 'subscriber-count') return { ok: true, subscribers: [...subscribers.values()].filter(binding => !binding.config?.id || binding.config.id === id).length }
  if (action === 'empty') empty = Boolean(enabled)
  else if (action === 'fail-list') failList = Boolean(enabled)
  else if (action === 'restore') { empty = false; failList = false }
  else if (action === 'external-update') {
    const incident = records.get(id)
    if (!incident) throw new Error(`Unknown incident: ${id}`)
    incident.priority = 'critical'
    incident.history.push({ at: new Date().toISOString().slice(11, 16), actor: 'Release monitor', text: 'A new deployment increased impact; severity raised to critical.' })
    emitChange(incident)
  } else throw new Error(`Unknown fixture action: ${action}`)
  return { ok: true, subscribers: action === 'external-update' ? [...subscribers.values()].filter(binding => !binding.config?.id || binding.config.id === id).length : undefined }
}, { description: 'Harness-only deterministic incident fixture controls; never call from the submitted UI.', request_format: { type: 'object', required: ['action'], properties: { action: { type: 'string' }, enabled: { type: 'boolean' }, id: { type: 'string' } } }, response_format: { type: 'object', required: ['ok'], properties: { ok: { type: 'boolean' }, subscribers: { type: 'integer' } } } })

iii.registerFunction('{{ui_content}}', async ({ path }) => {
  const file = files[path]
  if (!file) throw new Error(`Unknown Console UI asset: ${path}`)
  return { content: await readFile(new URL(`../${file}`, import.meta.url), 'utf8'), content_type: file.endsWith('.css') ? 'text/css' : 'text/javascript' }
}, { description: 'Read an approved incident Console UI asset.', request_format: { type: 'object', required: ['path'], properties: { path: { type: 'string' } } }, response_format: { type: 'object', required: ['content', 'content_type'], properties: { content: { type: 'string' }, content_type: { type: 'string' } } } })

iii.registerTrigger({ type: 'console:script', function_id: '{{ui_content}}', config: { path: '{{script_path}}' } })
iii.registerTrigger({ type: 'console:style', function_id: '{{ui_content}}', config: { path: '{{style_path}}' } })
"#
    .replace("{{worker}}", &contract.worker)
    .replace("{{change_trigger}}", &contract.change_trigger)
    .replace("{{script_path}}", &format!("{}/page.js", contract.worker))
    .replace("{{style_path}}", &format!("{}/styles.css", contract.worker))
    .replace("{{list}}", &contract.functions["list"])
    .replace("{{get}}", &contract.functions["get"])
    .replace("{{update}}", &contract.functions["update"])
    .replace("{{fixture}}", &contract.functions["fixture"])
    .replace("{{ui_content}}", &contract.functions["ui-content"])
}

fn readme(contract: &WorkerContract) -> String {
    format!(
        "# Incident triage UI task\n\nThe Runner supplies this live Worker and owns its backend, data, Compose file, pinned packages, and shared build driver. Design and build the Console pages only. Change only `ui/page.tsx`, `ui/styles.css`, and `ui/design-plan.md`.\n\n## Registered functions\n\n- `{}` returns `{{ incidents: Incident[] }}` with five deterministic records. Incident fields are `id`, `title`, `service`, `priority`, `status`, `assignee`, `summary`, `started_at`, and `history[]` (`at`, `actor`, `text`).\n- `{}` accepts `{{ id }}` and returns `{{ incident }}`.\n- `{}` accepts `{{ id, status, assignee }}` and persists the update plus an audit-history entry. Status is `new`, `investigating`, `monitoring`, or `resolved`; mutations emit a `{}` event.\n- `{}` is reserved for runner probes; do not call it from UI code.\n- `{}` serves only the built page and stylesheet assets.\n\n## Console contract\n\nUse the installed `@iii-dev/console-ui` `Host` and `PageRenderProps` types to register both pages. `host.iii.trigger(functionId, payload)` invokes Worker functions. `host.panels.open({{ pageId, context: {{ id }} }})` opens the detail page in its own pane in the same workspace tab; its render props include `panelContext.context.id`. Persist pane state with `usePaneState`, use `useWorkerLive` for live changes, and scope CSS beneath `[data-iii-ui=\"{}\"]`.\n\nThe Harness validates the UI build, starts this run-scoped Worker through Compose, and opens the real Console after your turn. Do not run the Worker directly.\n",
        contract.functions["list"],
        contract.functions["get"],
        contract.functions["update"],
        contract.change_trigger,
        contract.functions["fixture"],
        contract.functions["ui-content"],
        contract.worker,
    )
}

fn compose_file(contract: &WorkerContract) -> String {
    format!(
        "containers:\n  {}:\n    worker: path://.\n    environment:\n      ADE_INCIDENT_BOARD_COMPOSE_RUN: {}\n    scripts:\n      run: npm start\n",
        contract.worker,
        contract.worker,
    )
}

fn package_json() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "name": "ade-incident-board-fixture",
        "private": true,
        "type": "module",
        "scripts": {
            "start": "npm run build:ui && node src/index.mjs",
            "build:ui": "node ui/build.mjs",
            "typecheck": "tsc -p ui/tsconfig.json"
        },
        "dependencies": {
            "@iii-dev/console-ui": "0.2.0",
            "iii-sdk": "0.23.1-rc.6",
            "lucide-react": "1.16.0",
            "react": "19.2.8"
        },
        "devDependencies": {
            "@types/react": "19.2.14",
            "esbuild": "0.28.2",
            "typescript": "6.0.3"
        }
    }))
    .expect("static package JSON serializes")
}

fn package_lock() -> Result<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ade-incident-board/package-lock.json");
    Ok(fs::read(path)?)
}

fn baseline_files(contract: &WorkerContract) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::from([
        ("README.md".into(), readme(contract).into_bytes()),
        ("package.json".into(), package_json()),
        ("package-lock.json".into(), package_lock()?),
        (
            "worker-compose.yaml".into(),
            compose_file(contract).into_bytes(),
        ),
        ("src/index.mjs".into(), worker_source(contract).into_bytes()),
        (
            "ui/page.tsx".into(),
            b"// Implement the planned Console UI.\n".to_vec(),
        ),
        (
            "ui/build.mjs".into(),
            ui_build_script(contract).into_bytes(),
        ),
        ("ui/tsconfig.json".into(), ui_tsconfig().into_bytes()),
        (
            "ui/styles.css".into(),
            b"/* Implement scoped Console styles. */\n".to_vec(),
        ),
        ("ui/design-plan.md".into(), Vec::new()),
    ]);
    Ok(std::mem::take(&mut files))
}

fn ui_build_script(contract: &WorkerContract) -> String {
    format!(
        "import {{ buildWorkerUi }} from '@iii-dev/console-ui/build-worker-ui'\n\nawait buildWorkerUi({{ scope: '{}', root: import.meta.dirname, outdir: '../dist/ui', lint: {{ strict: true }} }})\n",
        contract.worker
    )
}

fn ui_tsconfig() -> String {
    "{\n  \"extends\": \"@iii-dev/console-ui/tsconfig.worker-ui.json\",\n  \"include\": [\"page.tsx\"]\n}\n".into()
}

async fn prepare_workspace(run_id: &str) -> Result<()> {
    let contract = worker_contract(run_id);
    let root = workspace_root(run_id);
    remove_workspace(&root)?;
    for (relative, bytes) in baseline_files(&contract)? {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    let mut install = tokio::process::Command::new("npm");
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
        .context("materialize pinned iii-sdk timed out after 120 seconds")??;
    if !output.status.success() {
        bail!(
            "could not materialize pinned iii-sdk: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    dependency_fingerprints()
        .lock()
        .map_err(|_| anyhow::anyhow!("dependency fingerprint state is poisoned"))?
        .insert(
            run_id.to_string(),
            tree_fingerprint(&root.join("node_modules"))?,
        );
    Ok(())
}

async fn build_candidate_ui(root: &Path) -> Result<Value> {
    let build = run_npm(root, &["run", "build:ui"]).await?;
    let typecheck = run_npm(root, &["run", "typecheck"]).await?;
    let passed = build["passed"] == true && typecheck["passed"] == true;
    Ok(json!({
        "passed": passed,
        "status": "checked",
        "reason": if passed { "The UI built and typechecked." } else { "The UI build, strict design lint, or TypeScript typecheck failed." },
        "build": build,
        "typecheck": typecheck,
    }))
}

async fn run_npm(root: &Path, args: &[&str]) -> Result<Value> {
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new("npm")
            .args(args)
            .current_dir(root)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("pinned UI validation timed out after 120 seconds")??;
    let mut log = String::from_utf8_lossy(&output.stdout).into_owned();
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    if log.len() > 12_000 {
        log.truncate(12_000);
    }
    Ok(json!({"passed":output.status.success(),"status_code":output.status.code(),"log":log}))
}

fn workspace_root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::canonicalize(&base)
        .unwrap_or(base)
        .join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

fn remove_workspace(root: &Path) -> Result<()> {
    let parent = root.parent().context("workspace path has no parent")?;
    let name = root
        .file_name()
        .and_then(OsStr::to_str)
        .context("workspace path has no name")?;
    if parent.file_name() != Some(OsStr::new("scenario-workspaces"))
        || !name.starts_with(&format!("{ID}-"))
    {
        bail!("refusing to remove workspace outside the scenario-owned base");
    }
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn evaluate_candidate(context: &E2eContext, run_id: &str) -> Result<Value> {
    let contract = worker_contract(run_id);
    let root = workspace_root(run_id);
    let mut checks = serde_json::Map::new();
    let baseline = baseline_files(&contract)?;
    let scope_ok = scope_integrity(run_id, &root, &baseline);
    checks.insert(
        "scope_integrity".into(),
        check(
            scope_ok,
            "Only the three assigned UI files changed; the backend, Compose, and dependencies remain frozen.",
            "The candidate changed or added a protected workspace file.",
        ),
    );

    let plan = fs::read_to_string(root.join("ui/design-plan.md")).unwrap_or_default();
    let plan_content_ok = design_plan_ok(&plan);
    checks.insert(
        "design_plan".into(),
        json!({
            "passed": plan_content_ok,
            "reason": if plan_content_ok { "The plan covers the required decisions and has a Verify: step beside AC-01 through AC-05." } else { "The plan is missing a required topic or a Verify: step beside AC-01 through AC-05." },
            "plan_content_valid": plan_content_ok,
            "plan_sha256": crate::artifact::sha256_bytes(plan.as_bytes()),
        }),
    );

    let build = if scope_ok {
        build_candidate_ui(&root).await?
    } else {
        json!({"passed":false,"status":"checked","reason":"The candidate changed protected build inputs or installed packages; the runner skipped executing them.","build":null,"typecheck":null})
    };
    checks.insert(
        "ui_build".into(),
        json!({
            "passed": build["passed"],
            "status": build["status"],
            "reason": if build["passed"] == true { "The shared pinned build driver and strict Console UI lint passed, and TypeScript typecheck is clean." } else { build["reason"].as_str().unwrap_or("The shared UI build or typecheck did not pass.") },
            "build": build["build"],
            "typecheck": build["typecheck"],
        }),
    );

    let compose = root.join("worker-compose.yaml");
    let compose_validation = if build["passed"] == true {
        context
            .trigger_value("compose::validate", json!({"file": compose}))
            .await
    } else {
        Err(anyhow::anyhow!("candidate UI build did not pass"))
    };
    if build["passed"] == true {
        ensure_remote_or_success(&compose_validation, "validate frozen incident Compose")?;
    }
    let compose_valid = scope_ok
        && build["passed"] == true
        && compose_validation.is_ok()
        && fs::read_to_string(&compose).ok().as_deref() == Some(compose_file(&contract).as_str());
    let up = if compose_valid {
        context
            .trigger_value(
                "compose::up",
                json!({"file": compose, "container": contract.worker}),
            )
            .await
    } else {
        Err(anyhow::anyhow!("frozen Compose contract did not validate"))
    };
    if compose_valid {
        ensure_remote_or_success(&up, "start incident Worker")?;
    }
    let mut ready = false;
    let mut last_status = Value::Null;
    if up.is_ok() {
        for _ in 0..120 {
            match context
                .trigger_value("compose::status", json!({"file": compose}))
                .await
            {
                Ok(status) => {
                    ready = status["containers"].as_array().is_some_and(|containers| {
                        containers.iter().any(|container| {
                            container["container"] == contract.worker
                                && container["state"] == "ready"
                        })
                    });
                    last_status = status;
                    if ready {
                        break;
                    }
                }
                Err(error) if is_remote_failure(&error) => {
                    last_status = json!({"error": format!("{error:#}")})
                }
                Err(error) => return Err(error.context("query incident Worker status")),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let (surface_ok, observed_functions) = if ready {
        let ids = contract.functions.values().cloned().collect::<Vec<_>>();
        let info = context
            .trigger_value("engine::functions::info", json!({"function_ids": ids}))
            .await;
        ensure_remote_or_success(&info, "inspect incident Worker functions")?;
        let ok = info
            .as_ref()
            .ok()
            .is_some_and(|value| function_surface_ok(value, &contract));
        (ok, result_value(info))
    } else {
        (false, Value::Null)
    };
    let runtime_ok = compose_valid && ready && surface_ok;
    let runtime_blocked = build["passed"] == true && (!compose_valid || !ready || !surface_ok);
    let runtime_reason = if !scope_ok {
        "Runner skipped startup because a protected workspace file or dependency changed."
    } else if build["passed"] != true {
        "Runner skipped startup because the submitted UI did not build."
    } else if runtime_ok {
        "The protected Compose project is ready and all documented functions are registered."
    } else {
        "Compose, the Worker, or its required function surface was unavailable after a valid UI build."
    };
    let runtime_prerequisite = json!({
        "status": if runtime_blocked { "blocked" } else { "checked" },
        "passed": runtime_ok,
        "reason": runtime_reason,
        "compose_valid": compose_valid,
        "worker_ready": ready,
        "function_surface": surface_ok,
        "observed": {"compose": result_value(compose_validation), "up": result_value(up), "status": last_status, "functions": observed_functions}
    });

    if runtime_ok {
        let list = invoke(context.client(), &contract.functions["list"], json!({})).await;
        ensure_remote_or_success(&list, "probe incident list function")?;
        let seeded = list
            .as_ref()
            .ok()
            .and_then(|value| value["incidents"].as_array())
            .is_some_and(|incidents| incidents.len() == 5);
        checks.insert(
            "function_fixture".into(),
            check(
                seeded,
                "The frozen incident API returned all five deterministic records.",
                "The incident API did not return the five expected records.",
            ),
        );
    }

    let script = if runtime_ok {
        invoke(
            context.client(),
            &contract.functions["ui-content"],
            json!({"path": format!("{}/page.js", contract.worker)}),
        )
        .await
    } else {
        Err(anyhow::anyhow!("UI runtime is unavailable"))
    };
    let style = if runtime_ok {
        invoke(
            context.client(),
            &contract.functions["ui-content"],
            json!({"path": format!("{}/styles.css", contract.worker)}),
        )
        .await
    } else {
        Err(anyhow::anyhow!("UI runtime is unavailable"))
    };
    if runtime_ok {
        ensure_remote_or_success(&script, "read incident Console page asset")?;
        ensure_remote_or_success(&style, "read incident Console style asset")?;
    }
    let console_status = if build["passed"] == true {
        context.trigger_value("console::status", json!({})).await
    } else {
        Err(anyhow::anyhow!("candidate UI build did not pass"))
    };
    if build["passed"] == true {
        ensure_remote_or_success(&console_status, "inspect Console status")?;
    }
    let console_port = console_status
        .as_ref()
        .ok()
        .and_then(|value| value["http_port"].as_u64())
        .and_then(|port| u16::try_from(port).ok());
    let mut manifest = Value::Null;
    if runtime_ok && console_port.is_some() {
        for _ in 0..40 {
            let result = context
                .trigger_value("console::ui-manifest", json!({}))
                .await;
            ensure_remote_or_success(&result, "inspect Console UI manifest")?;
            manifest = result.unwrap_or(Value::Null);
            if console_assets_ok(&manifest, &contract) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    let assets_ok = console_assets_ok(&manifest, &contract);
    let content_ok = ui_content_ok(&script, &style, &contract);
    let console_ok = console_port.is_some() && assets_ok && content_ok;
    checks.insert(
        "console_delivery".into(),
        json!({
            "passed": console_ok,
            "reason": if console_ok { "The Console registered the page script and scoped stylesheet without warnings." } else { "The Console did not register both submitted assets without warnings." },
            "status": if runtime_blocked || (build["passed"] == true && console_port.is_none()) { "blocked" } else { "checked" },
            "console_status": result_value(console_status),
            "manifest": bounded_value(manifest.clone()),
            "script": result_value(script),
            "style": result_value(style),
        }),
    );

    let mut browser = match (build["passed"] == true && runtime_ok, console_port) {
        (true, Some(port)) => capture_browser(context, &root, &contract, port).await?,
        _ => {
            json!({"status":if runtime_blocked || (build["passed"] == true && console_port.is_none()) {"blocked"} else {"checked"},"reason":if !scope_ok {"Candidate changed protected files; browser probes were skipped."} else if build["passed"] != true {"Candidate UI did not build; browser probes were skipped."} else {"Worker or Console prerequisites were unavailable; browser probes were not run."},"checks":{},"captures":[]})
        }
    };
    for id in [
        "board_search",
        "detail_and_persistence",
        "external_live_update",
        "empty_and_error_recovery",
        "responsive_theme_keyboard",
    ] {
        let check_value = browser["checks"][id].clone();
        checks.insert(
            id.into(),
            if check_value.is_null() {
                json!({"passed":false,"status":browser["status"],"reason":browser["reason"]})
            } else {
                check_value
            },
        );
    }
    let captures = browser["captures"].as_array();
    let valid_screenshot = |capture: &Value, expected: &str| {
        if capture["file"].as_str() != Some(expected) || capture["oversized"] == true {
            return false;
        }
        let Some(data) = capture["data"].as_str() else {
            return false;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return false;
        };
        let hash = crate::artifact::sha256_bytes(&bytes);
        bytes.len() <= MAX_SCREENSHOT_BYTES
            && capture["size_bytes"].as_u64() == Some(bytes.len() as u64)
            && capture["sha256"].as_str() == Some(hash.as_str())
    };
    let visual_evidence_ok = [
        "screenshots/board-light.png",
        "screenshots/mobile-board.png",
        "screenshots/detail-live-update.png",
        "screenshots/detail-phone.png",
    ]
    .iter()
    .all(|expected| {
        captures.is_some_and(|captures| {
            captures
                .iter()
                .any(|capture| valid_screenshot(capture, expected))
        })
    });
    checks.insert(
        "visual_evidence".into(),
        json!({
            "passed":visual_evidence_ok,
            "status":browser["status"],
            "reason":if visual_evidence_ok { "Four Console screenshots were captured within the size limit and hash-verified." } else { "One or more required Console screenshots could not be captured and hash-verified." },
            "captures":captures.map(|items|items.iter().map(|capture|json!({"file":capture["file"],"sha256":capture["sha256"],"size_bytes":capture["size_bytes"],"oversized":capture["oversized"]})).collect::<Vec<_>>()).unwrap_or_default(),
        }),
    );

    let identity = json!({
        "worker": contract.worker,
        "functions": contract.functions,
        "pages": {"board": contract.board_page, "detail": contract.detail_page},
        "source_sha256": source_sha256(&root).ok(),
        "compose_sha256": fs::read(&compose).ok().map(|bytes| crate::artifact::sha256_bytes(&bytes)),
        "console_port": console_port,
    });
    let mut files = serde_json::Map::new();
    for (name, path) in [
        ("ui/design-plan.md", "plan/design-plan.md"),
        ("ui/page.tsx", "source/page.tsx"),
        ("ui/styles.css", "source/styles.css"),
    ] {
        if let Ok(contents) = fs::read_to_string(root.join(name)) {
            insert_text_file(&mut files, path, &contents);
        }
    }
    for capture in browser["captures"].as_array().into_iter().flatten() {
        if let (Some(name), Some(data)) = (capture["file"].as_str(), capture["data"].as_str()) {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
            insert_binary_file(&mut files, name, &bytes);
        }
    }
    browser["captures"] = browser["captures"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|capture| {
            let mut compact = capture.clone();
            compact.as_object_mut().map(|fields| fields.remove("data"));
            compact
        })
        .collect();
    Ok(json!({
        "runtime_prerequisite": runtime_prerequisite,
        "identity": identity,
        "viewport": {"wide": {"width": 1920, "height": 1080}, "phone": {"width": 390, "height": 844}},
        "checks": checks,
        "browser": browser,
        "files": files,
    }))
}

fn scope_integrity(run_id: &str, root: &Path, baseline: &BTreeMap<String, Vec<u8>>) -> bool {
    let allowed = ["ui/page.tsx", "ui/styles.css", "ui/design-plan.md"];
    let dependencies_unchanged = dependency_fingerprints()
        .lock()
        .ok()
        .and_then(|fingerprints| fingerprints.get(run_id).cloned())
        .zip(tree_fingerprint(&root.join("node_modules")).ok())
        .is_some_and(|(expected, actual)| expected == actual);
    let mut seen = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                return false;
            };
            if kind.is_symlink() {
                return false;
            }
            if kind.is_dir() {
                if matches!(entry.file_name().to_str(), Some("node_modules" | "dist")) {
                    continue;
                }
                stack.push(path);
            } else if kind.is_file() {
                let Ok(relative) = path.strip_prefix(root) else {
                    return false;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !baseline.contains_key(&relative) {
                    return false;
                }
                seen.push(relative);
            }
        }
    }
    seen.sort();
    seen.dedup();
    dependencies_unchanged
        && baseline.iter().all(|(relative, expected)| {
            if allowed.contains(&relative.as_str()) {
                true
            } else {
                seen.binary_search(relative).is_ok()
                    && fs::read(root.join(relative)).ok().as_deref() == Some(expected.as_slice())
            }
        })
}

fn dependency_fingerprints() -> &'static Mutex<BTreeMap<String, String>> {
    DEPENDENCY_FINGERPRINTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn tree_fingerprint(root: &Path) -> Result<String> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let kind = entry.file_type()?;
            let value = if kind.is_symlink() {
                format!("link:{}", fs::read_link(&path)?.to_string_lossy())
            } else if kind.is_dir() {
                stack.push(path);
                continue;
            } else if kind.is_file() {
                format!("file:{}", crate::artifact::sha256_bytes(&fs::read(&path)?))
            } else {
                return Err(anyhow::anyhow!(
                    "unsupported node_modules entry: {}",
                    path.display()
                ));
            };
            entries.push(format!("{relative}\n{value}\n"));
        }
    }
    entries.sort_unstable();
    Ok(crate::artifact::sha256_bytes(entries.concat().as_bytes()))
}

fn design_plan_ok(plan: &str) -> bool {
    let lower = plan.to_ascii_lowercase();
    let lines = plan.lines().collect::<Vec<_>>();
    let criteria_verified = (1..=5).all(|number| {
        let marker = format!("ac-{number:02}");
        lines.iter().enumerate().any(|(index, line)| {
            line.to_ascii_lowercase().contains(&marker)
                && lines[index..(index + 3).min(lines.len())]
                    .iter()
                    .any(|line| line.to_ascii_lowercase().contains("verify:"))
        })
    });
    lower.contains("responder")
        && lower.contains("problem")
        && lower.contains("outcome")
        && lower.contains("pane")
        && lower.contains("loading")
        && lower.contains("empty")
        && lower.contains("error")
        && lower.contains("success")
        && lower.contains("overflow")
        && lower.contains("phone")
        && (lower.contains("wide") || lower.contains("desktop"))
        && (lower.contains("narrow pane") || lower.contains("narrow split"))
        && (lower.contains("tab") || lower.contains("tabs"))
        && (lower.contains("console-ui") || lower.contains("shared component"))
        && lower.contains("token")
        && lower.contains("live update")
        && lower.contains("light")
        && lower.contains("dark")
        && lower.contains("keyboard")
        && lower.contains("focus")
        && criteria_verified
}

fn function_surface_ok(info: &Value, contract: &WorkerContract) -> bool {
    let Some(functions) = info["functions"].as_array() else {
        return false;
    };
    contract.functions.iter().all(|(operation, id)| {
        functions.iter().any(|function| {
            function["function_id"] == id.as_str()
                && function["description"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
                && function["request_schema"]["type"] == "object"
                && function["response_schema"]["type"] == "object"
                && match *operation {
                    "list" => {
                        function["response_schema"]["properties"]["incidents"]["type"] == "array"
                    }
                    "get" | "update" => {
                        function["response_schema"]["properties"]["incident"]["type"] == "object"
                    }
                    "fixture" => function["request_schema"]["required"]
                        .as_array()
                        .is_some_and(|values| values.iter().any(|value| value == "action")),
                    "ui-content" => {
                        function["request_schema"]["properties"]["path"]["type"] == "string"
                            && function["response_schema"]["properties"]["content"]["type"]
                                == "string"
                    }
                    _ => false,
                }
        })
    })
}

fn console_assets_ok(manifest: &Value, contract: &WorkerContract) -> bool {
    let expected = [
        (format!("{}/page.js", contract.worker), "script"),
        (format!("{}/styles.css", contract.worker), "style"),
    ];
    let Some(assets) = manifest["assets"].as_array() else {
        return false;
    };
    let worker_assets = assets
        .iter()
        .filter(|asset| {
            asset["path"]
                .as_str()
                .is_some_and(|path| path.starts_with(&format!("{}/", contract.worker)))
        })
        .collect::<Vec<_>>();
    manifest["disabled"] == false
        && worker_assets.len() == expected.len()
        && expected.iter().all(|(path, kind)| {
            worker_assets.iter().any(|asset| {
                asset["path"] == *path
                    && asset["kind"] == *kind
                    && asset["hash"].as_str().is_some_and(|hash| !hash.is_empty())
                    && asset["warnings"].as_array().is_some_and(Vec::is_empty)
            })
        })
        && manifest["workers"].as_array().is_some_and(|workers| {
            workers.iter().any(|worker| {
                worker["worker"] == contract.worker
                    && worker["enabled"] == true
                    && worker["assets"] == expected.len()
            })
        })
}

fn ui_content_ok(script: &Result<Value>, style: &Result<Value>, contract: &WorkerContract) -> bool {
    let script = script
        .as_ref()
        .ok()
        .and_then(|value| value["content"].as_str());
    let style = style
        .as_ref()
        .ok()
        .and_then(|value| value["content"].as_str());
    script.is_some_and(|content| {
        !content.trim().is_empty()
            && content.contains(&contract.board_page)
            && content.contains(&contract.detail_page)
            && !content.contains(&contract.functions["fixture"])
    }) && style.is_some_and(|content| {
        content.contains(&format!("[data-iii-ui=\"{}\"]", contract.worker))
            || content.contains(&format!("[data-iii-ui='{}']", contract.worker))
            || content.contains(&format!("[data-iii-ui={}]", contract.worker))
    })
}

fn check(passed: bool, success: &str, failure: &str) -> Value {
    json!({"passed":passed,"reason":if passed {success} else {failure}})
}

fn result_value(result: Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"value":bounded_value(value)}),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    }
}

fn bounded_value(value: Value) -> Value {
    let mut bytes = serde_json::to_vec(&value).unwrap_or_default();
    if bytes.len() > 32_768 {
        bytes.truncate(32_768);
        json!({"truncated":true,"prefix":String::from_utf8_lossy(&bytes)})
    } else {
        value
    }
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

fn is_remote_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Remote { .. })
    )
}

fn ensure_remote_or_success(result: &Result<Value>, action: &str) -> Result<()> {
    if let Err(error) = result {
        if !is_remote_failure(error) {
            bail!("{action} infrastructure failure: {error:#}");
        }
    }
    Ok(())
}

fn source_sha256(root: &Path) -> Result<String> {
    let mut content = Vec::new();
    for relative in ["ui/page.tsx", "ui/styles.css", "ui/design-plan.md"] {
        let bytes = fs::read(root.join(relative))?;
        content.extend_from_slice(relative.as_bytes());
        content.push(b'\n');
        content.extend_from_slice(
            crate::artifact::sha256_bytes(&bytes)
                .trim_start_matches("sha256:")
                .as_bytes(),
        );
        content.push(b'\n');
    }
    Ok(crate::artifact::sha256_bytes(&content))
}

fn insert_text_file(files: &mut serde_json::Map<String, Value>, name: &str, text: &str) {
    let bytes = text.as_bytes();
    files.insert(name.into(), json!({"encoding":"utf8","content":text,"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(bytes)}));
}

fn insert_binary_file(files: &mut serde_json::Map<String, Value>, name: &str, bytes: &[u8]) {
    files.insert(name.into(), json!({"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(bytes),"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(bytes)}));
}

fn workspace_has_pair(workspace: &Value, board: &str, detail: &str) -> bool {
    let board = format!("ext:{board}");
    let detail = format!("ext:{detail}");
    workspace["tabs"].as_array().is_some_and(|tabs| {
        tabs.iter().any(|tab| {
            let screens = tab["screens"].as_array();
            tab["columns"].as_u64().is_some_and(|columns| columns >= 2)
                && screens.is_some_and(|screens| {
                    screens
                        .iter()
                        .any(|screen| screen.as_str() == Some(board.as_str()))
                        && screens
                            .iter()
                            .any(|screen| screen.as_str() == Some(detail.as_str()))
                })
        })
    })
}

async fn capture_browser(
    context: &E2eContext,
    root: &Path,
    contract: &WorkerContract,
    port: u16,
) -> Result<Value> {
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
    let result = capture_browser_session(context, root, &session, &url, contract).await;
    let stop = context
        .trigger_value("browser::sessions::stop", json!({"session_id":session}))
        .await;
    stop.context("stop incident board evidence browser session")?;
    result
}

async fn capture_browser_session(
    context: &E2eContext,
    root: &Path,
    session: &str,
    url: &str,
    contract: &WorkerContract,
) -> Result<Value> {
    context
        .trigger_value(
            "console::workspace::open",
            json!({"screen":"chat","activate":true}),
        )
        .await
        .context("activate the Console chat before browser baseline")?;
    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":1920,"height":1080}),
        )
        .await?;
    let baseline = context
        .trigger_value(
            "browser::navigate",
            json!({"session_id":session,"url":url,"timeout_ms":30000}),
        )
        .await?;
    if baseline["ok"] != true || baseline["timed_out"] == true {
        return Ok(browser_failure(
            "The Console/browser baseline was unavailable before opening the candidate page.",
            baseline,
            "blocked",
        ));
    }
    context
        .trigger_value(
            "console::workspace::open",
            json!({"screen":format!("ext:{}", contract.board_page),"activate":true}),
        )
        .await
        .context("open the incident board in the real Console workspace")?;
    let navigation = context
        .trigger_value(
            "browser::navigate",
            json!({"session_id":session,"url":url,"timeout_ms":30000}),
        )
        .await?;
    if navigation["ok"] != true || navigation["timed_out"] == true {
        return Ok(browser_failure(
            "The registered incident board page did not render in Console.",
            navigation,
            "checked",
        ));
    }
    let ready = execute(
        context,
        session,
        r#"return await (async()=>{for(let i=0;i<240;i++){if(document.querySelector('[data-testid="incident-board"]'))return true;await new Promise(r=>setTimeout(r,50));}return false;})();"#,
    )
    .await
    .unwrap_or(Value::Bool(false));
    if ready != true {
        return Ok(browser_failure(
            "The board did not expose its semantic root in Console.",
            navigation,
            "checked",
        ));
    }

    execute(context, session, r#"document.documentElement.setAttribute('data-theme','light');return await new Promise(resolve=>requestAnimationFrame(()=>resolve(true)));"#).await?;
    let board_probe = execute(
        context,
        session,
        r#"return (()=>{const root=document.querySelector('[data-testid="incident-board"]');const lanes=[...root.querySelectorAll('[data-status-lane]')];const laneData=Object.fromEntries(lanes.map(lane=>[lane.dataset.statusLane,{ids:[...lane.querySelectorAll('[data-incident-id]')].map(card=>card.dataset.incidentId).sort(),count:Number(lane.querySelector('[data-testid="lane-count"]')?.textContent?.trim()),left:lane.getBoundingClientRect().left,top:lane.getBoundingClientRect().top}]));const cards=[...root.querySelectorAll('[data-incident-id]')];const search=root.querySelector('[aria-label="Search incidents"]');return {lane_statuses:lanes.map(lane=>lane.dataset.statusLane),lane_data:laneData,card_ids:cards.map(card=>card.dataset.incidentId),search_tag:search?.tagName,visible_lanes:lanes.filter(l=>l.getClientRects().length>0).length,board_rect:{width:root.getBoundingClientRect().width,height:root.getBoundingClientRect().height},scroll_width:document.documentElement.scrollWidth,viewport:innerWidth,background:getComputedStyle(root).backgroundColor,color:getComputedStyle(root).color};})();"#,
    )
    .await?;
    execute(context, session, r#"document.documentElement.setAttribute('data-theme','dark');return await new Promise(resolve=>setTimeout(()=>resolve(true),150));"#).await?;
    let dark_board = execute(context, session, r#"const e=document.querySelector('[data-testid="incident-board"]');return {background:getComputedStyle(e).backgroundColor,color:getComputedStyle(e).color};"#).await?;
    execute(context, session, r#"document.documentElement.setAttribute('data-theme','light');return await new Promise(resolve=>requestAnimationFrame(()=>resolve(true)));"#).await?;
    let seeded_board_ok = board_probe["lane_statuses"]
        .as_array()
        .is_some_and(|values| {
            ["new", "investigating", "monitoring", "resolved"]
                .iter()
                .all(|status| values.iter().any(|value| value == status))
        })
        && board_probe["lane_data"]["new"]["ids"] == json!(["INC-105", "INC-108"])
        && board_probe["lane_data"]["investigating"]["ids"] == json!(["INC-104"])
        && board_probe["lane_data"]["monitoring"]["ids"] == json!(["INC-106"])
        && board_probe["lane_data"]["resolved"]["ids"] == json!(["INC-107"])
        && board_probe["lane_data"].as_object().is_some_and(|lanes| {
            ["new", "investigating", "monitoring", "resolved"]
                .iter()
                .all(|status| {
                    lanes[*status]["ids"].as_array().map(Vec::len)
                        == lanes[*status]["count"].as_u64().map(|count| count as usize)
                })
        })
        && board_probe["card_ids"].as_array().is_some_and(|cards| {
            cards.len() == 5
                && ["INC-104", "INC-105", "INC-106", "INC-107", "INC-108"]
                    .iter()
                    .all(|id| cards.iter().any(|card| card == id))
        });
    let wide_lanes_ok = board_probe["visible_lanes"] == 4
        && ["new", "investigating", "monitoring", "resolved"]
            .windows(2)
            .all(|pair| {
                let left = &board_probe["lane_data"][pair[0]];
                let right = &board_probe["lane_data"][pair[1]];
                left["left"]
                    .as_f64()
                    .zip(right["left"].as_f64())
                    .is_some_and(|(a, b)| a < b)
                    && left["top"]
                        .as_f64()
                        .zip(right["top"].as_f64())
                        .is_some_and(|(a, b)| (a - b).abs() <= 8.0)
            })
        && board_probe["board_rect"]["width"]
            .as_f64()
            .is_some_and(|width| width > 700.0);
    let search_probe = execute(
        context,
        session,
        r#"return await (async()=>{const input=document.querySelector('[aria-label="Search incidents"]');if(!input)return {ok:false};const setter=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;setter.call(input,'cache');input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));for(let i=0;i<80;i++){const ids=[...document.querySelectorAll('[data-testid="incident-board"] [data-incident-id]')].map(e=>e.dataset.incidentId);if(ids.length===1){const lanes=[...document.querySelectorAll('[data-status-lane]')];return {ok:ids[0]==='INC-105',ids,counts:Object.fromEntries(lanes.map(l=>[l.dataset.statusLane,Number(l.querySelector('[data-testid="lane-count"]')?.textContent?.trim())]))};}await new Promise(r=>setTimeout(r,50));}return {ok:false,ids:[...document.querySelectorAll('[data-testid="incident-board"] [data-incident-id]')].map(e=>e.dataset.incidentId)};})();"#,
    )
    .await?;
    execute(context, session, r#"const input=document.querySelector('[aria-label="Search incidents"]');if(input){const setter=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;setter.call(input,'');input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));}return true;"#).await?;
    let filtered_counts_ok = search_probe["counts"]["new"] == 1
        && search_probe["counts"]["investigating"] == 0
        && search_probe["counts"]["monitoring"] == 0
        && search_probe["counts"]["resolved"] == 0;

    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":390,"height":844}),
        )
        .await?;
    let mobile_probe = execute(
        context,
        session,
        r#"return (()=>{const root=document.querySelector('[data-testid="incident-board"]');const rect=root?.getBoundingClientRect();const card=root?.querySelector('[data-incident-id]');const lanes=[...root?.querySelectorAll('[data-status-lane]')||[]];const visible=lanes.filter(l=>l.getClientRects().length>0);return {root_width:rect?.width,root_left:rect?.left,root_right:rect?.right,scroll_width:document.documentElement.scrollWidth,viewport:innerWidth,card_role:card?.getAttribute('role'),card_tab_index:card?.tabIndex,visible_lanes:visible.map(l=>l.dataset.statusLane),tabs:root?.querySelectorAll('[role="tab"]').length,search_visible:!!root?.querySelector('[aria-label="Search incidents"]')};})();"#,
    )
    .await?;
    let mobile_ok = mobile_probe["root_width"]
        .as_f64()
        .is_some_and(|width| width > 250.0 && width <= 390.0)
        && mobile_probe["root_left"]
            .as_f64()
            .is_some_and(|left| left >= -4.0)
        && mobile_probe["root_right"]
            .as_f64()
            .is_some_and(|right| right <= 394.0)
        && mobile_probe["scroll_width"]
            .as_u64()
            .is_some_and(|width| width <= 394)
        && mobile_probe["visible_lanes"]
            .as_array()
            .is_some_and(|lanes| lanes.len() == 1)
        && mobile_probe["tabs"].as_u64().is_some_and(|tabs| tabs >= 4)
        && (mobile_probe["card_role"] == "button" || mobile_probe["card_role"].is_null())
        && mobile_probe["card_tab_index"]
            .as_i64()
            .is_some_and(|value| value >= 0)
        && mobile_probe["search_visible"] == true;
    let phone_tabs_switch = execute(context, session, TAB_SWITCH_PROBE).await? == true;
    let mobile_screenshot =
        screenshot_png(context, session, "screenshots/mobile-board.png").await?;

    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":1920,"height":1080}),
        )
        .await?;
    execute(context, session, r#"document.documentElement.setAttribute('data-theme','light');return await new Promise(resolve=>requestAnimationFrame(()=>resolve(true)));"#).await?;
    let board_light = screenshot_png(context, session, "screenshots/board-light.png").await?;
    let card_focus = execute(
        context,
        session,
        r#"const card=document.querySelector('[data-testid="incident-board"] [data-incident-id="INC-104"]');if(!card)return false;window.__incidentFocusRing=false;document.addEventListener('keydown',event=>{if(event.key==='Enter'){const style=getComputedStyle(document.activeElement);window.__incidentFocusRing=style.outlineStyle!=='none'&&parseFloat(style.outlineWidth)>0||style.boxShadow!=='none';}},{once:true,capture:true});card.focus();return {tab_index:card.tabIndex,role:card.getAttribute('role'),tag:card.tagName};"#,
    )
    .await?;
    let keyboard_open = context
        .trigger_value(
            "browser::act",
            json!({"session_id":session,"action":"press","key":"Enter"}),
        )
        .await?;
    let focus_ring = execute(
        context,
        session,
        "return window.__incidentFocusRing === true;",
    )
    .await?;
    let card_open = card_focus["tab_index"]
        .as_i64()
        .is_some_and(|value| value >= 0)
        && (card_focus["tag"] == "BUTTON" || card_focus["role"] == "button")
        && keyboard_open["ok"] == true;
    let detail_ready = execute(
        context,
        session,
        r#"return await (async()=>{for(let i=0;i<160;i++){if(document.querySelector('[data-testid="incident-detail"][data-incident-id="INC-104"]'))return true;await new Promise(r=>setTimeout(r,50));}return false;})();"#,
    )
    .await?;
    let workspace = context
        .trigger_value("console::workspace::list", json!({}))
        .await;
    ensure_remote_or_success(&workspace, "verify incident pages in the Console workspace")?;
    let workspace = workspace.unwrap_or(Value::Null);
    let two_panes = workspace_has_pair(&workspace, &contract.board_page, &contract.detail_page);
    let board_screen = format!("ext:{}", contract.board_page);
    let board_width_before_split = execute(
        context,
        session,
        "return document.querySelector('[data-testid=\"incident-board\"]')?.getBoundingClientRect().width ?? null;",
    )
    .await?
    .as_f64();
    let split_sizes = workspace["tabs"].as_array().and_then(|tabs| {
        tabs.iter().find_map(|tab| {
            let screens = tab["screens"].as_array()?;
            let board_index = screens.iter().position(|screen| screen == &board_screen)?;
            let original = tab["sizes"].as_array()?;
            if screens.len() < 2 || original.len() != screens.len() {
                return None;
            }
            let narrow = (0..screens.len())
                .map(|index| {
                    if index == board_index {
                        0.2
                    } else {
                        0.8 / (screens.len() - 1) as f64
                    }
                })
                .collect::<Vec<_>>();
            Some((narrow, original.clone()))
        })
    });
    let narrow_split_probe = if let (Some(width_before), Some((narrow, original))) =
        (board_width_before_split, split_sizes)
    {
        context
            .trigger_value(
                "console::workspace::open",
                json!({"screen":board_screen,"sizes":narrow,"activate":true}),
            )
            .await?;
        let geometry = execute(
            context,
            session,
            &format!(
                r#"const before={width_before};return await (async()=>{{for(let i=0;i<160;i++){{const board=document.querySelector('[data-testid="incident-board"]');const lanes=[...board?.querySelectorAll('[data-status-lane]')||[]];const visible=lanes.filter(lane=>lane.getClientRects().length>0);const width=board?.getBoundingClientRect().width;const tabs=board?.querySelectorAll('[role="tab"]').length;if(width>250&&width<600&&(before<=600||width<before-80)&&visible.length===1&&tabs>=4)return {{ok:true,width,visible_lane:visible[0].dataset.statusLane,tabs}};await new Promise(resolve=>setTimeout(resolve,50));}}return {{ok:false}};}})();"#
            ),
        )
        .await?;
        let tabs_switch =
            geometry["ok"] == true && execute(context, session, TAB_SWITCH_PROBE).await? == true;
        context
            .trigger_value(
                "console::workspace::open",
                json!({"screen":board_screen,"sizes":original,"activate":true}),
            )
            .await?;
        let restored = execute(
            context,
            session,
            &format!(r#"const before={width_before};return await (async()=>{{for(let i=0;i<160;i++){{const width=document.querySelector('[data-testid="incident-board"]')?.getBoundingClientRect().width;if(typeof width==='number'&&Math.abs(width-before)<12)return true;await new Promise(resolve=>setTimeout(resolve,50));}}return false;}})();"#),
        )
        .await? == true;
        if !restored {
            bail!("Console workspace did not restore the incident board pane width");
        }
        json!({"ok":tabs_switch,"geometry":geometry,"tabs_switch":tabs_switch,"restored":restored})
    } else {
        Value::Null
    };
    let status_probe = if detail_ready == true {
        execute(
            context,
            session,
            r#"return await (async()=>{const status=document.querySelector('[data-testid="incident-detail"] [aria-label="Status"]');if(!status)return {ok:false,reason:'missing status control'};status.focus();let changed=false;if(status.tagName==='SELECT'){Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(status,'monitoring');status.dispatchEvent(new Event('input',{bubbles:true}));status.dispatchEvent(new Event('change',{bubbles:true}));changed=true;}else{status.click();for(let i=0;i<40;i++){const option=[...document.querySelectorAll('[role="option"]')].find(item=>item.getClientRects().length>0&&item.innerText.trim().toLowerCase()==='monitoring');if(option){option.click();changed=true;break;}await new Promise(r=>setTimeout(r,50));}}if(!changed)return {ok:false,reason:'monitoring option was unavailable'};status.blur();await new Promise(r=>setTimeout(r,500));const detail=document.querySelector('[data-testid="incident-detail"]');const save=[...detail.querySelectorAll('button,[role="button"]')].some(button=>button.getClientRects().length>0&&/\bsave\b/i.test((button.getAttribute('aria-label')||button.innerText).trim()));return {ok:true,global_save_present:save,control_value:status.value||status.innerText};})();"#,
        )
        .await?
    } else {
        Value::Null
    };
    let mut status_persisted = false;
    let mut status_backend = Value::Null;
    if status_probe["ok"] == true && status_probe["global_save_present"] == false {
        for _ in 0..20 {
            let saved = invoke(
                context.client(),
                &contract.functions["get"],
                json!({"id":"INC-104"}),
            )
            .await;
            ensure_remote_or_success(&saved, "verify status persisted before the assignee edit")?;
            status_persisted = saved.as_ref().ok().is_some_and(|value| {
                value["incident"]["status"] == "monitoring"
                    && value["incident"]["assignee"] == "Mira Chen"
                    && value["incident"]["history"]
                        .as_array()
                        .is_some_and(|history| history.len() >= 3)
            });
            status_backend = result_value(saved);
            if status_persisted {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let edit_probe = if status_persisted {
        execute(
            context,
            session,
            r#"return await (async()=>{const assignee=document.querySelector('[data-testid="incident-detail"] [aria-label="Assignee"]');if(!assignee)return {ok:false,reason:'missing assignee control'};assignee.focus();let changed=false;if(assignee.matches('input,textarea')){const proto=assignee.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(proto,'value').set.call(assignee,'Ana Costa');assignee.dispatchEvent(new Event('input',{bubbles:true}));assignee.dispatchEvent(new Event('change',{bubbles:true}));changed=true;}else if(assignee.tagName==='SELECT'){Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(assignee,'Ana Costa');assignee.dispatchEvent(new Event('input',{bubbles:true}));assignee.dispatchEvent(new Event('change',{bubbles:true}));changed=true;}else{assignee.click();for(let i=0;i<40;i++){const option=[...document.querySelectorAll('[role="option"]')].find(item=>item.getClientRects().length>0&&item.innerText.trim().toLowerCase()==='ana costa');if(option){option.click();changed=true;break;}await new Promise(r=>setTimeout(r,50));}}if(!changed)return {ok:false,reason:'Ana Costa option was unavailable'};assignee.blur();await new Promise(r=>setTimeout(r,500));const detail=document.querySelector('[data-testid="incident-detail"]');const save=[...detail.querySelectorAll('button,[role="button"]')].some(button=>button.getClientRects().length>0&&/\bsave\b/i.test((button.getAttribute('aria-label')||button.innerText).trim()));return {ok:true,global_save_present:save,control_value:assignee.value||assignee.innerText};})();"#,
        )
        .await?
    } else {
        Value::Null
    };
    let mut assignee_saved_before_reload = false;
    if edit_probe["ok"] == true && edit_probe["global_save_present"] == false {
        for _ in 0..20 {
            let saved = invoke(
                context.client(),
                &contract.functions["get"],
                json!({"id":"INC-104"}),
            )
            .await;
            ensure_remote_or_success(&saved, "verify assignee persisted before reload")?;
            assignee_saved_before_reload = saved.as_ref().ok().is_some_and(|value| {
                value["incident"]["status"] == "monitoring"
                    && value["incident"]["assignee"] == "Ana Costa"
                    && value["incident"]["history"]
                        .as_array()
                        .is_some_and(|history| history.len() >= 4)
            });
            if assignee_saved_before_reload {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let persisted = if assignee_saved_before_reload {
        let reload = context
            .trigger_value(
                "browser::navigate",
                json!({"session_id":session,"url":url,"timeout_ms":30000}),
            )
            .await?;
        let detail_reloaded = reload["ok"] == true
            && execute(
                context,
                session,
                r#"return await (async()=>{for(let i=0;i<200;i++){const d=document.querySelector('[data-testid="incident-detail"][data-incident-id="INC-104"]');const assignee=d?.querySelector('[aria-label="Assignee"]');if((assignee?.value||assignee?.innerText||'').includes('Ana Costa')&&d.innerText.toLowerCase().includes('monitoring')&&d.innerText.includes('Assigned to Ana Costa; status changed to monitoring.'))return true;await new Promise(r=>setTimeout(r,50));}return false;})();"#,
            )
            .await?
            == true;
        let workspace = context
            .trigger_value("console::workspace::list", json!({}))
            .await;
        ensure_remote_or_success(&workspace, "verify Console workspace after reload")?;
        detail_reloaded
            && workspace_has_pair(
                &workspace.unwrap_or(Value::Null),
                &contract.board_page,
                &contract.detail_page,
            )
    } else {
        false
    };
    let backend_saved = invoke(
        context.client(),
        &contract.functions["get"],
        json!({"id":"INC-104"}),
    )
    .await;
    ensure_remote_or_success(&backend_saved, "verify incident update persistence")?;
    let backend_saved_ok = backend_saved.as_ref().ok().is_some_and(|value| {
        value["incident"]["status"] == "monitoring"
            && value["incident"]["assignee"] == "Ana Costa"
            && value["incident"]["history"]
                .as_array()
                .is_some_and(|history| history.len() >= 4)
    });
    let detail_light = execute(context, session, r#"const d=document.querySelector('[data-testid="incident-detail"]');const rect=d?.getBoundingClientRect();return {background:d?getComputedStyle(d).backgroundColor:null,color:d?getComputedStyle(d).color:null,width:rect?.width,viewport:innerWidth,scroll_width:document.documentElement.scrollWidth};"#).await?;
    execute(context, session, r#"document.documentElement.setAttribute('data-theme','dark');return await new Promise(resolve=>setTimeout(()=>resolve(true),150));"#).await?;
    let detail_theme = execute(
        context,
        session,
        r#"return (()=>{const d=document.querySelector('[data-testid="incident-detail"]');return {background:d?getComputedStyle(d).backgroundColor:null,color:d?getComputedStyle(d).color:null,text:d?.innerText||''};})();"#,
    )
    .await?;
    let document_origin = execute(context, session, "return performance.timeOrigin;").await?;
    let mut external_subscribed = false;
    for _ in 0..60 {
        let count = invoke(
            context.client(),
            &contract.functions["fixture"],
            json!({"action":"subscriber-count","id":"INC-104"}),
        )
        .await;
        ensure_remote_or_success(&count, "inspect incident live subscription")?;
        external_subscribed = count
            .as_ref()
            .ok()
            .is_some_and(|value| value["subscribers"].as_u64().is_some_and(|count| count > 0));
        if external_subscribed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let external = invoke(
        context.client(),
        &contract.functions["fixture"],
        json!({"action":"external-update","id":"INC-104"}),
    )
    .await;
    ensure_remote_or_success(&external, "apply runner-owned external incident update")?;
    let external_visible = execute(
        context,
        session,
        r#"return await (async()=>{for(let i=0;i<200;i++){const d=document.querySelector('[data-testid="incident-detail"]');const text=d?.innerText||'';if(text.includes('Release monitor')&&text.toLowerCase().includes('raised to critical')&&text.toLowerCase().includes('critical'))return {visible:true,origin:performance.timeOrigin};await new Promise(r=>setTimeout(r,50));}return {visible:false,origin:performance.timeOrigin};})();"#,
    )
    .await?;
    let detail_screenshot =
        screenshot_png(context, session, "screenshots/detail-live-update.png").await?;
    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":390,"height":844}),
        )
        .await?;
    let detail_mobile_probe = execute(context, session, r#"return (()=>{const root=document.querySelector('[data-testid="incident-detail"]');const rect=root?.getBoundingClientRect();return {width:rect?.width,left:rect?.left,right:rect?.right,scroll_width:document.documentElement.scrollWidth,viewport:innerWidth};})();"#).await?;
    let detail_mobile_ok = detail_mobile_probe["width"]
        .as_f64()
        .is_some_and(|width| width > 250.0 && width <= 390.0)
        && detail_mobile_probe["left"]
            .as_f64()
            .is_some_and(|left| left >= -4.0)
        && detail_mobile_probe["right"]
            .as_f64()
            .is_some_and(|right| right <= 394.0)
        && detail_mobile_probe["scroll_width"]
            .as_u64()
            .is_some_and(|width| width <= 394);
    let detail_wide_ok = detail_light["width"]
        .as_f64()
        .is_some_and(|width| width > 540.0)
        && detail_light["scroll_width"]
            .as_u64()
            .is_some_and(|width| width <= 1924);
    let detail_mobile_screenshot =
        screenshot_png(context, session, "screenshots/detail-phone.png").await?;

    let return_to_board = context
        .trigger_value(
            "console::workspace::open",
            json!({"screen":format!("ext:{}",contract.board_page),"activate":true}),
        )
        .await?;
    let board_again = return_to_board["tab_id"].is_string()
        && execute(context, session, r#"return await (async()=>{for(let i=0;i<120;i++){if(document.querySelector('[data-testid="incident-board"]'))return true;await new Promise(r=>setTimeout(r,50));}return false;})();"#).await? == true;
    let empty_probe = if board_again {
        invoke(
            context.client(),
            &contract.functions["fixture"],
            json!({"action":"empty","enabled":true}),
        )
        .await
    } else {
        Err(anyhow::anyhow!("could not return to incident board"))
    };
    ensure_remote_or_success(&empty_probe, "set empty incident fixture")?;
    let empty_visible = refresh_and_wait(context, session, "[data-testid=\"empty-state\"]").await?;
    let empty_message = execute(
        context,
        session,
        r#"const message=document.querySelector('[data-testid="empty-state"]')?.cloneNode(true);message?.querySelectorAll('button,[role="button"]').forEach(control=>control.remove());return !!message?.textContent.trim();"#,
    )
    .await?
        == true;
    let restore = invoke(
        context.client(),
        &contract.functions["fixture"],
        json!({"action":"restore"}),
    )
    .await;
    ensure_remote_or_success(&restore, "restore incident fixture after empty-state probe")?;
    let fail_list = invoke(
        context.client(),
        &contract.functions["fixture"],
        json!({"action":"fail-list","enabled":true}),
    )
    .await;
    ensure_remote_or_success(&fail_list, "enable incident list failure fixture")?;
    let error_visible = refresh_and_wait(context, session, "[data-testid=\"error-state\"]").await?;
    let error_message = execute(
        context,
        session,
        r#"const message=document.querySelector('[data-testid="error-state"]')?.cloneNode(true);message?.querySelectorAll('button,[role="button"]').forEach(control=>control.remove());return !!message?.textContent.trim();"#,
    )
    .await?
        == true;
    let retry_available = execute(context, session, r#"return !!document.querySelector('[data-testid="error-state"] [data-testid="retry-incidents"]')||!!document.querySelector('[data-testid="retry-incidents"]');"#).await? == true;
    let restored = invoke(
        context.client(),
        &contract.functions["fixture"],
        json!({"action":"restore"}),
    )
    .await;
    ensure_remote_or_success(
        &restored,
        "restore incident fixture after error-state probe",
    )?;
    let retry_visible = retry_and_wait(
        context,
        session,
        "[data-testid=\"incident-board\"] [data-incident-id]",
    )
    .await?;

    let style = fs::read_to_string(root.join("ui/styles.css")).unwrap_or_default();
    let theme_ok = style_has_theme_rules(&style)
        && (board_probe["background"] != dark_board["background"]
            || board_probe["color"] != dark_board["color"])
        && (detail_light["background"] != detail_theme["background"]
            || detail_light["color"] != detail_theme["color"]);
    let keyboard_ok = card_open && focus_ring == true && style_has_focus_rule(&style);
    let search_ok =
        board_probe["search_tag"] == "INPUT" && search_probe["ok"] == true && filtered_counts_ok;
    let pane_ok = card_open && detail_ready == true && two_panes;
    let edits_ok = status_persisted
        && status_probe["global_save_present"] == false
        && edit_probe["ok"] == true
        && edit_probe["global_save_present"] == false
        && assignee_saved_before_reload;
    let reload_ok = persisted && backend_saved_ok;
    let empty_ok = empty_visible && empty_message;
    let retry_ok = error_visible && error_message && retry_available && retry_visible;
    let board_wide_ok = wide_lanes_ok
        && board_probe["scroll_width"]
            .as_u64()
            .is_some_and(|width| width <= 1924);
    let layout_ok = board_wide_ok
        && mobile_ok
        && phone_tabs_switch
        && narrow_split_probe["ok"] == true
        && detail_mobile_ok
        && detail_wide_ok;
    let mut checks = serde_json::Map::new();
    checks.insert("board_search".into(), json!({
        "passed":seeded_board_ok && search_ok,
        "awarded":(if seeded_board_ok {9} else {0}) + (if search_ok {5} else {0}),
        "reason":format!("Seeded lanes and counts: {}/9; cache search and filtered counts: {}/5.", if seeded_board_ok {9} else {0}, if search_ok {5} else {0}),
        "board":board_probe,"search":search_probe,
    }));
    checks.insert("detail_and_persistence".into(), json!({
        "passed":pane_ok && edits_ok && reload_ok,
        "awarded":(if pane_ok {6} else {0}) + (if edits_ok {6} else {0}) + (if reload_ok {6} else {0}),
        "reason":format!("Contextual pane: {}/6; separate field saves: {}/6; values and history after reload: {}/6.", if pane_ok {6} else {0}, if edits_ok {6} else {0}, if reload_ok {6} else {0}),
        "workspace":workspace,"two_panes":two_panes,"detail_ready":detail_ready,"keyboard_card":card_focus,"status_edit":status_probe,"status_backend_before_assignee":status_backend,"status_persisted_before_assignee":status_persisted,"assignee_edit":edit_probe,"assignee_saved_before_reload":assignee_saved_before_reload,"persisted_after_reload":persisted,"backend":result_value(backend_saved),
    }));
    checks.insert("external_live_update".into(), json!({
        "passed":external_subscribed && external_visible["visible"] == true && external_visible["origin"] == document_origin,
        "reason":if external_subscribed && external_visible["visible"] == true && external_visible["origin"] == document_origin {"An active incident subscription delivered the external priority and history change without a document reload."} else {"No matching live subscription was active, or the external change did not appear without reloading."},
        "runner_mutation":result_value(external),"matching_subscription":external_subscribed,"visible_without_reload":external_visible["visible"],"document_origin_before":document_origin,"document_origin_after":external_visible["origin"],
    }));
    checks.insert("empty_and_error_recovery".into(), json!({
        "passed":empty_ok && retry_ok,
        "awarded":(if empty_ok {3} else {0}) + (if retry_ok {5} else {0}),
        "reason":format!("Empty message: {}/3; failed-load message and Retry: {}/5.", if empty_ok {3} else {0}, if retry_ok {5} else {0}),
        "empty_state":empty_visible,"empty_message":empty_message,"error_state":error_visible,"error_message":error_message,"retry_available":retry_available,"retry_recovered":retry_visible,
    }));
    checks.insert("responsive_theme_keyboard".into(), json!({
        "passed":layout_ok && theme_ok && keyboard_ok,
        "awarded":(if layout_ok {3} else {0}) + (if theme_ok {1} else {0}) + (if keyboard_ok {2} else {0}),
        "reason":format!("Viewport fit and lane tabs: {}/3; themes: {}/1; keyboard and focus: {}/2.", if layout_ok {3} else {0}, if theme_ok {1} else {0}, if keyboard_ok {2} else {0}),
        "mobile_board":mobile_probe,"phone_tabs_switch":phone_tabs_switch,"narrow_split":narrow_split_probe,"mobile_detail":detail_mobile_probe,"theme":{"light_board":board_probe,"dark_board":dark_board,"light_detail":detail_light,"dark_detail":detail_theme},"theme_rules":theme_ok,"keyboard_semantics":keyboard_ok,
    }));
    let captures = [
        mobile_screenshot,
        board_light,
        detail_screenshot,
        detail_mobile_screenshot,
    ]
    .into_iter()
    .filter(|capture| capture["data"].is_string())
    .collect::<Vec<_>>();
    Ok(json!({
        "status":"checked",
        "reason":"Browser probes completed in the live Console.",
        "url":url,
        "baseline":baseline,
        "navigation":navigation,
        "checks":checks,
        "captures":captures,
    }))
}

async fn execute(context: &E2eContext, session: &str, code: &str) -> Result<Value> {
    let result = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"timeout_ms":30000,"code":code}),
        )
        .await?;
    Ok(result["result"].clone())
}

async fn screenshot_png(context: &E2eContext, session: &str, file: &str) -> Result<Value> {
    let value = context
        .trigger_value(
            "browser::screenshot",
            json!({"session_id":session,"full_page":true,"format":"png"}),
        )
        .await?;
    if value["details"]["session_id"] != session {
        bail!("incident board screenshot session identity mismatch");
    }
    let block = value["content"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"] == "image" && item["mime"] == "image/png")
        })
        .context("incident board screenshot omitted PNG")?;
    let data = block["data"]
        .as_str()
        .context("incident board PNG data missing")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    if bytes.len() > MAX_SCREENSHOT_BYTES {
        return Ok(
            json!({"file":file,"oversized":true,"size_bytes":bytes.len(),"maximum_bytes":MAX_SCREENSHOT_BYTES,"details":value["details"]}),
        );
    }
    Ok(
        json!({"file":file,"data":data,"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(&bytes),"details":value["details"]}),
    )
}

async fn refresh_and_wait(context: &E2eContext, session: &str, selector: &str) -> Result<bool> {
    let selector = serde_json::to_string(selector)?;
    let code = format!(
        "const selector={selector};\n{}",
        r#"document.querySelector('[data-testid="refresh-incidents"]')?.click();return await (async()=>{for(let i=0;i<100;i++){const e=document.querySelector(selector);if(e&&(e.offsetWidth||e.offsetHeight||e.getClientRects().length))return true;await new Promise(r=>setTimeout(r,50));}return false;})();"#
    );
    Ok(execute(context, session, &code).await? == true)
}

async fn retry_and_wait(context: &E2eContext, session: &str, selector: &str) -> Result<bool> {
    let selector = serde_json::to_string(selector)?;
    let code = format!(
        "const selector={selector};\n{}",
        r#"document.querySelector('[data-testid="retry-incidents"]')?.click();return await (async()=>{for(let i=0;i<100;i++){const error=document.querySelector('[data-testid="error-state"]');const visibleError=!!error&&(error.offsetWidth||error.offsetHeight||error.getClientRects().length);const e=document.querySelector(selector);const visible=!!e&&(e.offsetWidth||e.offsetHeight||e.getClientRects().length);const cards=[...document.querySelectorAll('[data-testid="incident-board"] [data-incident-id]')];if(visible&&!visibleError&&cards.length===5)return true;await new Promise(r=>setTimeout(r,50));}return false;})();"#
    );
    Ok(execute(context, session, &code).await? == true)
}

fn style_has_theme_rules(style: &str) -> bool {
    style.contains("var(--") || style.contains("data-theme")
}

fn style_has_focus_rule(style: &str) -> bool {
    style.contains(":focus-visible") || style.contains(":focus {") || style.contains(":focus{")
}

fn browser_failure(reason: &str, navigation: Value, status: &str) -> Value {
    let mut checks = serde_json::Map::new();
    for id in [
        "board_search",
        "detail_and_persistence",
        "external_live_update",
        "empty_and_error_recovery",
        "responsive_theme_keyboard",
    ] {
        checks.insert(
            id.into(),
            json!({"passed":false,"status":status,"reason":reason}),
        );
    }
    json!({"status":status,"reason":reason,"checks":checks,"captures":[],"navigation":navigation})
}
