mod builtin;
mod catalog;
pub mod incident_response;
mod run;
mod scheduler;
pub mod security_scan;

use std::sync::Arc;

pub use builtin::{
    harness_descriptor, harness_descriptor_v2, register_harness_step, register_harness_step_v2,
    HarnessStepConfig, HarnessStepConfigV2, HarnessStepPolicy, HARNESS_STEP_ID,
    HARNESS_STEP_VERSION, HARNESS_STEP_VERSION_V2,
};
pub use catalog::{
    CapturedWorkflowAsset, NoopWorkflowCleanupHook, RegisteredStepType, StepCatalog,
    StepEvaluation, StepExecutor, StepExecutorContext, StepExecutorOutput, TypedPortValue,
    WorkflowAssetContent, WorkflowCleanupContext, WorkflowCleanupHook, WorkflowEvaluationOutcome,
    WorkflowEvaluationResult, WorkflowGateResult, WorkflowProvenance,
};
pub(crate) use run::observe_worker_contracts;
pub use scheduler::{
    execute_workflow, CheckpointStore, WorkflowAssetReport, WorkflowAttemptReport,
    WorkflowCheckpointV1, WorkflowCleanupReport, WorkflowCleanupStatus, WorkflowCriterionResult,
    WorkflowExecutionRequest, WorkflowFailurePhase, WorkflowStepFailure, WorkflowStepReport,
    WorkflowStepStatus,
};

use crate::context::E2eContext;
use crate::scenarios::ScenarioId;

/// Runtime materialized entirely from Rust for one composite scenario attempt.
/// The definition is retained only to drive this in-process scheduler and to
/// produce a non-executable evidence snapshot.
pub struct CompositeScenarioRuntime {
    pub definition: WorkflowDefinitionV1,
    pub catalog: Arc<StepCatalog>,
    pub cleanup_hook: Arc<dyn WorkflowCleanupHook>,
}

/// Return the Rust-owned definition for a composite scenario. Adding a future
/// sequential scenario requires registering it here and implementing its step
/// catalog; no JSON definition is loaded or accepted by the runner.
pub fn composite_definition(scenario: ScenarioId) -> Option<WorkflowDefinitionV1> {
    match scenario {
        ScenarioId::SecurityReview => Some(security_scan::definition()),
        ScenarioId::IncidentResponse => Some(incident_response::definition()),
        _ => None,
    }
}

/// Build a descriptor-only catalog for whole-suite contract preflight. These
/// entries cannot execute and exist only to validate Rust definitions and the
/// exact worker contracts they declare.
pub fn composite_descriptor_catalog(scenarios: &[ScenarioId]) -> Result<StepCatalog> {
    let mut catalog = StepCatalog::new();
    for scenario in scenarios {
        let Some(definition) = composite_definition(*scenario) else {
            continue;
        };
        if definition.nodes.iter().any(|node| {
            node.step_type == builtin::HARNESS_STEP_ID
                && node.step_version == builtin::HARNESS_STEP_VERSION
        }) && catalog
            .get(builtin::HARNESS_STEP_ID, builtin::HARNESS_STEP_VERSION)
            .is_none()
        {
            catalog.register_descriptor(harness_descriptor()?)?;
        }
        if definition.nodes.iter().any(|node| {
            node.step_type == builtin::HARNESS_STEP_ID
                && node.step_version == builtin::HARNESS_STEP_VERSION_V2
        }) && catalog
            .get(builtin::HARNESS_STEP_ID, builtin::HARNESS_STEP_VERSION_V2)
            .is_none()
        {
            catalog.register_descriptor(harness_descriptor_v2()?)?;
        }
        if scenario == &ScenarioId::SecurityReview {
            for descriptor in security_scan::descriptors_only() {
                if catalog.get(&descriptor.id, descriptor.version).is_none() {
                    catalog.register_descriptor(descriptor)?;
                }
            }
        }
        if scenario == &ScenarioId::IncidentResponse {
            for descriptor in incident_response::descriptors_only()? {
                if catalog.get(&descriptor.id, descriptor.version).is_none() {
                    catalog.register_descriptor(descriptor)?;
                }
            }
        }
    }
    Ok(catalog)
}

