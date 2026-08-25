use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::context::E2eContext;
use crate::wire::{
    FunctionPolicy, MessageInput, SendOptions, SendRequest, SendResponse, SessionInit,
};

use super::{
    AdaptiveMaterializedWorkflow, AdaptivePlanNodeV1, AdaptivePlanRevisionEvidence,
    AdaptiveWorkflowPlanV1, AdaptiveWorkflowPolicyV1, StepCatalog,
    ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
};

const AGENT_PLANNER_SCHEMA_VERSION: u32 = 1;
const PLANNER_STUCK_TIMEOUT: Duration = Duration::from_secs(180);
const PLANNER_MAX_OUTPUT_TOKENS: u64 = 32 * 1024;
const PLANNER_MAX_TOTAL_TOKENS: u64 = 64 * 1024;

/// Reference information that may be shown to the planner. It describes how a
/// plan will be judged, but deliberately contains no reference-plan nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePlannerReferenceCheckV1 {
    pub id: String,
    pub description: String,
}

/// A runner-observed event that requires the single permitted replan. Evidence
/// identifiers are runner-owned and the planner must bind revision two to all
/// of them exactly; it cannot invent or omit an invalidation receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePlannerInvalidationV1 {
    pub description: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePlannerMetadataV1 {
    pub scenario_id: String,
    pub objective: String,
    #[serde(default)]
    pub reference_checks: Vec<AdaptivePlannerReferenceCheckV1>,
    pub invalidation: AdaptivePlannerInvalidationV1,
}

/// Inputs owned by the runner. `catalog` is never serialized into the model
/// prompt; it is used only for deterministic policy/materialization checks.
pub struct AgentPlannerRequest<'a> {
    pub context: &'a E2eContext,
    pub model: &'a str,
    pub provider: &'a str,
    pub scenario_prompt: &'a str,
    pub policy: &'a AdaptiveWorkflowPolicyV1,
    pub catalog: &'a StepCatalog,
    pub metadata: &'a AdaptivePlannerMetadataV1,
    pub execution_id: &'a str,
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub state_root: &'a Path,
    pub restored_attempt: bool,
    pub cancellation: Option<&'a watch::Receiver<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPlannerUsageEvidenceV1 {
    pub turns: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// Sanitized public evidence. Complete planner output and plans remain only in
/// the private store; reports can expose their digests and bounded usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPlannerEvidenceV1 {
    pub restored: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub policy_sha256: String,
    pub plans_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentPlannerUsageEvidenceV1>,
    pub revisions: Vec<AdaptivePlanRevisionEvidence>,
}

