use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::wire::{
    FunctionPolicy, MessageInput, SendOptions, SendRequest, SendResponse, SessionInit,
};

use super::{
    ControlSource, PortValueKind, ReplayPolicy, RequiredFunctionContract, StepCatalog,
    StepEvaluation, StepExecutor, StepExecutorContext, StepExecutorOutput, StepOperationalKind,
    StepPortDescriptor, StepTypeDescriptor, TypedPortValue, WorkflowGateResult,
};

pub const HARNESS_STEP_ID: &str = "harness.prompt";
pub const HARNESS_STEP_VERSION: u32 = 1;
pub const HARNESS_STEP_VERSION_V2: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessStepConfig {
    pub prompt: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default = "default_max_total_tokens")]
    pub max_total_tokens: u64,
    #[serde(default = "default_stuck_timeout_seconds")]
    pub stuck_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessStepConfigV2 {
    pub prompt: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default = "default_max_total_tokens")]
    pub max_total_tokens: u64,
    #[serde(default = "default_stuck_timeout_seconds")]
    pub stuck_timeout_seconds: u64,
    #[serde(default)]
    pub function_allow: Vec<String>,
    #[serde(default)]
    pub function_deny: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HarnessStepPolicy {
    approved_roots: Vec<PathBuf>,
    mandatory_denials: Vec<String>,
}