/// Materialize an executable runtime for a single attempt. Runtime state is not
/// shared across attempts, which keeps cleanup ownership exact and retry-safe.
pub fn composite_runtime(
    scenario: ScenarioId,
    context: Arc<E2eContext>,
    model: &str,
    provider: &str,
) -> Result<CompositeScenarioRuntime> {
    let definition = composite_definition(scenario)
        .with_context(|| format!("scenario '{}' is not composite", scenario.as_str()))?;
    let mut catalog = StepCatalog::new();
    if definition.nodes.iter().any(|node| {
        node.step_type == builtin::HARNESS_STEP_ID
            && node.step_version == builtin::HARNESS_STEP_VERSION
    }) {
        register_harness_step(&mut catalog, context.clone(), model, provider)?;
    }
    if definition.nodes.iter().any(|node| {
        node.step_type == builtin::HARNESS_STEP_ID
            && node.step_version == builtin::HARNESS_STEP_VERSION_V2
    }) {
        register_harness_step_v2(
            &mut catalog,
            context.clone(),
            model,
            provider,
            incident_response::harness_policy()?,
        )?;
    }
    let cleanup_hook = match scenario {
        ScenarioId::SecurityReview => {
            security_scan::register_security_scan_steps(&mut catalog, context)?
        }
        ScenarioId::IncidentResponse => {
            incident_response::register_incident_response_steps(&mut catalog, context)?
        }
        _ => bail!("scenario '{}' has no composite runtime", scenario.as_str()),
    };
    definition.validate(&catalog)?;
    Ok(CompositeScenarioRuntime {
        definition,
        catalog: Arc::new(catalog),
        cleanup_hook,
    })
}

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORKFLOW_SCHEMA_VERSION: u32 = 1;
pub const LOCAL_MAX_PARALLELISM: u16 = 16;
pub const LOCAL_MAX_NODES: u16 = 256;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinitionV1 {
    pub schema_version: u32,
    pub id: String,
    pub scenario_version: u32,
    pub description: String,
    #[serde(default)]
    pub limits: WorkflowLimits,
    pub nodes: Vec<WorkflowNodeV1>,
    #[serde(default)]
    pub criteria: Vec<WorkflowCriterionDeclaration>,
}

impl WorkflowDefinitionV1 {
    pub fn canonical_sha256(&self) -> Result<String> {
        crate::artifact::sha256_value(self)
    }

    pub fn validate(&self, catalog: &StepCatalog) -> Result<MaterializedWorkflow> {
        validate_identifier(&self.id, "workflow id")?;
        if self.schema_version != WORKFLOW_SCHEMA_VERSION {
            bail!(
                "workflow '{}' uses schema_version {}; supported version is {}",
                self.id,
                self.schema_version,
                WORKFLOW_SCHEMA_VERSION
            );
        }
        if self.scenario_version == 0 {
            bail!("workflow '{}' scenario_version must be positive", self.id);
        }
        if self.description.trim().is_empty() {
            bail!("workflow '{}' description cannot be empty", self.id);
        }
        self.limits.validate(self.nodes.len())?;
        if self.nodes.is_empty() {
            bail!("workflow '{}' must declare at least one node", self.id);
        }

        let mut nodes = HashMap::with_capacity(self.nodes.len());
        for node in &self.nodes {
            validate_identifier(&node.id, "node id")?;
            if nodes.insert(node.id.as_str(), node).is_some() {
                bail!("workflow '{}' has duplicate node id '{}'", self.id, node.id);
            }
            if node.required && !matches!(node.activation, ActivationPolicy::Always) {
                bail!(
                    "required node '{}' cannot be conditional; conditional nodes must be optional",
                    node.id
                );
            }
        }

        let mut descriptors = HashMap::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let registered = catalog
                .get(&node.step_type, node.step_version)
                .with_context(|| {
                    format!(
                        "node '{}' references unregistered step type '{}@{}'",
                        node.id, node.step_type, node.step_version
                    )
                })?;
            registered
                .descriptor
                .validate_config(&node.config)
                .with_context(|| format!("validate configuration for node '{}'", node.id))?;
            descriptors.insert(node.id.as_str(), &registered.descriptor);
        }

