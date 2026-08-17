use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use super::{PortValueKind, StepTypeDescriptor, WorkflowNodeV1};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypedPortValue {
    pub kind: PortValueKind,
    pub value: Value,
}

impl TypedPortValue {
    pub fn validate(&self) -> Result<()> {
        if !self.kind.accepts_literal(&self.value) {
            bail!(
                "value is incompatible with declared port kind {:?}",
                self.kind
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", content = "content", rename_all = "snake_case")]
pub enum WorkflowAssetContent {
    Json(Value),
    TextUtf8(String),
}

impl WorkflowAssetContent {
    pub fn media_type_compatible(&self, media_type: &str) -> bool {
        match self {
            Self::Json(_) => media_type == "application/json" || media_type.ends_with("+json"),
            Self::TextUtf8(_) => {
                media_type.starts_with("text/") && media_type.to_ascii_lowercase().contains("utf-8")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowProvenance {
    pub source_step_id: String,
    pub relation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapturedWorkflowAsset {
    pub id: String,
    pub kind: String,
    pub media_type: String,
    pub content: WorkflowAssetContent,
    #[serde(default)]
    pub provenance: Vec<WorkflowProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowGateResult {
    pub id: String,
    pub passed: bool,
    pub reason: String,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEvaluationOutcome {
    Passed,
    Failed,
    Advisory,
    NotEvaluated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEvaluationResult {
    pub id: String,
    pub outcome: WorkflowEvaluationOutcome,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StepExecutorContext {
    pub workflow_id: String,
    pub workflow_sha256: String,
    pub run_id: String,
    pub attempt_id: String,
    pub node: WorkflowNodeV1,
    pub inputs: BTreeMap<String, TypedPortValue>,
    pub output_dir: PathBuf,
    pub cancellation: watch::Receiver<bool>,
}

#[derive(Debug, Clone)]
pub struct WorkflowCleanupContext {
    pub workflow_id: String,
    pub workflow_sha256: String,
    pub run_id: String,
    pub attempt_id: String,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct StepExecutorOutput {
    pub outputs: BTreeMap<String, TypedPortValue>,
    pub captured_assets: Vec<CapturedWorkflowAsset>,
    pub transcript: Option<Value>,
    pub metrics: Option<Value>,
    pub cost_usd: Option<f64>,
    pub harness_session_id: Option<String>,
    pub evaluation: StepEvaluation,
    pub technical_failure: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StepEvaluation {
    pub hard_gates: Vec<WorkflowGateResult>,
    pub evaluations: Vec<WorkflowEvaluationResult>,
}

#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn preflight(&self, _context: &StepExecutorContext) -> Result<()> {
        Ok(())
    }

    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput>;

    async fn capture(
        &self,
        _context: &StepExecutorContext,
        execution: &StepExecutorOutput,
    ) -> Result<Vec<CapturedWorkflowAsset>> {
        Ok(execution.captured_assets.clone())
    }

    async fn evaluate(
        &self,
        _context: &StepExecutorContext,
        _execution: &StepExecutorOutput,
        _assets: &[CapturedWorkflowAsset],
    ) -> Result<StepEvaluation> {
        Ok(StepEvaluation::default())
    }

    async fn cancel(&self, _context: &StepExecutorContext) -> Result<()> {
        Ok(())
    }

    async fn cleanup(&self, _context: &StepExecutorContext) -> Result<()> {
        Ok(())
    }

    async fn cleanup_workflow(&self, _context: &WorkflowCleanupContext) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RegisteredStepType {
    pub descriptor: StepTypeDescriptor,
    pub executor: Arc<dyn StepExecutor>,
}

#[derive(Clone, Default)]
pub struct StepCatalog {
    entries: HashMap<(String, u32), RegisteredStepType>,
}

impl StepCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        descriptor: StepTypeDescriptor,
        executor: Arc<dyn StepExecutor>,
    ) -> Result<()> {
        descriptor.validate()?;
        let key = (descriptor.id.clone(), descriptor.version);
        if self.entries.contains_key(&key) {
            bail!(
                "step type '{}@{}' is already registered",
                descriptor.id,
                descriptor.version
            );
        }
        self.entries.insert(
            key,
            RegisteredStepType {
                descriptor,
                executor,
            },
        );
        Ok(())
    }

    pub fn register_descriptor(&mut self, descriptor: StepTypeDescriptor) -> Result<()> {
        self.register(descriptor, Arc::new(DescriptorOnlyExecutor))
    }

    pub fn get(&self, id: &str, version: u32) -> Option<&RegisteredStepType> {
        self.entries.get(&(id.to_string(), version))
    }

    pub fn descriptors(&self) -> Vec<StepTypeDescriptor> {
        let mut descriptors = self
            .entries
            .values()
            .map(|registered| registered.descriptor.clone())
            .collect::<Vec<_>>();
        descriptors.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.version.cmp(&right.version))
        });
        descriptors
    }
}

struct DescriptorOnlyExecutor;

#[async_trait]
impl StepExecutor for DescriptorOnlyExecutor {
    async fn execute(&self, context: StepExecutorContext) -> Result<StepExecutorOutput> {
        bail!(
            "step type '{}@{}' was registered for validation only",
            context.node.step_type,
            context.node.step_version
        )
    }
}