impl HarnessStepPolicy {
    pub fn new(
        approved_roots: impl IntoIterator<Item = PathBuf>,
        mandatory_denials: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let approved_roots = approved_roots
            .into_iter()
            .map(|root| {
                if !root.is_absolute() {
                    bail!(
                        "approved Harness workspace root must be absolute: {}",
                        root.display()
                    );
                }
                root.canonicalize().with_context(|| {
                    format!(
                        "canonicalize approved Harness workspace root {}",
                        root.display()
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if approved_roots.is_empty() {
            bail!("Harness prompt v2 requires at least one approved workspace root");
        }
        let mandatory_denials = normalized_patterns(mandatory_denials, "mandatory denial")?;
        Ok(Self {
            approved_roots,
            mandatory_denials,
        })
    }

    fn workspace_metadata(&self, requested: Option<&str>) -> Result<Option<Value>> {
        let Some(requested) = requested else {
            return Ok(None);
        };
        let requested = Path::new(requested);
        if !requested.is_absolute() {
            bail!("Harness prompt workspace_root must be absolute");
        }
        let canonical = requested.canonicalize().with_context(|| {
            format!(
                "canonicalize Harness workspace root {}",
                requested.display()
            )
        })?;
        if canonical != requested {
            bail!("Harness prompt workspace_root must already be canonical");
        }
        if !self
            .approved_roots
            .iter()
            .any(|approved| canonical == *approved || canonical.starts_with(approved))
        {
            bail!("Harness prompt workspace_root is outside the runtime-approved filesystem roots");
        }
        let canonical = canonical
            .to_str()
            .context("Harness prompt workspace_root is not valid UTF-8")?;
        Ok(Some(json!({ "fs_scope": { "root": canonical } })))
    }

    fn function_policy(&self, config: &HarnessStepConfigV2) -> Result<FunctionPolicy> {
        let allow = normalized_patterns(config.function_allow.clone(), "function allow")?;
        let deny = normalized_patterns(
            config
                .function_deny
                .iter()
                .cloned()
                .chain(self.mandatory_denials.iter().cloned())
                .chain([
                    "e2e::*".to_string(),
                    "incident-fixture::reset".to_string(),
                    "incident-fixture::deploy".to_string(),
                ]),
            "function denial",
        )?;
        Ok(FunctionPolicy {
            allow,
            deny,
            ..FunctionPolicy::default()
        })
    }
}

fn normalized_patterns(
    patterns: impl IntoIterator<Item = String>,
    label: &str,
) -> Result<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            bail!("Harness prompt {label} contains an empty pattern");
        }
        normalized.insert(pattern.to_string());
    }
    Ok(normalized.into_iter().collect())
}

fn default_max_turns() -> u32 {
    100
}

fn default_max_total_tokens() -> u64 {
    100_000
}

fn default_stuck_timeout_seconds() -> u64 {
    300
}

struct HarnessStepExecutor {
    context: Arc<E2eContext>,
    model: String,
    provider: String,
    sessions: Mutex<HashMap<(String, String), String>>,
}

struct HarnessStepExecutorV2 {
    context: Arc<E2eContext>,
    model: String,
    provider: String,
    policy: HarnessStepPolicy,
    sessions: Mutex<HashMap<(String, String), String>>,
}

pub fn register_harness_step(
    catalog: &mut StepCatalog,
    context: Arc<E2eContext>,
    model: impl Into<String>,
    provider: impl Into<String>,
) -> Result<()> {
    catalog.register(
        harness_descriptor()?,
        Arc::new(HarnessStepExecutor {
            context,
            model: model.into(),
            provider: provider.into(),
            sessions: Mutex::new(HashMap::new()),
        }),
    )
}

pub fn register_harness_step_v2(
    catalog: &mut StepCatalog,
    context: Arc<E2eContext>,
    model: impl Into<String>,
    provider: impl Into<String>,
    policy: HarnessStepPolicy,
) -> Result<()> {
    catalog.register(
        harness_descriptor_v2()?,
        Arc::new(HarnessStepExecutorV2 {
            context,
            model: model.into(),
            provider: provider.into(),
            policy,
            sessions: Mutex::new(HashMap::new()),
        }),
    )
}

pub fn harness_descriptor() -> Result<StepTypeDescriptor> {
    let schema = schemars::schema_for!(HarnessStepConfig);
    Ok(StepTypeDescriptor {
        id: HARNESS_STEP_ID.into(),
        version: HARNESS_STEP_VERSION,
        description: "Start one independent Harness session with bounded untrusted inputs.".into(),
        config_schema: serde_json::to_value(schema)?,
        inputs: BTreeMap::from([(
            "data".into(),
            StepPortDescriptor {
                kind: PortValueKind::Json,
                optional: true,
                control_source: None,
            },
        )]),
        outputs: BTreeMap::from([(
            "completed".into(),
            StepPortDescriptor {
                kind: PortValueKind::Boolean,
                optional: false,
                control_source: Some(ControlSource::Deterministic),
            },
        )]),
        capabilities: vec!["harness::independent_session".into()],
        required_functions: [
            "harness::send",
            "harness::status",
            "harness::session-tree",
            "harness::metrics",
            "harness::stop",
            "harness::teardown",
        ]
        .into_iter()
        .map(|function_id| RequiredFunctionContract {
            function_id: function_id.into(),
            request_schema_sha256: None,
            response_schema_sha256: None,
        })
        .collect(),
        replay_policy: ReplayPolicy::NonRepeatable,
        operational_kind: StepOperationalKind::Harness,
    })
}

pub fn harness_descriptor_v2() -> Result<StepTypeDescriptor> {
    let schema = schemars::schema_for!(HarnessStepConfigV2);
    Ok(StepTypeDescriptor {
        id: HARNESS_STEP_ID.into(),
        version: HARNESS_STEP_VERSION_V2,
        description:
            "Start one independent Harness session with runtime-approved filesystem and function boundaries."
                .into(),
        config_schema: serde_json::to_value(schema)?,
        inputs: BTreeMap::from([
            (
                "data".into(),
                StepPortDescriptor {
                    kind: PortValueKind::Json,
                    optional: true,
                    control_source: None,
                },
            ),
            (
                "workspace_root".into(),
                StepPortDescriptor {
                    kind: PortValueKind::TextUtf8,
                    optional: true,
                    control_source: None,
                },
            ),
        ]),
        outputs: BTreeMap::from([(
            "completed".into(),
            StepPortDescriptor {
                kind: PortValueKind::Boolean,
                optional: false,
                control_source: Some(ControlSource::Deterministic),
            },
        )]),
        capabilities: vec!["harness::independent_session".into()],
        required_functions: harness_required_functions(),
        replay_policy: ReplayPolicy::NonRepeatable,
        operational_kind: StepOperationalKind::Harness,
    })
}

fn harness_required_functions() -> Vec<RequiredFunctionContract> {
    [
        "harness::send",
        "harness::status",
        "harness::session-tree",
        "harness::metrics",
        "harness::stop",
        "harness::teardown",
    ]
    .into_iter()
    .map(|function_id| RequiredFunctionContract {
        function_id: function_id.into(),
        request_schema_sha256: None,
        response_schema_sha256: None,
    })
    .collect()
}

#[async_trait]
impl StepExecutor for HarnessStepExecutor {
    async fn preflight(&self, context: &StepExecutorContext) -> Result<()> {
        let config = decode_config(context)?;
        if config.prompt.trim().is_empty()
            || config.max_turns == 0
            || config.max_total_tokens == 0
            || config.stuck_timeout_seconds == 0
        {
            bail!("Harness step prompt and limits must be non-empty and positive");
        }
        for function in [
            "harness::send",
            "harness::status",
            "harness::session-tree",
            "harness::metrics",
            "harness::stop",
            "harness::teardown",
        ] {
            if !self.context.function_exists(function).await? {
                bail!("required Harness function '{function}' is unavailable");
            }
        }
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        let config = decode_config(&context)?;
        let session_id = format!("e2e_{}_{}", context.attempt_id, context.node.id);
        let structured_input = context.inputs.get("data").map(|input| input.value.clone());
        let message = match structured_input {
            Some(input) => format!(
                "{}\n\n<untrusted_workflow_input media_type=\"application/json\">\n{}\n</untrusted_workflow_input>",
                config.prompt,
                serde_json::to_string(&input).context("encode bounded workflow input")?
            ),
            None => config.prompt.clone(),
        };
        let response: SendResponse = self
            .context
            .trigger(
                "harness::send",
                SendRequest {
                    session_id: Some(session_id.clone()),
                    message: MessageInput::Text(message),
                    model: Some(self.model.clone()),
                    provider: Some(self.provider.clone()),
                    idempotency_key: Some(format!(
                        "e2e:{}:{}:{}",
                        context.run_id, context.attempt_id, context.node.id
                    )),
                    session: Some(SessionInit {
                        title: Some(format!("Harness E2E workflow: {}", context.node.id)),
                        metadata: Some(json!({
                            "e2e_workflow_id": context.workflow_id,
                            "e2e_workflow_sha256": context.workflow_sha256,
                            "e2e_run_id": context.run_id,
                            "e2e_attempt_id": context.attempt_id,
                            "e2e_step_id": context.node.id,
                        })),
                    }),
                    options: Some(SendOptions {
                        max_turns: Some(config.max_turns),
                        max_output_tokens: config.max_output_tokens,
                        max_total_tokens: Some(config.max_total_tokens),
                        functions: Some(FunctionPolicy {
                            allow: vec!["*".into()],
                            deny: vec!["e2e::*".into()],
                            ..FunctionPolicy::default()
                        }),
                        metadata: None,
                    }),
                },
            )
            .await?;
        if !response.accepted
            || response.session_id != session_id
            || response.merged == Some(true)
            || response.queued == Some(true)
        {
            bail!("harness::send returned an unexpected response: {response:?}");
        }
        self.lock_sessions().insert(
            (context.attempt_id.clone(), context.node.id.clone()),
            session_id.clone(),
        );
        let metrics = self
            .context
            .wait_for_tree(
                &context.node.id,
                &session_id,
                Duration::from_secs(config.stuck_timeout_seconds),
                false,
                Some(&context.cancellation),
            )
            .await?;
        let transcript = self.context.transcript(&session_id).await?;
        let cost_usd = metrics.totals.cost_usd;
        Ok(StepExecutorOutput {
            outputs: BTreeMap::from([(
                "completed".into(),
                TypedPortValue {
                    kind: PortValueKind::Boolean,
                    value: Value::Bool(true),
                },
            )]),
            transcript: Some(transcript),
            metrics: Some(serde_json::to_value(&metrics)?),
            cost_usd,
            harness_session_id: Some(session_id),
            ..StepExecutorOutput::default()
        })
    }

    async fn evaluate(
        &self,
        _context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[super::CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        Ok(StepEvaluation {
            hard_gates: vec![WorkflowGateResult {
                id: "harness_session_completed".into(),
                passed: execution.harness_session_id.is_some(),
                reason: "The independent Harness session completed and its metrics were captured."
                    .into(),
                evidence_ids: Vec::new(),
            }],
            evaluations: Vec::new(),
        })
    }

    async fn cancel(&self, context: &StepExecutorContext) -> Result<()> {
        let session = self
            .lock_sessions()
            .get(&(context.attempt_id.clone(), context.node.id.clone()))
            .cloned();
        if let Some(session) = session {
            self.context.stop_session(&session, None).await?;
        }
        Ok(())
    }

    async fn cleanup(&self, context: &StepExecutorContext) -> Result<()> {
        let session = self
            .lock_sessions()
            .remove(&(context.attempt_id.clone(), context.node.id.clone()));
        if let Some(session) = session {
            self.context.teardown(&session).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl StepExecutor for HarnessStepExecutorV2 {
    async fn preflight(&self, context: &StepExecutorContext) -> Result<()> {
        let config = decode_config_v2(context)?;
        validate_v2_config(&config)?;
        let workspace_root = optional_text_input(context, "workspace_root")?;
        self.policy.workspace_metadata(workspace_root)?;
        self.policy.function_policy(&config)?;
        for function in [
            "harness::send",
            "harness::status",
            "harness::session-tree",
            "harness::metrics",
            "harness::stop",
            "harness::teardown",
        ] {
            if !self.context.function_exists(function).await? {
                bail!("required Harness function '{function}' is unavailable");
            }
        }
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        let config = decode_config_v2(&context)?;
        validate_v2_config(&config)?;
        let workspace_root = optional_text_input(&context, "workspace_root")?;
        let metadata = self.policy.workspace_metadata(workspace_root)?;
        let functions = self.policy.function_policy(&config)?;
        let session_id = format!("e2e_{}_{}", context.attempt_id, context.node.id);
        let structured_input = context.inputs.get("data").map(|input| input.value.clone());
        let message = match structured_input {
            Some(input) => format!(
                "{}\n\n<untrusted_workflow_input media_type=\"application/json\">\n{}\n</untrusted_workflow_input>",
                config.prompt,
                serde_json::to_string(&input).context("encode bounded workflow input")?
            ),
            None => config.prompt.clone(),
        };
        let response: SendResponse = self
            .context
            .trigger(
                "harness::send",
                SendRequest {
                    session_id: Some(session_id.clone()),
                    message: MessageInput::Text(message),
                    model: Some(self.model.clone()),
                    provider: Some(self.provider.clone()),
                    idempotency_key: Some(format!(
                        "e2e:{}:{}:{}",
                        context.run_id, context.attempt_id, context.node.id
                    )),
                    session: Some(SessionInit {
                        title: Some(format!("Harness E2E workflow: {}", context.node.id)),
                        metadata: Some(json!({
                            "e2e_workflow_id": context.workflow_id,
                            "e2e_workflow_sha256": context.workflow_sha256,
                            "e2e_run_id": context.run_id,
                            "e2e_attempt_id": context.attempt_id,
                            "e2e_step_id": context.node.id,
                        })),
                    }),
                    options: Some(SendOptions {
                        max_turns: Some(config.max_turns),
                        max_output_tokens: config.max_output_tokens,
                        max_total_tokens: Some(config.max_total_tokens),
                        functions: Some(functions),
                        metadata,
                    }),
                },
            )
            .await?;
        if !response.accepted
            || response.session_id != session_id
            || response.merged == Some(true)
            || response.queued == Some(true)
        {
            bail!("harness::send returned an unexpected response: {response:?}");
        }
        self.lock_sessions().insert(
            (context.attempt_id.clone(), context.node.id.clone()),
            session_id.clone(),
        );
        let metrics = self
            .context
            .wait_for_tree(
                &context.node.id,
                &session_id,
                Duration::from_secs(config.stuck_timeout_seconds),
                false,
                Some(&context.cancellation),
            )
            .await?;
        let transcript = self.context.transcript(&session_id).await?;
        let cost_usd = metrics.totals.cost_usd;
        Ok(StepExecutorOutput {
            outputs: BTreeMap::from([(
                "completed".into(),
                TypedPortValue {
                    kind: PortValueKind::Boolean,
                    value: Value::Bool(true),
                },
            )]),
            transcript: Some(transcript),
            metrics: Some(serde_json::to_value(&metrics)?),
            cost_usd,
            harness_session_id: Some(session_id),
            ..StepExecutorOutput::default()
        })
    }

    async fn evaluate(
        &self,
        _context: &StepExecutorContext,
        execution: &StepExecutorOutput,
        _assets: &[super::CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        Ok(StepEvaluation {
            hard_gates: vec![WorkflowGateResult {
                id: "harness_session_completed".into(),
                passed: execution.harness_session_id.is_some(),
                reason: "The policy-scoped independent Harness session completed and its metrics were captured."
                    .into(),
                evidence_ids: Vec::new(),
            }],
            evaluations: Vec::new(),
        })
    }

    async fn cancel(&self, context: &StepExecutorContext) -> Result<()> {
        let session = self
            .lock_sessions()
            .get(&(context.attempt_id.clone(), context.node.id.clone()))
            .cloned();
        if let Some(session) = session {
            self.context.stop_session(&session, None).await?;
        }
        Ok(())
    }

    async fn cleanup(&self, context: &StepExecutorContext) -> Result<()> {
        let session = self
            .lock_sessions()
            .remove(&(context.attempt_id.clone(), context.node.id.clone()));
        if let Some(session) = session {
            self.context.teardown(&session).await?;
        }
        Ok(())
    }
}

impl HarnessStepExecutor {
    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), String>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl HarnessStepExecutorV2 {
    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), String>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn decode_config(context: &StepExecutorContext) -> Result<HarnessStepConfig> {
    serde_json::from_value(context.node.config.clone()).with_context(|| {
        format!(
            "decode Harness configuration for node '{}'",
            context.node.id
        )
    })
}

fn decode_config_v2(context: &StepExecutorContext) -> Result<HarnessStepConfigV2> {
    serde_json::from_value(context.node.config.clone()).with_context(|| {
        format!(
            "decode Harness v2 configuration for node '{}'",
            context.node.id
        )
    })
}

fn validate_v2_config(config: &HarnessStepConfigV2) -> Result<()> {
    if config.prompt.trim().is_empty()
        || config.max_turns == 0
        || config.max_total_tokens == 0
        || config.stuck_timeout_seconds == 0
        || config.max_output_tokens == Some(0)
        || config
            .max_output_tokens
            .is_some_and(|maximum| maximum > config.max_total_tokens)
    {
        bail!("Harness v2 step prompt and limits must be non-empty, positive, and ordered");
    }
    normalized_patterns(config.function_allow.clone(), "function allow")?;
    normalized_patterns(config.function_deny.clone(), "function denial")?;
    Ok(())
}

fn optional_text_input<'a>(context: &'a StepExecutorContext, id: &str) -> Result<Option<&'a str>> {
    context
        .inputs
        .get(id)
        .map(|value| {
            value
                .value
                .as_str()
                .with_context(|| format!("workflow input '{id}' must be text_utf8"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_step_does_not_accept_function_ids_and_is_not_retry_safe() {
        let descriptor = harness_descriptor().unwrap();
        assert_eq!(descriptor.replay_policy, ReplayPolicy::NonRepeatable);
        let schema = descriptor.config_schema.to_string();
        assert!(!schema.contains("function_id"));
        assert!(!schema.contains("denied_functions"));
    }

    #[test]
    fn harness_step_v2_declares_scoped_workspace_and_code_owned_policy() {
        let descriptor = harness_descriptor_v2().unwrap();
        assert_eq!(descriptor.version, HARNESS_STEP_VERSION_V2);
        assert_eq!(descriptor.replay_policy, ReplayPolicy::NonRepeatable);
        assert_eq!(
            descriptor.inputs["workspace_root"].kind,
            PortValueKind::TextUtf8
        );
        let schema = descriptor.config_schema.to_string();
        assert!(schema.contains("function_allow"));
        assert!(schema.contains("function_deny"));
        assert!(!schema.contains("workspace_root"));
    }

    #[test]
    fn v2_policy_enforces_approved_canonical_roots_and_mandatory_denials() {
        let approved = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let approved_path = approved.path().canonicalize().unwrap();
        let outside_path = outside.path().canonicalize().unwrap();
        let policy =
            HarnessStepPolicy::new([approved_path.clone()], ["fixture::mutate".to_string()])
                .unwrap();
        assert_eq!(
            policy
                .workspace_metadata(approved_path.to_str())
                .unwrap()
                .unwrap()
                .pointer("/fs_scope/root")
                .and_then(Value::as_str),
            approved_path.to_str()
        );
        assert!(policy
            .workspace_metadata(outside_path.to_str())
            .unwrap_err()
            .to_string()
            .contains("outside"));

        let functions = policy
            .function_policy(&HarnessStepConfigV2 {
                prompt: "analyze".into(),
                max_turns: 1,
                max_output_tokens: Some(10),
                max_total_tokens: 10,
                stuck_timeout_seconds: 1,
                function_allow: vec!["coder::read-file".into()],
                function_deny: vec!["state::*".into(), "state::*".into()],
            })
            .unwrap();
        assert_eq!(functions.allow, vec!["coder::read-file"]);
        assert!(functions.deny.contains(&"e2e::*".to_string()));
        assert!(functions
            .deny
            .contains(&"incident-fixture::reset".to_string()));
        assert!(functions.deny.contains(&"fixture::mutate".to_string()));
        assert_eq!(
            functions
                .deny
                .iter()
                .filter(|pattern| pattern.as_str() == "state::*")
                .count(),
            1
        );
    }
}
