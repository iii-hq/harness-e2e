use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::wire::{
    FunctionPolicy, HarnessMode, MessageInput, PermissionMode, SendOptions, SendRequest,
    SendResponse, SessionInit,
};

use super::{
    ControlSource, PortValueKind, ReplayPolicy, RequiredFunctionContract, StepCatalog,
    StepEvaluation, StepExecutor, StepExecutorContext, StepExecutorOutput, StepOperationalKind,
    StepPortDescriptor, StepTypeDescriptor, TypedPortValue, WorkflowGateResult,
};

pub const HARNESS_STEP_ID: &str = "harness.prompt";
pub const HARNESS_STEP_VERSION: u32 = 1;

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
                        mode: Some(HarnessMode::Agent),
                        max_turns: Some(config.max_turns),
                        max_output_tokens: config.max_output_tokens,
                        max_total_tokens: Some(config.max_total_tokens),
                        max_validation_retries: None,
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
                PermissionMode::Full,
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

impl HarnessStepExecutor {
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
}