#[derive(Debug, Clone)]
pub struct AgentPlannerOutcome {
    pub plans: Vec<AdaptiveWorkflowPlanV1>,
    pub completed_node_ids: BTreeSet<String>,
    pub materialized: AdaptiveMaterializedWorkflow,
    pub evidence: AgentPlannerEvidenceV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPlanDocumentV1 {
    revision_1: AgentPlanRevisionOneV1,
    revision_2: AgentPlanRevisionTwoV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPlanRevisionOneV1 {
    nodes: Vec<AdaptivePlanNodeV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPlanRevisionTwoV1 {
    nodes: Vec<AdaptivePlanNodeV1>,
    reason: String,
    evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPlannerBindingV1 {
    schema_version: u32,
    execution_id: String,
    run_id: String,
    attempt_id: String,
    scenario_id: String,
    model: String,
    provider: String,
    policy_sha256: String,
    prompt_sha256: String,
    metadata_sha256: String,
}

impl AgentPlannerBindingV1 {
    fn validate(&self) -> Result<()> {
        if self.schema_version != AGENT_PLANNER_SCHEMA_VERSION {
            bail!("unsupported adaptive planner binding schema version");
        }
        for (label, value) in [
            ("execution id", self.execution_id.as_str()),
            ("run id", self.run_id.as_str()),
            ("attempt id", self.attempt_id.as_str()),
            ("scenario id", self.scenario_id.as_str()),
        ] {
            validate_path_identifier(value, label)?;
        }
        if self.model.trim().is_empty() || self.provider.trim().is_empty() {
            bail!("adaptive planner binding requires model and provider");
        }
        for (label, digest) in [
            ("policy", self.policy_sha256.as_str()),
            ("prompt", self.prompt_sha256.as_str()),
            ("metadata", self.metadata_sha256.as_str()),
        ] {
            validate_sha256(digest).with_context(|| format!("validate {label} digest"))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPlannerPrivateEnvelopeV1 {
    schema_version: u32,
    binding: AgentPlannerBindingV1,
    plans_sha256: String,
    plans: Vec<AdaptiveWorkflowPlanV1>,
}

#[derive(Debug, Clone)]
struct AgentPlannerStore {
    state_root: PathBuf,
    relative_path: PathBuf,
}

impl AgentPlannerStore {
    fn new(
        state_root: impl AsRef<Path>,
        execution_id: &str,
        run_id: &str,
        attempt_id: &str,
    ) -> Result<Self> {
        validate_path_identifier(execution_id, "execution id")?;
        validate_path_identifier(run_id, "run id")?;
        validate_path_identifier(attempt_id, "attempt id")?;
        Ok(Self {
            state_root: state_root.as_ref().to_path_buf(),
            relative_path: PathBuf::from("adaptive-plans")
                .join(execution_id)
                .join(run_id)
                .join(attempt_id)
                .join("plans-v1.json"),
        })
    }

    fn path(&self) -> PathBuf {
        self.state_root.join(&self.relative_path)
    }

    fn persist(
        &self,
        binding: &AgentPlannerBindingV1,
        plans: &[AdaptiveWorkflowPlanV1],
    ) -> Result<String> {
        binding.validate()?;
        let plans_sha256 = crate::artifact::sha256_value(&plans)?;
        let envelope = AgentPlannerPrivateEnvelopeV1 {
            schema_version: AGENT_PLANNER_SCHEMA_VERSION,
            binding: binding.clone(),
            plans_sha256: plans_sha256.clone(),
            plans: plans.to_vec(),
        };
        let mut bytes = serde_json::to_vec_pretty(&envelope)
            .context("encode private adaptive planner state")?;
        bytes.push(b'\n');
        let path = self.path();
        let parent = path
            .parent()
            .context("adaptive planner state path has no parent")?;
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        #[cfg(unix)]
        fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .with_context(|| format!("protect {}", parent.display()))?;

        if path.exists() {
            let existing = self.load_envelope()?;
            if existing.binding != *binding || existing.plans_sha256 != plans_sha256 {
                bail!("refusing to replace conflicting adaptive planner state");
            }
            return Ok(plans_sha256);
        }

        let temporary = path.with_file_name(".plans-v1.json.tmp");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(plans_sha256)
    }

    fn load(
        &self,
        expected: &AgentPlannerBindingV1,
    ) -> Result<(Vec<AdaptiveWorkflowPlanV1>, String)> {
        expected.validate()?;
        let envelope = self.load_envelope()?;
        if envelope.binding != *expected {
            bail!("adaptive planner state does not match the current attempt binding");
        }
        Ok((envelope.plans, envelope.plans_sha256))
    }

    fn load_envelope(&self) -> Result<AgentPlannerPrivateEnvelopeV1> {
        let path = self.path();
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let envelope: AgentPlannerPrivateEnvelopeV1 =
            serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))?;
        if envelope.schema_version != AGENT_PLANNER_SCHEMA_VERSION {
            bail!("unsupported private adaptive planner state schema version");
        }
        envelope.binding.validate()?;
        validate_sha256(&envelope.plans_sha256)?;
        let observed = crate::artifact::sha256_value(&envelope.plans)?;
        if observed != envelope.plans_sha256 {
            bail!("adaptive planner state plan digest mismatch");
        }
        Ok(envelope)
    }
}

/// Ask the subject for two bounded plan revisions or recover the exact plans
/// already persisted for this attempt. Restore never calls Harness.
pub async fn plan_adaptive_workflow(
    request: AgentPlannerRequest<'_>,
) -> Result<AgentPlannerOutcome> {
    validate_request(&request)?;
    let policy_sha256 = request.policy.canonical_sha256()?;
    let binding = AgentPlannerBindingV1 {
        schema_version: AGENT_PLANNER_SCHEMA_VERSION,
        execution_id: request.execution_id.into(),
        run_id: request.run_id.into(),
        attempt_id: request.attempt_id.into(),
        scenario_id: request.metadata.scenario_id.clone(),
        model: request.model.into(),
        provider: request.provider.into(),
        policy_sha256: policy_sha256.clone(),
        prompt_sha256: crate::artifact::sha256_bytes(request.scenario_prompt.as_bytes()),
        metadata_sha256: crate::artifact::sha256_value(request.metadata)?,
    };
    let store = AgentPlannerStore::new(
        request.state_root,
        request.execution_id,
        request.run_id,
        request.attempt_id,
    )?;

    if request.restored_attempt {
        let (plans, plans_sha256) = store.load(&binding)?;
        let completed_node_ids = revision_one_node_ids(&plans)?;
        validate_invalidation_binding(&plans, request.metadata)?;
        let materialized = request
            .policy
            .materialize(&plans, &completed_node_ids, request.catalog)
            .context("validate restored adaptive plans")?;
        return Ok(AgentPlannerOutcome {
            evidence: AgentPlannerEvidenceV1 {
                restored: true,
                session_id: None,
                policy_sha256,
                plans_sha256,
                transcript_sha256: None,
                usage: None,
                revisions: materialized.revisions.clone(),
            },
            plans,
            completed_node_ids,
            materialized,
        });
    }

    let session_id = format!("adaptive_planner_{}", request.attempt_id);
    request
        .context
        .bind_turn_completed()
        .await
        .context("bind adaptive planner turn observation")?;
    let result = run_fresh_planner(&request, &binding, &store, &session_id, &policy_sha256).await;
    let teardown = request.context.teardown(&session_id).await;
    let unbind = request.context.unbind_turn_completed().await;
    match result {
        Ok(outcome) => {
            teardown.context("teardown adaptive planner session")?;
            unbind.context("unbind adaptive planner turn observation")?;
            Ok(outcome)
        }
        Err(error) => {
            if let Err(cleanup_error) = teardown {
                tracing::warn!(error = %format!("{cleanup_error:#}"), "adaptive planner teardown failed after planner error");
            }
            if let Err(cleanup_error) = unbind {
                tracing::warn!(error = %format!("{cleanup_error:#}"), "adaptive planner observation unbind failed after planner error");
            }
            Err(error)
        }
    }
}

async fn run_fresh_planner(
    request: &AgentPlannerRequest<'_>,
    binding: &AgentPlannerBindingV1,
    store: &AgentPlannerStore,
    session_id: &str,
    policy_sha256: &str,
) -> Result<AgentPlannerOutcome> {
    request.context.drain_turn_completed_events();
    let planner_prompt = build_planner_prompt(
        request.scenario_prompt,
        request.policy,
        request.metadata,
        policy_sha256,
    )?;
    let response: SendResponse = request
        .context
        .trigger(
            "harness::send",
            SendRequest {
                session_id: Some(session_id.into()),
                message: MessageInput::Text(planner_prompt),
                model: Some(request.model.into()),
                provider: Some(request.provider.into()),
                idempotency_key: Some(format!(
                    "e2e:{}:{}:adaptive-planner",
                    request.run_id, request.attempt_id
                )),
                session: Some(SessionInit {
                    title: Some(format!(
                        "Harness E2E adaptive planner: {}",
                        request.metadata.scenario_id
                    )),
                    metadata: Some(json!({
                        "e2e_execution_id": request.execution_id,
                        "e2e_run_id": request.run_id,
                        "e2e_attempt_id": request.attempt_id,
                        "e2e_scenario": request.metadata.scenario_id,
                        "e2e_execution_kind": "adaptive_planner",
                        "adaptive_policy_sha256": policy_sha256,
                    })),
                }),
                options: Some(SendOptions {
                    max_turns: Some(1),
                    max_output_tokens: Some(PLANNER_MAX_OUTPUT_TOKENS),
                    max_total_tokens: Some(PLANNER_MAX_TOTAL_TOKENS),
                    max_validation_retries: Some(0),
                    functions: Some(FunctionPolicy {
                        allow: Vec::new(),
                        deny: vec!["*".into(), "e2e::*".into()],
                        ..FunctionPolicy::default()
                    }),
                    metadata: None,
                }),
            },
        )
        .await
        .context("send adaptive planner turn")?;
    if !response.accepted
        || response.session_id != session_id
        || response.merged == Some(true)
        || response.queued == Some(true)
    {
        bail!("adaptive planner harness::send returned an unexpected response: {response:?}");
    }
    let metrics = request
        .context
        .wait_for_turn(
            &request.metadata.scenario_id,
            session_id,
            &response.turn_id,
            PLANNER_STUCK_TIMEOUT,
            false,
            request.cancellation,
        )
        .await
        .context("wait for adaptive planner turn")?;
    let transcript = request
        .context
        .transcript(session_id)
        .await
        .context("collect adaptive planner transcript")?;
    let response_text = final_assistant_text(&transcript)?;
    let document = parse_agent_plan_document(response_text)?;
    let plans = bind_agent_document(document, policy_sha256)?;
    validate_invalidation_binding(&plans, request.metadata)?;
    let completed_node_ids = revision_one_node_ids(&plans)?;
    let materialized = request
        .policy
        .materialize(&plans, &completed_node_ids, request.catalog)
        .context("validate agent-authored adaptive plans")?;
    let plans_sha256 = store.persist(binding, &plans)?;
    let transcript_sha256 = crate::artifact::sha256_value(&transcript)?;
    let usage = AgentPlannerUsageEvidenceV1 {
        turns: metrics.totals.turns,
        input_tokens: metrics.totals.input_tokens,
        output_tokens: metrics.totals.output_tokens,
        cache_read_tokens: metrics.totals.cache_read_tokens,
        cache_write_tokens: metrics.totals.cache_write_tokens,
        reasoning_tokens: metrics.totals.reasoning_tokens,
        cost_usd: metrics.totals.cost_usd,
    };
    Ok(AgentPlannerOutcome {
        evidence: AgentPlannerEvidenceV1 {
            restored: false,
            session_id: Some(session_id.into()),
            policy_sha256: policy_sha256.into(),
            plans_sha256,
            transcript_sha256: Some(transcript_sha256),
            usage: Some(usage),
            revisions: materialized.revisions.clone(),
        },
        plans,
        completed_node_ids,
        materialized,
    })
}

fn validate_request(request: &AgentPlannerRequest<'_>) -> Result<()> {
    for (label, value) in [
        ("execution id", request.execution_id),
        ("run id", request.run_id),
        ("attempt id", request.attempt_id),
        ("scenario id", request.metadata.scenario_id.as_str()),
    ] {
        validate_path_identifier(value, label)?;
    }
    if request.model.trim().is_empty()
        || request.provider.trim().is_empty()
        || request.scenario_prompt.trim().is_empty()
        || request.metadata.objective.trim().is_empty()
        || request.metadata.invalidation.description.trim().is_empty()
    {
        bail!("adaptive planner requires model, provider, prompt, objective and invalidation");
    }
    if request.policy.id != request.metadata.scenario_id {
        bail!("adaptive planner metadata scenario does not match policy id");
    }
    if request.policy.max_plan_revisions != 2 {
        bail!("agent planner requires exactly two adaptive plan revisions");
    }
    let mut check_ids = BTreeSet::new();
    for check in &request.metadata.reference_checks {
        validate_path_identifier(&check.id, "reference check id")?;
        if check.description.trim().is_empty() || !check_ids.insert(check.id.as_str()) {
            bail!("adaptive planner reference checks must be described and unique");
        }
    }
    let mut evidence_ids = BTreeSet::new();
    for evidence_id in &request.metadata.invalidation.evidence_ids {
        validate_evidence_id(evidence_id)?;
        if !evidence_ids.insert(evidence_id.as_str()) {
            bail!("adaptive planner invalidation evidence ids must be unique");
        }
    }
    if evidence_ids.is_empty() {
        bail!("adaptive planner invalidation requires evidence ids");
    }
    Ok(())
}

fn build_planner_prompt(
    scenario_prompt: &str,
    policy: &AdaptiveWorkflowPolicyV1,
    metadata: &AdaptivePlannerMetadataV1,
    policy_sha256: &str,
) -> Result<String> {
    let response_shape = json!({
        "revision_1": {
            "nodes": [{
                "id": "unique_node_id",
                "template_id": "one_allowlisted_template_id",
                "depends_on": ["prior_node_id_or_before_plan_anchor"],
                "focus": "only_when_the_template_allows_it",
                "instructions": "only_when_the_template_allows_it"
            }]
        },
        "revision_2": {
            "nodes": [{
                "id": "all_revision_1_nodes_unchanged_then_optional_new_nodes",
                "template_id": "one_allowlisted_template_id",
                "depends_on": [],
            }],
            "reason": "why the trusted invalidation requires this revision",
            "evidence_ids": metadata.invalidation.evidence_ids,
        }
    });
    Ok(format!(
        "You are the bounded planner for an E2E scenario. Return exactly one JSON object and no markdown, prose, or code fences. Unknown fields are rejected. You may choose only node ids, allowlisted template ids, dependencies, and the optional focus/instructions values permitted by the policy. Never invent a step type, function, workspace, budget, activation, criterion, mutation control, or evidence id. Revision 2 occurs after every revision-1 node completed: preserve every revision-1 node byte-for-byte and append only work required by the trusted invalidation.\n\nScenario request:\n{scenario_prompt}\n\nReference objective and checks (not a reference plan):\n{}\n\nTrusted invalidation metadata:\n{}\n\nFrozen runner policy (sha256 {policy_sha256}):\n{}\n\nRequired response shape:\n{}",
        serde_json::to_string_pretty(&json!({
            "objective": metadata.objective,
            "checks": metadata.reference_checks,
        }))?,
        serde_json::to_string_pretty(&metadata.invalidation)?,
        serde_json::to_string_pretty(policy)?,
        serde_json::to_string_pretty(&response_shape)?,
    ))
}

fn parse_agent_plan_document(text: &str) -> Result<AgentPlanDocumentV1> {
    let trimmed = text.trim();
    if trimmed.starts_with("```") || trimmed.ends_with("```") {
        bail!("adaptive planner response must be bare JSON without code fences");
    }
    serde_json::from_str(trimmed).context("decode strict adaptive planner JSON")
}

fn bind_agent_document(
    document: AgentPlanDocumentV1,
    policy_sha256: &str,
) -> Result<Vec<AdaptiveWorkflowPlanV1>> {
    validate_sha256(policy_sha256)?;
    let first = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256: policy_sha256.into(),
        revision: 1,
        supersedes_sha256: None,
        reason: None,
        evidence_ids: Vec::new(),
        nodes: document.revision_1.nodes,
    };
    let first_sha256 = first.canonical_sha256()?;
    let second = AdaptiveWorkflowPlanV1 {
        schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
        policy_sha256: policy_sha256.into(),
        revision: 2,
        supersedes_sha256: Some(first_sha256),
        reason: Some(document.revision_2.reason),
        evidence_ids: document.revision_2.evidence_ids,
        nodes: document.revision_2.nodes,
    };
    Ok(vec![first, second])
}

fn validate_invalidation_binding(
    plans: &[AdaptiveWorkflowPlanV1],
    metadata: &AdaptivePlannerMetadataV1,
) -> Result<()> {
    if plans.len() != 2 {
        bail!("adaptive planner must produce exactly two revisions");
    }
    if plans[1].evidence_ids != metadata.invalidation.evidence_ids {
        bail!("adaptive plan revision 2 does not bind the exact trusted evidence ids");
    }
    Ok(())
}

fn revision_one_node_ids(plans: &[AdaptiveWorkflowPlanV1]) -> Result<BTreeSet<String>> {
    let first = plans
        .first()
        .context("adaptive planner state has no revision 1")?;
    if first.revision != 1 {
        bail!("adaptive planner state does not begin at revision 1");
    }
    Ok(first.nodes.iter().map(|node| node.id.clone()).collect())
}

fn final_assistant_text(transcript: &Value) -> Result<&str> {
    let messages = transcript
        .get("messages")
        .and_then(Value::as_array)
        .context("adaptive planner transcript has no messages")?;
    for envelope in messages.iter().rev() {
        let message = envelope.get("message").unwrap_or(envelope);
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            return Ok(text);
        }
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .context("adaptive planner assistant message has malformed content")?;
        let texts = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>();
        if texts.len() != 1 {
            bail!("adaptive planner response must contain exactly one text block");
        }
        return Ok(texts[0]);
    }
    bail!("adaptive planner transcript has no assistant response")
}

fn validate_path_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        bail!("{label} must be a safe identifier");
    }
    Ok(())
}

fn validate_evidence_id(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        bail!("adaptive planner evidence id is invalid");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        bail!("digest must use sha256:<hex>");
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("digest must contain 64 hexadecimal characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::workflow::{
        ActivationPolicy, AdaptiveAnchorPlacement, AdaptiveNodeTemplateV1, AdaptiveTrustedAnchorV1,
        ControlSource, DependencyPolicy, PortValueKind, ReplayPolicy, StepOperationalKind,
        StepPortDescriptor, StepTypeDescriptor, WorkflowLimits, WorkflowNodeV1,
    };

    fn document_json() -> String {
        serde_json::to_string(&json!({
            "revision_1": {"nodes": [
                {"id": "inspect", "template_id": "inspect", "depends_on": ["preflight"]}
            ]},
            "revision_2": {
                "nodes": [
                    {"id": "inspect", "template_id": "inspect", "depends_on": ["preflight"]},
                    {"id": "repair", "template_id": "repair", "depends_on": ["inspect"]}
                ],
                "reason": "trusted check invalidated the first plan",
                "evidence_ids": ["validation/v1"]
            }
        }))
        .unwrap()
    }

    fn binding() -> AgentPlannerBindingV1 {
        AgentPlannerBindingV1 {
            schema_version: AGENT_PLANNER_SCHEMA_VERSION,
            execution_id: "execution-1".into(),
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            scenario_id: "scenario-1".into(),
            model: "model".into(),
            provider: "provider".into(),
            policy_sha256: crate::artifact::sha256_bytes(b"policy"),
            prompt_sha256: crate::artifact::sha256_bytes(b"prompt"),
            metadata_sha256: crate::artifact::sha256_bytes(b"metadata"),
        }
    }

    fn policy_and_catalog() -> (AdaptiveWorkflowPolicyV1, StepCatalog) {
        let descriptor = |id: &str| StepTypeDescriptor {
            id: id.into(),
            version: 1,
            description: id.into(),
            config_schema: json!({"type": "object", "additionalProperties": false}),
            inputs: BTreeMap::new(),
            outputs: BTreeMap::from([(
                "ok".into(),
                StepPortDescriptor {
                    kind: PortValueKind::Boolean,
                    optional: false,
                    control_source: Some(ControlSource::Deterministic),
                },
            )]),
            capabilities: Vec::new(),
            required_functions: Vec::new(),
            replay_policy: ReplayPolicy::Idempotent,
            operational_kind: StepOperationalKind::Transformation,
        };
        let mut catalog = StepCatalog::new();
        catalog
            .register_descriptor(descriptor("inspect.step"))
            .unwrap();
        catalog
            .register_descriptor(descriptor("repair.step"))
            .unwrap();
        catalog
            .register_descriptor(descriptor("anchor.step"))
            .unwrap();
        let anchor = |id: &str, depends_on: Vec<String>| WorkflowNodeV1 {
            id: id.into(),
            step_type: "anchor.step".into(),
            step_version: 1,
            depends_on,
            inputs: BTreeMap::new(),
            config: json!({}),
            activation: ActivationPolicy::Always,
            dependency_policy: DependencyPolicy::Succeeded,
            required: true,
        };
        let policy = AdaptiveWorkflowPolicyV1 {
            schema_version: ADAPTIVE_WORKFLOW_SCHEMA_VERSION,
            id: "scenario-1".into(),
            scenario_version: 1,
            description: "fixture".into(),
            limits: WorkflowLimits {
                max_parallel: 1,
                max_nodes: 8,
                ..WorkflowLimits::default()
            },
            max_plan_nodes: 4,
            max_plan_depth: 4,
            max_plan_revisions: 2,
            max_instruction_bytes: 1024,
            templates: vec![
                AdaptiveNodeTemplateV1 {
                    id: "inspect".into(),
                    description: "inspect".into(),
                    step_type: "inspect.step".into(),
                    step_version: 1,
                    base_config: json!({}),
                    inputs: BTreeMap::new(),
                    activation: ActivationPolicy::Always,
                    dependency_policy: DependencyPolicy::Succeeded,
                    required: true,
                    allowed_focuses: Vec::new(),
                    focus_config_key: None,
                    instructions_config_key: None,
                    min_occurrences: 1,
                    max_occurrences: 1,
                },
                AdaptiveNodeTemplateV1 {
                    id: "repair".into(),
                    description: "repair".into(),
                    step_type: "repair.step".into(),
                    step_version: 1,
                    base_config: json!({}),
                    inputs: BTreeMap::new(),
                    activation: ActivationPolicy::Always,
                    dependency_policy: DependencyPolicy::Succeeded,
                    required: true,
                    allowed_focuses: Vec::new(),
                    focus_config_key: None,
                    instructions_config_key: None,
                    min_occurrences: 0,
                    max_occurrences: 1,
                },
            ],
            trusted_anchors: vec![
                AdaptiveTrustedAnchorV1 {
                    placement: AdaptiveAnchorPlacement::BeforePlan,
                    terminal: false,
                    node: anchor("preflight", Vec::new()),
                },
                AdaptiveTrustedAnchorV1 {
                    placement: AdaptiveAnchorPlacement::AfterPlan,
                    terminal: true,
                    node: anchor("finalize", vec!["repair".into()]),
                },
            ],
            criteria: Vec::new(),
        };
        (policy, catalog)
    }

    #[test]
    fn parser_accepts_only_bare_strict_json() {
        let parsed = parse_agent_plan_document(&document_json()).unwrap();
        assert_eq!(parsed.revision_1.nodes[0].id, "inspect");
        assert!(parse_agent_plan_document(&format!("```json\n{}\n```", document_json())).is_err());

        let with_unknown =
            document_json().replace("\"revision_1\":{", "\"revision_1\":{\"unexpected\":true,");
        assert!(parse_agent_plan_document(&with_unknown).is_err());
    }

    #[test]
    fn runner_binds_sequence_policy_and_supersedes_hash() {
        let policy_sha256 = crate::artifact::sha256_bytes(b"policy");
        let document = parse_agent_plan_document(&document_json()).unwrap();
        let plans = bind_agent_document(document, &policy_sha256).unwrap();
        assert_eq!(plans[0].revision, 1);
        assert_eq!(plans[0].policy_sha256, policy_sha256);
        assert_eq!(plans[1].revision, 2);
        assert_eq!(
            plans[1].supersedes_sha256.as_deref(),
            Some(plans[0].canonical_sha256().unwrap().as_str())
        );
    }

    #[test]
    fn store_rejects_tampering_and_binding_changes() {
        let root = tempfile::tempdir().unwrap();
        let store =
            AgentPlannerStore::new(root.path(), "execution-1", "run-1", "attempt-1").unwrap();
        let plans = bind_agent_document(
            parse_agent_plan_document(&document_json()).unwrap(),
            &binding().policy_sha256,
        )
        .unwrap();
        store.persist(&binding(), &plans).unwrap();
        let (loaded, _) = store.load(&binding()).unwrap();
        assert_eq!(loaded[1].nodes.len(), 2);

        let mut changed = binding();
        changed.prompt_sha256 = crate::artifact::sha256_bytes(b"different prompt");
        assert!(store.load(&changed).is_err());

        let path = store.path();
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["plans"][1]["reason"] = Value::String("tampered".into());
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(store.load(&binding()).is_err());
    }

    #[test]
    fn path_identifiers_cannot_escape_private_root() {
        let root = tempfile::tempdir().unwrap();
        for invalid in ["", ".", "..", "../escape", "a/b", "a\\b", "white space"] {
            assert!(AgentPlannerStore::new(root.path(), invalid, "run", "attempt").is_err());
        }
        let store = AgentPlannerStore::new(root.path(), "exec.good", "run_1", "attempt-1").unwrap();
        assert!(store.path().starts_with(root.path()));
    }

    #[test]
    fn bound_plans_materialize_and_preserve_completed_revision_one() {
        let (policy, catalog) = policy_and_catalog();
        let plans = bind_agent_document(
            parse_agent_plan_document(&document_json()).unwrap(),
            &policy.canonical_sha256().unwrap(),
        )
        .unwrap();
        let completed = revision_one_node_ids(&plans).unwrap();
        let materialized = policy.materialize(&plans, &completed, &catalog).unwrap();
        assert_eq!(materialized.revisions.len(), 2);
        assert!(materialized
            .definition
            .nodes
            .iter()
            .any(|node| node.id == "repair"));

        let mut modified = plans;
        modified[1].nodes[0].depends_on.clear();
        assert!(policy.materialize(&modified, &completed, &catalog).is_err());
    }

    #[test]
    fn invalidation_receipts_are_exactly_runner_owned() {
        let metadata = AdaptivePlannerMetadataV1 {
            scenario_id: "scenario-1".into(),
            objective: "recover".into(),
            reference_checks: Vec::new(),
            invalidation: AdaptivePlannerInvalidationV1 {
                description: "changed".into(),
                evidence_ids: vec!["validation/v1".into()],
            },
        };
        let policy_sha256 = crate::artifact::sha256_bytes(b"policy");
        let mut plans = bind_agent_document(
            parse_agent_plan_document(&document_json()).unwrap(),
            &policy_sha256,
        )
        .unwrap();
        validate_invalidation_binding(&plans, &metadata).unwrap();
        plans[1].evidence_ids.push("invented/v1".into());
        assert!(validate_invalidation_binding(&plans, &metadata).is_err());
    }
}