        let ancestors = validate_dependencies_and_order(&self.nodes, &nodes)?;
        for node in &self.nodes {
            validate_node_bindings(node, &nodes, &descriptors, &ancestors)?;
            validate_activation(node, &nodes, &descriptors, &ancestors)?;
        }
        validate_criteria(self, &nodes, &descriptors)?;
        validate_retry_safety(self, &descriptors)?;

        let mut topological_order = ancestors.keys().copied().collect::<Vec<_>>();
        topological_order.sort_by(|left, right| {
            ancestors[left]
                .len()
                .cmp(&ancestors[right].len())
                .then_with(|| left.cmp(right))
        });
        Ok(MaterializedWorkflow {
            definition: self.clone(),
            sha256: self.canonical_sha256()?,
            topological_order: topological_order.into_iter().map(str::to_string).collect(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLimits {
    #[serde(default = "default_max_parallel")]
    pub max_parallel: u16,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u16,
    #[serde(default = "default_step_timeout_seconds")]
    pub step_timeout_seconds: u64,
    #[serde(default = "default_workflow_timeout_seconds")]
    pub workflow_timeout_seconds: u64,
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    #[serde(default)]
    pub technical_retries: u8,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            max_parallel: default_max_parallel(),
            max_nodes: default_max_nodes(),
            step_timeout_seconds: default_step_timeout_seconds(),
            workflow_timeout_seconds: default_workflow_timeout_seconds(),
            max_total_tokens: None,
            max_cost_usd: None,
            technical_retries: 0,
        }
    }
}

impl WorkflowLimits {
    fn validate(&self, node_count: usize) -> Result<()> {
        if self.max_parallel == 0 || self.max_parallel > LOCAL_MAX_PARALLELISM {
            bail!("max_parallel must be between 1 and the local limit {LOCAL_MAX_PARALLELISM}");
        }
        if self.max_nodes == 0 || self.max_nodes > LOCAL_MAX_NODES {
            bail!("max_nodes must be between 1 and {LOCAL_MAX_NODES}");
        }
        if node_count > usize::from(self.max_nodes) {
            bail!(
                "workflow declares {node_count} nodes but max_nodes is {}",
                self.max_nodes
            );
        }
        if self.step_timeout_seconds == 0 || self.workflow_timeout_seconds == 0 {
            bail!("step and workflow timeouts must be positive");
        }
        if self.step_timeout_seconds > self.workflow_timeout_seconds {
            bail!("step timeout cannot exceed workflow timeout");
        }
        if self.max_total_tokens == Some(0) {
            bail!("max_total_tokens must be positive when set");
        }
        if self
            .max_cost_usd
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            bail!("max_cost_usd must be finite and positive when set");
        }
        Ok(())
    }
}

fn default_max_parallel() -> u16 {
    4
}

fn default_max_nodes() -> u16 {
    64
}

fn default_step_timeout_seconds() -> u64 {
    300
}

fn default_workflow_timeout_seconds() -> u64 {
    1_800
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeV1 {
    pub id: String,
    pub step_type: String,
    pub step_version: u32,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, WorkflowInputBinding>,
    #[serde(default)]
    pub activation: ActivationPolicy,
    #[serde(default)]
    pub dependency_policy: DependencyPolicy,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowInputBinding {
    Literal { kind: PortValueKind, value: Value },
    Output { node_id: String, port: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "policy", content = "conditions", rename_all = "snake_case")]
pub enum ActivationPolicy {
    #[default]
    Always,
    All(Vec<BooleanCondition>),
    Any(Vec<BooleanCondition>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BooleanCondition {
    pub node_id: String,
    pub port: String,
    pub equals: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DependencyPolicy {
    #[default]
    Succeeded,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortValueKind {
    Boolean,
    Json,
    TextUtf8,
    Assessment,
}

impl PortValueKind {
    pub fn accepts_literal(self, value: &Value) -> bool {
        match self {
            Self::Boolean => value.is_boolean(),
            Self::TextUtf8 => value.is_string(),
            Self::Json | Self::Assessment => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCriterionDeclaration {
    pub id: String,
    pub weight: u8,
    pub producer_node_id: String,
    pub output_port: String,
    #[serde(default)]
    pub advisory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepPortDescriptor {
    pub kind: PortValueKind,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub control_source: Option<ControlSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlSource {
    Deterministic,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPolicy {
    Idempotent,
    Compensable,
    NonRepeatable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepOperationalKind {
    Harness,
    Product,
    Assessment,
    Transformation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredFunctionContract {
    pub function_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_schema_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepTypeDescriptor {
    pub id: String,
    pub version: u32,
    pub description: String,
    pub config_schema: Value,
    #[serde(default)]
    pub inputs: BTreeMap<String, StepPortDescriptor>,
    #[serde(default)]
    pub outputs: BTreeMap<String, StepPortDescriptor>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub required_functions: Vec<RequiredFunctionContract>,
    pub replay_policy: ReplayPolicy,
    pub operational_kind: StepOperationalKind,
}

impl StepTypeDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.id, "step type id")?;
        if self.version == 0 {
            bail!("step type '{}' version must be positive", self.id);
        }
        if self.description.trim().is_empty() {
            bail!("step type '{}' description cannot be empty", self.id);
        }
        jsonschema::JSONSchema::compile(&self.config_schema)
            .map_err(|error| anyhow::anyhow!("invalid config schema for '{}': {error}", self.id))?;
        validate_ports(&self.id, &self.inputs)?;
        validate_ports(&self.id, &self.outputs)?;
        for required in &self.required_functions {
            if required.function_id.trim().is_empty() {
                bail!("step type '{}' has an empty required function id", self.id);
            }
        }
        Ok(())
    }

    pub fn validate_config(&self, config: &Value) -> Result<()> {
        let validator = jsonschema::JSONSchema::compile(&self.config_schema)
            .map_err(|error| anyhow::anyhow!("compile config schema for '{}': {error}", self.id))?;
        if let Err(errors) = validator.validate(config) {
            bail!(
                "configuration does not match '{}@{}': {}",
                self.id,
                self.version,
                errors
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MaterializedWorkflow {
    pub definition: WorkflowDefinitionV1,
    pub sha256: String,
    pub topological_order: Vec<String>,
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !valid || value == "." || value == ".." {
        bail!("{label} '{value}' must contain only ASCII letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

fn validate_ports(step_id: &str, ports: &BTreeMap<String, StepPortDescriptor>) -> Result<()> {
    for (name, descriptor) in ports {
        validate_identifier(name, "port name")
            .with_context(|| format!("step type '{step_id}' has an invalid port"))?;
        if descriptor.kind != PortValueKind::Boolean && descriptor.control_source.is_some() {
            bail!("step type '{step_id}' port '{name}' declares control_source but is not boolean");
        }
    }
    Ok(())
}

fn validate_dependencies_and_order<'a>(
    ordered_nodes: &'a [WorkflowNodeV1],
    nodes: &HashMap<&'a str, &'a WorkflowNodeV1>,
) -> Result<HashMap<&'a str, BTreeSet<&'a str>>> {
    let mut indegree = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in ordered_nodes {
        let mut unique = HashSet::new();
        for dependency in &node.depends_on {
            if dependency == &node.id {
                bail!("node '{}' cannot depend on itself", node.id);
            }
            if !nodes.contains_key(dependency.as_str()) {
                bail!(
                    "node '{}' references missing dependency '{}'",
                    node.id,
                    dependency
                );
            }
            if !unique.insert(dependency.as_str()) {
                bail!(
                    "node '{}' declares dependency '{}' more than once",
                    node.id,
                    dependency
                );
            }
            children
                .entry(dependency.as_str())
                .or_default()
                .push(node.id.as_str());
        }
        indegree.insert(node.id.as_str(), node.depends_on.len());
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect::<BTreeSet<_>>();
    let mut topo = Vec::with_capacity(ordered_nodes.len());
    while let Some(id) = ready.pop_first() {
        topo.push(id);
        if let Some(next) = children.get(id) {
            for child in next {
                let degree = indegree.get_mut(child).expect("known child");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(child);
                }
            }
        }
    }
    if topo.len() != ordered_nodes.len() {
        let cyclic = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id))
            .collect::<Vec<_>>();
        bail!("workflow contains a cycle involving: {}", cyclic.join(", "));
    }

    let mut ancestors: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for id in topo {
        let node = nodes[id];
        let mut inherited = BTreeSet::new();
        for dependency in &node.depends_on {
            inherited.insert(dependency.as_str());
            inherited.extend(ancestors[dependency.as_str()].iter().copied());
        }
        ancestors.insert(id, inherited);
    }
    Ok(ancestors)
}

fn validate_node_bindings(
    node: &WorkflowNodeV1,
    nodes: &HashMap<&str, &WorkflowNodeV1>,
    descriptors: &HashMap<&str, &StepTypeDescriptor>,
    ancestors: &HashMap<&str, BTreeSet<&str>>,
) -> Result<()> {
    let descriptor = descriptors[node.id.as_str()];
    for required in descriptor
        .inputs
        .iter()
        .filter_map(|(name, port)| (!port.optional).then_some(name))
    {
        if !node.inputs.contains_key(required) {
            bail!(
                "node '{}' is missing required input port '{}'",
                node.id,
                required
            );
        }
    }
    for (input_name, binding) in &node.inputs {
        let input = descriptor.inputs.get(input_name).with_context(|| {
            format!(
                "node '{}' binds undeclared input port '{}'",
                node.id, input_name
            )
        })?;
        match binding {
            WorkflowInputBinding::Literal { kind, value } => {
                if *kind != input.kind || !kind.accepts_literal(value) {
                    bail!(
                        "node '{}' input '{}' literal is incompatible with {:?}",
                        node.id,
                        input_name,
                        input.kind
                    );
                }
            }
            WorkflowInputBinding::Output {
                node_id,
                port: output_name,
            } => {
                let producer = nodes.get(node_id.as_str()).with_context(|| {
                    format!(
                        "node '{}' input '{}' references missing node '{}'",
                        node.id, input_name, node_id
                    )
                })?;
                if !ancestors[node.id.as_str()].contains(node_id.as_str()) {
                    bail!(
                        "node '{}' input '{}' may reference only an ancestor; '{}' is not one",
                        node.id,
                        input_name,
                        node_id
                    );
                }
                let output = descriptors[node_id.as_str()]
                    .outputs
                    .get(output_name)
                    .with_context(|| {
                        format!(
                            "node '{}' input '{}' references undeclared output '{}.{}'",
                            node.id, input_name, node_id, output_name
                        )
                    })?;
                if input.kind != output.kind {
                    bail!(
                        "node '{}' input '{}' expects {:?}, but '{}.{}' produces {:?}",
                        node.id,
                        input_name,
                        input.kind,
                        node_id,
                        output_name,
                        output.kind
                    );
                }
                if node.required && (!producer.required || output.optional) {
                    bail!(
                        "required node '{}' depends on optional output '{}.{}' without an alternative",
                        node.id,
                        node_id,
                        output_name
                    );
                }
            }
        }
    }
    Ok(())
}

fn validate_activation(
    node: &WorkflowNodeV1,
    nodes: &HashMap<&str, &WorkflowNodeV1>,
    descriptors: &HashMap<&str, &StepTypeDescriptor>,
    ancestors: &HashMap<&str, BTreeSet<&str>>,
) -> Result<()> {
    let conditions = match &node.activation {
        ActivationPolicy::Always => return Ok(()),
        ActivationPolicy::All(conditions) | ActivationPolicy::Any(conditions) => conditions,
    };
    if conditions.is_empty() {
        bail!("node '{}' has an empty conditional activation", node.id);
    }
    let mut expected = HashMap::new();
    for condition in conditions {
        if !nodes.contains_key(condition.node_id.as_str()) {
            bail!(
                "node '{}' condition references missing node '{}'",
                node.id,
                condition.node_id
            );
        }
        if !ancestors[node.id.as_str()].contains(condition.node_id.as_str()) {
            bail!(
                "node '{}' condition may reference only an ancestor; '{}' is not one",
                node.id,
                condition.node_id
            );
        }
        let output = descriptors[condition.node_id.as_str()]
            .outputs
            .get(&condition.port)
            .with_context(|| {
                format!(
                    "node '{}' condition references undeclared output '{}.{}'",
                    node.id, condition.node_id, condition.port
                )
            })?;
        if output.kind != PortValueKind::Boolean {
            bail!(
                "node '{}' condition output '{}.{}' is not boolean",
                node.id,
                condition.node_id,
                condition.port
            );
        }
        if output.control_source == Some(ControlSource::Ai) && node.required {
            bail!(
                "AI output '{}.{}' cannot activate required node '{}'",
                condition.node_id,
                condition.port,
                node.id
            );
        }
        let key = (condition.node_id.as_str(), condition.port.as_str());
        if expected
            .insert(key, condition.equals)
            .is_some_and(|seen| seen != condition.equals)
            && matches!(node.activation, ActivationPolicy::All(_))
        {
            bail!("node '{}' has contradictory activation conditions", node.id);
        }
    }
    Ok(())
}

fn validate_criteria(
    definition: &WorkflowDefinitionV1,
    nodes: &HashMap<&str, &WorkflowNodeV1>,
    descriptors: &HashMap<&str, &StepTypeDescriptor>,
) -> Result<()> {
    if definition.criteria.is_empty() {
        return Ok(());
    }
    let mut ids = HashSet::new();
    let mut total = 0_u16;
    for criterion in &definition.criteria {
        validate_identifier(&criterion.id, "criterion id")?;
        if !ids.insert(criterion.id.as_str()) {
            bail!("workflow has duplicate criterion id '{}'", criterion.id);
        }
        if criterion.weight == 0 {
            bail!(
                "workflow criterion '{}' weight must be positive",
                criterion.id
            );
        }
        total = total.saturating_add(u16::from(criterion.weight));
        if !nodes.contains_key(criterion.producer_node_id.as_str()) {
            bail!(
                "criterion '{}' references missing producer '{}'",
                criterion.id,
                criterion.producer_node_id
            );
        }
        let port = descriptors[criterion.producer_node_id.as_str()]
            .outputs
            .get(&criterion.output_port)
            .with_context(|| {
                format!(
                    "criterion '{}' references undeclared output '{}.{}'",
                    criterion.id, criterion.producer_node_id, criterion.output_port
                )
            })?;
        if port.kind != PortValueKind::Assessment {
            bail!(
                "criterion '{}' producer output must have assessment type",
                criterion.id
            );
        }
        if !criterion.advisory && !nodes[criterion.producer_node_id.as_str()].required {
            bail!(
                "deterministic criterion '{}' cannot be produced by optional node '{}'",
                criterion.id,
                criterion.producer_node_id
            );
        }
    }
    if total != 100 {
        bail!("workflow criterion weights must total 100; observed {total}");
    }
    Ok(())
}

fn validate_retry_safety(
    definition: &WorkflowDefinitionV1,
    descriptors: &HashMap<&str, &StepTypeDescriptor>,
) -> Result<()> {
    if definition.limits.technical_retries == 0 {
        return Ok(());
    }
    let unsafe_steps = definition
        .nodes
        .iter()
        .filter(|node| descriptors[node.id.as_str()].replay_policy != ReplayPolicy::Idempotent)
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    if !unsafe_steps.is_empty() {
        bail!(
            "technical retries require an entirely idempotent workflow; unsafe nodes: {}",
            unsafe_steps.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;

    struct Noop;

    #[async_trait]
    impl StepExecutor for Noop {
        async fn execute(&self, _: StepExecutorContext) -> Result<StepExecutorOutput> {
            Ok(StepExecutorOutput::default())
        }
    }

    fn descriptor(id: &str) -> StepTypeDescriptor {
        StepTypeDescriptor {
            id: id.into(),
            version: 1,
            description: "test".into(),
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
        }
    }

    fn catalog() -> StepCatalog {
        let mut catalog = StepCatalog::new();
        catalog
            .register(descriptor("test.noop"), Arc::new(Noop))
            .unwrap();
        catalog
    }

    fn node(id: &str, dependencies: &[&str]) -> WorkflowNodeV1 {
        WorkflowNodeV1 {
            id: id.into(),
            step_type: "test.noop".into(),
            step_version: 1,
            config: json!({}),
            depends_on: dependencies.iter().map(|value| (*value).into()).collect(),
            inputs: BTreeMap::new(),
            activation: ActivationPolicy::Always,
            dependency_policy: DependencyPolicy::Succeeded,
            required: true,
        }
    }

    fn workflow(nodes: Vec<WorkflowNodeV1>) -> WorkflowDefinitionV1 {
        WorkflowDefinitionV1 {
            schema_version: 1,
            id: "test.workflow".into(),
            scenario_version: 1,
            description: "test workflow".into(),
            limits: WorkflowLimits::default(),
            nodes,
            criteria: Vec::new(),
        }
    }

    #[test]
    fn rejects_cycles_and_unknown_step_versions() {
        let error = workflow(vec![node("a", &["b"]), node("b", &["a"])])
            .validate(&catalog())
            .unwrap_err();
        assert!(error.to_string().contains("cycle"));

        let mut unknown = node("a", &[]);
        unknown.step_version = 7;
        let error = workflow(vec![unknown]).validate(&catalog()).unwrap_err();
        assert!(error.to_string().contains("unregistered"));
    }

    #[test]
    fn rejects_required_conditional_nodes_and_invalid_budgets() {
        let mut conditional = node("conditional", &["root"]);
        conditional.activation = ActivationPolicy::All(vec![BooleanCondition {
            node_id: "root".into(),
            port: "ok".into(),
            equals: true,
        }]);
        let error = workflow(vec![node("root", &[]), conditional])
            .validate(&catalog())
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("conditional nodes must be optional"));

        let mut impossible = workflow(vec![node("root", &[])]);
        impossible.limits.max_parallel = LOCAL_MAX_PARALLELISM + 1;
        assert!(impossible
            .validate(&catalog())
            .unwrap_err()
            .to_string()
            .contains("local limit"));
    }

    #[test]
    fn hash_is_independent_of_json_object_key_order() {
        let mut left = workflow(vec![node("root", &[])]);
        left.nodes[0].config = json!({"b": 2, "a": 1});
        let mut right = left.clone();
        right.nodes[0].config = json!({"a": 1, "b": 2});
        assert_eq!(
            left.canonical_sha256().unwrap(),
            right.canonical_sha256().unwrap()
        );
    }

    #[test]
    fn technical_retries_reject_non_repeatable_steps() {
        let mut catalog = catalog();
        let mut unsafe_descriptor = descriptor("test.non_repeatable");
        unsafe_descriptor.replay_policy = ReplayPolicy::NonRepeatable;
        catalog.register(unsafe_descriptor, Arc::new(Noop)).unwrap();
        let mut unsafe_node = node("unsafe", &[]);
        unsafe_node.step_type = "test.non_repeatable".into();
        let mut definition = workflow(vec![unsafe_node]);
        definition.limits.technical_retries = 1;

        let error = definition.validate(&catalog).unwrap_err().to_string();
        assert!(error.contains("entirely idempotent"), "{error}");
        assert!(error.contains("unsafe"), "{error}");
    }
}
