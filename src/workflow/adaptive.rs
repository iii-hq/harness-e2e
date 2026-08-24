use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ActivationPolicy, ControlSource, DependencyPolicy, StepCatalog, StepOperationalKind,
    WorkflowCriterionDeclaration, WorkflowDefinitionV1, WorkflowInputBinding, WorkflowLimits,
    WorkflowNodeV1, WORKFLOW_SCHEMA_VERSION,
};

pub const ADAPTIVE_WORKFLOW_SCHEMA_VERSION: u32 = 1;
pub const ADAPTIVE_MAX_PLAN_REVISIONS: u8 = 2;
const DEFAULT_MAX_INSTRUCTION_BYTES: u32 = 8 * 1024;

/// Runner-owned policy. It is intentionally Serialize-only: an agent-provided
/// document can reference this policy by hash but can never replace it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveWorkflowPolicyV1 {
    pub schema_version: u32,
    pub id: String,
    pub scenario_version: u32,
    pub description: String,
    pub limits: WorkflowLimits,
    pub max_plan_nodes: u16,
    pub max_plan_depth: u16,
    #[serde(default = "default_max_plan_revisions")]
    pub max_plan_revisions: u8,
    #[serde(default = "default_max_instruction_bytes")]
    pub max_instruction_bytes: u32,
    pub templates: Vec<AdaptiveNodeTemplateV1>,
    pub trusted_anchors: Vec<AdaptiveTrustedAnchorV1>,
    #[serde(default)]
    pub criteria: Vec<WorkflowCriterionDeclaration>,
}

fn default_max_plan_revisions() -> u8 {
    ADAPTIVE_MAX_PLAN_REVISIONS
}

fn default_max_instruction_bytes() -> u32 {
    DEFAULT_MAX_INSTRUCTION_BYTES
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveNodeTemplateV1 {
    pub id: String,
    pub description: String,
    pub step_type: String,
    pub step_version: u32,
    #[serde(default)]
    pub base_config: Value,
    #[serde(default)]
    pub inputs: BTreeMap<String, WorkflowInputBinding>,
    #[serde(default)]
    pub activation: ActivationPolicy,
    #[serde(default)]
    pub dependency_policy: DependencyPolicy,
    #[serde(default = "default_required")]
    pub required: bool,
    #[serde(default)]
    pub allowed_focuses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_config_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_config_key: Option<String>,
    #[serde(default)]
    pub min_occurrences: u16,
    #[serde(default = "default_max_occurrences")]
    pub max_occurrences: u16,
}

fn default_required() -> bool {
    true
}

fn default_max_occurrences() -> u16 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdaptiveAnchorPlacement {
    BeforePlan,
    AfterPlan,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveTrustedAnchorV1 {
    pub placement: AdaptiveAnchorPlacement,
    #[serde(default)]
    pub terminal: bool,
    pub node: WorkflowNodeV1,
}

/// The only agent-authored shape. Step types, functions, workspaces, budgets,
/// activation, inputs, criteria and mutation controls stay in the policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePlanNodeV1 {
    pub id: String,
    pub template_id: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveWorkflowPlanV1 {
    pub schema_version: u32,
    pub policy_sha256: String,
    pub revision: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    pub nodes: Vec<AdaptivePlanNodeV1>,
}

impl AdaptiveWorkflowPlanV1 {
    pub fn canonical_sha256(&self) -> Result<String> {
        crate::artifact::sha256_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePlanRevisionEvidence {
    pub revision: u8,
    pub plan_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AdaptiveMaterializedWorkflow {
    pub definition: WorkflowDefinitionV1,
    pub policy_sha256: String,
    pub latest_plan_sha256: String,
    pub revisions: Vec<AdaptivePlanRevisionEvidence>,
}

impl AdaptiveWorkflowPolicyV1 {
    pub fn canonical_sha256(&self) -> Result<String> {
        crate::artifact::sha256_value(self)
    }

    /// Validate all revisions and freeze the latest one into the ordinary,
    /// Rust-validated workflow definition consumed by the scheduler.
    /// `completed_node_ids` is empty for revision one. During replanning it
    /// prevents the agent from changing or removing already-completed work.
    pub fn materialize(
        &self,
        plans: &[AdaptiveWorkflowPlanV1],
        completed_node_ids: &BTreeSet<String>,
        catalog: &StepCatalog,
    ) -> Result<AdaptiveMaterializedWorkflow> {
        self.validate_policy(catalog)?;
        if plans.is_empty() {
            bail!("adaptive workflow '{}' requires a plan", self.id);
        }
        if plans.len() > usize::from(self.max_plan_revisions) {
            bail!(
                "adaptive workflow '{}' received {} revisions; limit is {}",
                self.id,
                plans.len(),
                self.max_plan_revisions
            );
        }
        let policy_sha256 = self.canonical_sha256()?;
        let before_ids = self
            .trusted_anchors
            .iter()
            .filter(|anchor| anchor.placement == AdaptiveAnchorPlacement::BeforePlan)
            .map(|anchor| anchor.node.id.as_str())
            .collect::<HashSet<_>>();
        let after_ids = self
            .trusted_anchors
            .iter()
            .filter(|anchor| anchor.placement == AdaptiveAnchorPlacement::AfterPlan)
            .map(|anchor| anchor.node.id.as_str())
            .collect::<HashSet<_>>();
        let mut evidence = Vec::with_capacity(plans.len());
        let mut previous: Option<(&AdaptiveWorkflowPlanV1, String)> = None;
        for (index, plan) in plans.iter().enumerate() {
            let expected_revision = u8::try_from(index + 1).unwrap_or(u8::MAX);
            self.validate_plan(
                plan,
                expected_revision,
                &policy_sha256,
                &before_ids,
                &after_ids,
            )?;
            let sha256 = plan.canonical_sha256()?;
            match &previous {
                None => {
                    if plan.supersedes_sha256.is_some()
                        || plan.reason.is_some()
                        || !plan.evidence_ids.is_empty()
                    {
                        bail!("adaptive plan revision 1 cannot supersede prior work");
                    }
                }
                Some((prior, prior_sha256)) => {
                    if plan.supersedes_sha256.as_deref() != Some(prior_sha256.as_str()) {
                        bail!(
                            "adaptive plan revision {} has an invalid supersedes_sha256",
                            plan.revision
                        );
                    }
                    if plan.reason.as_deref().is_none_or(str::is_empty)
                        || plan.evidence_ids.is_empty()
                    {
                        bail!(
                            "adaptive plan revision {} requires a reason and evidence_ids",
                            plan.revision
                        );
                    }
                    validate_completed_nodes_unchanged(prior, plan, completed_node_ids)?;
                }
            }
            evidence.push(AdaptivePlanRevisionEvidence {
                revision: plan.revision,
                plan_sha256: sha256.clone(),
                supersedes_sha256: plan.supersedes_sha256.clone(),
                reason: plan.reason.clone(),
                evidence_ids: plan.evidence_ids.clone(),
            });
            previous = Some((plan, sha256));
        }

        let latest = plans.last().expect("non-empty plans");
        let latest_plan_sha256 = previous.expect("validated plan").1;
        let definition = self.materialize_plan(latest, catalog)?;
        Ok(AdaptiveMaterializedWorkflow {
            definition,
            policy_sha256,
            latest_plan_sha256,
            revisions: evidence,
        })
    }

    fn validate_policy(&self, catalog: &StepCatalog) -> Result<()> {
        if self.schema_version != ADAPTIVE_WORKFLOW_SCHEMA_VERSION {
            bail!(
                "adaptive policy '{}' uses an unsupported schema version",
                self.id
            );
        }
        validate_identifier(&self.id, "adaptive policy id")?;
        if self.scenario_version == 0 || self.description.trim().is_empty() {
            bail!(
                "adaptive policy '{}' requires a version and description",
                self.id
            );
        }
        if self.limits.technical_retries != 0 {
            bail!("adaptive workflows prohibit technical retries");
        }
        if self.max_plan_nodes == 0
            || self.max_plan_nodes > self.limits.max_nodes
            || self.max_plan_depth == 0
        {
            bail!("adaptive plan node/depth bounds must be positive and within workflow limits");
        }
        if self.max_plan_revisions == 0 || self.max_plan_revisions > ADAPTIVE_MAX_PLAN_REVISIONS {
            bail!("adaptive plan revisions must be between 1 and 2");
        }
        if self.max_instruction_bytes == 0 {
            bail!("adaptive instruction bound must be positive");
        }
        if self.templates.is_empty() || self.trusted_anchors.is_empty() {
            bail!(
                "adaptive policy '{}' requires templates and trusted anchors",
                self.id
            );
        }
        let mut template_ids = HashSet::new();
        for template in &self.templates {
            validate_identifier(&template.id, "adaptive template id")?;
            if !template_ids.insert(template.id.as_str()) {
                bail!("adaptive policy has duplicate template '{}'", template.id);
            }
            if template.description.trim().is_empty()
                || template.max_occurrences == 0
                || template.min_occurrences > template.max_occurrences
            {
                bail!(
                    "adaptive template '{}' has invalid occurrence bounds",
                    template.id
                );
            }
            validate_optional_key(&template.focus_config_key)?;
            validate_optional_key(&template.instructions_config_key)?;
            if !template.allowed_focuses.is_empty() && template.focus_config_key.is_none() {
                bail!(
                    "adaptive template '{}' allows focus without a config key",
                    template.id
                );
            }
            let registered = catalog
                .get(&template.step_type, template.step_version)
                .with_context(|| {
                    format!(
                        "adaptive template '{}' references an unregistered step",
                        template.id
                    )
                })?;
            if registered.descriptor.operational_kind == StepOperationalKind::Product
                && matches!(template.activation, ActivationPolicy::Always)
            {
                bail!(
                    "adaptive product template '{}' requires runner-owned deterministic activation",
                    template.id
                );
            }
            let mut probe = template.base_config.clone();
            if let Some(key) = &template.focus_config_key {
                set_config_string(
                    &mut probe,
                    key,
                    template
                        .allowed_focuses
                        .first()
                        .map_or("probe", String::as_str),
                )?;
            }
            if let Some(key) = &template.instructions_config_key {
                set_config_string(&mut probe, key, "probe")?;
            }
            registered
                .descriptor
                .validate_config(&probe)
                .with_context(|| {
                    format!("validate adaptive template '{}' configuration", template.id)
                })?;
        }
        let mut anchor_ids = HashSet::new();
        let mut terminal_count = 0;
        for anchor in &self.trusted_anchors {
            validate_identifier(&anchor.node.id, "adaptive anchor node id")?;
            if !anchor_ids.insert(anchor.node.id.as_str()) {
                bail!("adaptive policy has duplicate anchor '{}'", anchor.node.id);
            }
            if anchor.terminal {
                terminal_count += 1;
                if anchor.placement != AdaptiveAnchorPlacement::AfterPlan
                    || !anchor.node.required
                    || !matches!(anchor.node.activation, ActivationPolicy::Always)
                {
                    bail!(
                        "terminal anchor '{}' must be a required after-plan node",
                        anchor.node.id
                    );
                }
            }
        }
        if terminal_count == 0 {
            bail!(
                "adaptive policy '{}' requires a trusted terminal anchor",
                self.id
            );
        }
        Ok(())
    }

    fn validate_plan(
        &self,
        plan: &AdaptiveWorkflowPlanV1,
        expected_revision: u8,
        policy_sha256: &str,
        before_ids: &HashSet<&str>,
        after_ids: &HashSet<&str>,
    ) -> Result<()> {
        if plan.schema_version != ADAPTIVE_WORKFLOW_SCHEMA_VERSION
            || plan.revision != expected_revision
            || plan.policy_sha256 != policy_sha256
        {
            bail!(
                "adaptive plan revision {} does not match its frozen policy/sequence",
                plan.revision
            );
        }
        if plan.nodes.is_empty() || plan.nodes.len() > usize::from(self.max_plan_nodes) {
            bail!(
                "adaptive plan revision {} violates its node bound",
                plan.revision
            );
        }
        let templates = self
            .templates
            .iter()
            .map(|template| (template.id.as_str(), template))
            .collect::<HashMap<_, _>>();
        let mut ids = HashSet::new();
        let mut occurrences: HashMap<&str, u16> = HashMap::new();
        for node in &plan.nodes {
            validate_identifier(&node.id, "adaptive plan node id")?;
            if !ids.insert(node.id.as_str())
                || before_ids.contains(node.id.as_str())
                || after_ids.contains(node.id.as_str())
            {
                bail!(
                    "adaptive plan has a duplicate or reserved node id '{}'",
                    node.id
                );
            }
            let template = templates.get(node.template_id.as_str()).with_context(|| {
                format!(
                    "adaptive plan node '{}' references unknown template '{}'",
                    node.id, node.template_id
                )
            })?;
            *occurrences.entry(template.id.as_str()).or_default() += 1;
            if let Some(focus) = &node.focus {
                if !template
                    .allowed_focuses
                    .iter()
                    .any(|allowed| allowed == focus)
                {
                    bail!(
                        "adaptive plan node '{}' uses disallowed focus '{}'",
                        node.id,
                        focus
                    );
                }
            }
            if node.instructions.as_ref().is_some_and(|instructions| {
                instructions.trim().is_empty()
                    || instructions.len() > self.max_instruction_bytes as usize
            }) {
                bail!(
                    "adaptive plan node '{}' has empty or oversized instructions",
                    node.id
                );
            }
            if node.instructions.is_some() && template.instructions_config_key.is_none() {
                bail!(
                    "adaptive plan node '{}' cannot set instructions for its template",
                    node.id
                );
            }
            let mut unique = HashSet::new();
            for dependency in &node.depends_on {
                if !unique.insert(dependency.as_str()) {
                    bail!(
                        "adaptive plan node '{}' repeats dependency '{}'",
                        node.id,
                        dependency
                    );
                }
            }
        }
        for template in &self.templates {
            let count = occurrences.get(template.id.as_str()).copied().unwrap_or(0);
            if count < template.min_occurrences || count > template.max_occurrences {
                bail!(
                    "adaptive template '{}' occurrence count {} is outside {}..={}",
                    template.id,
                    count,
                    template.min_occurrences,
                    template.max_occurrences
                );
            }
        }
        let plan_ids = plan
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        for node in &plan.nodes {
            for dependency in &node.depends_on {
                if after_ids.contains(dependency.as_str())
                    || (!plan_ids.contains(dependency.as_str())
                        && !before_ids.contains(dependency.as_str()))
                {
                    bail!(
                        "adaptive plan node '{}' references unavailable dependency '{}'",
                        node.id,
                        dependency
                    );
                }
            }
        }
        let depth = plan_depth(&plan.nodes, before_ids)?;
        if depth > usize::from(self.max_plan_depth) {
            bail!(
                "adaptive plan depth {depth} exceeds limit {}",
                self.max_plan_depth
            );
        }
        Ok(())
    }

    fn materialize_plan(
        &self,
        plan: &AdaptiveWorkflowPlanV1,
        catalog: &StepCatalog,
    ) -> Result<WorkflowDefinitionV1> {
        let templates = self
            .templates
            .iter()
            .map(|template| (template.id.as_str(), template))
            .collect::<HashMap<_, _>>();
        let before = self
            .trusted_anchors
            .iter()
            .filter(|anchor| anchor.placement == AdaptiveAnchorPlacement::BeforePlan)
            .map(|anchor| anchor.node.clone())
            .collect::<Vec<_>>();
        let after = self
            .trusted_anchors
            .iter()
            .filter(|anchor| anchor.placement == AdaptiveAnchorPlacement::AfterPlan)
            .map(|anchor| anchor.node.clone())
            .collect::<Vec<_>>();
        let before_leaves = leaf_ids(&before);
        let mut planned = Vec::with_capacity(plan.nodes.len());
        for proposed in &plan.nodes {
            let template = templates[proposed.template_id.as_str()];
            let mut config = template.base_config.clone();
            if let Some(focus) = &proposed.focus {
                set_config_string(
                    &mut config,
                    template
                        .focus_config_key
                        .as_deref()
                        .expect("validated focus key"),
                    focus,
                )?;
            }
            if let Some(instructions) = &proposed.instructions {
                set_config_string(
                    &mut config,
                    template
                        .instructions_config_key
                        .as_deref()
                        .expect("validated instruction key"),
                    instructions,
                )?;
            }
            let mut depends_on = proposed.depends_on.clone();
            if proposed
                .depends_on
                .iter()
                .all(|dependency| !plan.nodes.iter().any(|node| &node.id == dependency))
            {
                depends_on.extend(before_leaves.iter().cloned());
            }
            depends_on.sort();
            depends_on.dedup();
            planned.push(WorkflowNodeV1 {
                id: proposed.id.clone(),
                step_type: template.step_type.clone(),
                step_version: template.step_version,
                config,
                depends_on,
                inputs: template.inputs.clone(),
                activation: template.activation.clone(),
                dependency_policy: template.dependency_policy,
                required: template.required,
            });
        }
        let plan_leaves = leaf_ids(&planned);
        let after_ids = after
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let mut wired_after = after;
        for node in &mut wired_after {
            if node
                .depends_on
                .iter()
                .all(|dependency| !after_ids.contains(dependency.as_str()))
            {
                node.depends_on.extend(plan_leaves.iter().cloned());
                node.depends_on.sort();
                node.depends_on.dedup();
            }
        }
        let mut nodes = before;
        nodes.extend(planned);
        nodes.extend(wired_after);
        let definition = WorkflowDefinitionV1 {
            schema_version: WORKFLOW_SCHEMA_VERSION,
            id: self.id.clone(),
            scenario_version: self.scenario_version,
            description: self.description.clone(),
            limits: self.limits,
            nodes,
            criteria: self.criteria.clone(),
        };
        definition.validate(catalog)?;
        validate_product_activation_is_deterministic(&definition, catalog)?;
        validate_terminal_anchors_are_leaves(&definition, &self.trusted_anchors)?;
        Ok(definition)
    }
}

fn validate_completed_nodes_unchanged(
    previous: &AdaptiveWorkflowPlanV1,
    next: &AdaptiveWorkflowPlanV1,
    completed: &BTreeSet<String>,
) -> Result<()> {
    let prior = previous
        .nodes
        .iter()
        .map(|node| (&node.id, node))
        .collect::<HashMap<_, _>>();
    let revised = next
        .nodes
        .iter()
        .map(|node| (&node.id, node))
        .collect::<HashMap<_, _>>();
    for id in completed {
        let prior = prior
            .get(id)
            .with_context(|| format!("completed node '{id}' was not in the prior plan"))?;
        let revised = revised
            .get(id)
            .with_context(|| format!("revision removed completed node '{id}'"))?;
        if prior != revised {
            bail!("revision changed completed node '{id}'");
        }
    }
    Ok(())
}

fn plan_depth(nodes: &[AdaptivePlanNodeV1], anchors: &HashSet<&str>) -> Result<usize> {
    fn visit<'a>(
        id: &'a str,
        nodes: &HashMap<&'a str, &'a AdaptivePlanNodeV1>,
        anchors: &HashSet<&str>,
        visiting: &mut HashSet<&'a str>,
        memo: &mut HashMap<&'a str, usize>,
    ) -> Result<usize> {
        if let Some(depth) = memo.get(id) {
            return Ok(*depth);
        }
        if !visiting.insert(id) {
            bail!("adaptive plan contains a cycle involving '{id}'");
        }
        let node = nodes[id];
        let mut depth = 1;
        for dependency in &node.depends_on {
            if anchors.contains(dependency.as_str()) {
                continue;
            }
            depth = depth.max(1 + visit(dependency, nodes, anchors, visiting, memo)?);
        }
        visiting.remove(id);
        memo.insert(id, depth);
        Ok(depth)
    }

    let by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut visiting = HashSet::new();
    let mut memo = HashMap::new();
    let mut maximum = 0;
    for id in by_id.keys() {
        maximum = maximum.max(visit(id, &by_id, anchors, &mut visiting, &mut memo)?);
    }
    Ok(maximum)
}

fn leaf_ids(nodes: &[WorkflowNodeV1]) -> Vec<String> {
    let dependencies = nodes
        .iter()
        .flat_map(|node| node.depends_on.iter().map(String::as_str))
        .collect::<HashSet<_>>();
    nodes
        .iter()
        .filter(|node| !dependencies.contains(node.id.as_str()))
        .map(|node| node.id.clone())
        .collect()
}

fn validate_product_activation_is_deterministic(
    definition: &WorkflowDefinitionV1,
    catalog: &StepCatalog,
) -> Result<()> {
    for node in &definition.nodes {
        let descriptor = &catalog
            .get(&node.step_type, node.step_version)
            .expect("validated step")
            .descriptor;
        if descriptor.operational_kind != StepOperationalKind::Product {
            continue;
        }
        let conditions = match &node.activation {
            ActivationPolicy::Always => continue,
            ActivationPolicy::All(conditions) | ActivationPolicy::Any(conditions) => conditions,
        };
        for condition in conditions {
            let producer = definition
                .nodes
                .iter()
                .find(|candidate| candidate.id == condition.node_id)
                .expect("validated producer");
            let output = &catalog
                .get(&producer.step_type, producer.step_version)
                .expect("validated producer descriptor")
                .descriptor
                .outputs[&condition.port];
            if output.control_source != Some(ControlSource::Deterministic) {
                bail!(
                    "adaptive product node '{}' may only be activated by deterministic outputs",
                    node.id
                );
            }
        }
    }
    Ok(())
}

fn validate_terminal_anchors_are_leaves(
    definition: &WorkflowDefinitionV1,
    anchors: &[AdaptiveTrustedAnchorV1],
) -> Result<()> {
    for terminal in anchors.iter().filter(|anchor| anchor.terminal) {
        if definition.nodes.iter().any(|node| {
            node.depends_on
                .iter()
                .any(|dependency| dependency == &terminal.node.id)
        }) {
            bail!(
                "terminal anchor '{}' must remain a workflow leaf",
                terminal.node.id
            );
        }
    }
    Ok(())
}

fn validate_optional_key(key: &Option<String>) -> Result<()> {
    if let Some(key) = key {
        validate_identifier(key, "adaptive config key")?;
    }
    Ok(())
}

fn set_config_string(config: &mut Value, key: &str, value: &str) -> Result<()> {
    let object = config
        .as_object_mut()
        .context("adaptive template config must be an object when injecting fields")?;
    object.insert(key.to_string(), Value::String(value.to_string()));
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !valid || value == "." || value == ".." {
        bail!("{label} '{value}' is invalid");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::workflow::{
        PortValueKind, ReplayPolicy, StepExecutor, StepExecutorContext, StepExecutorOutput,
        StepPortDescriptor, StepTypeDescriptor,
    };

    struct Noop;

    #[async_trait]
    impl StepExecutor for Noop {
        async fn execute(&self, _: StepExecutorContext) -> Result<StepExecutorOutput> {
            Ok(StepExecutorOutput::default())
        }
    }

    fn descriptor(id: &str, kind: StepOperationalKind) -> StepTypeDescriptor {
        StepTypeDescriptor {
            id: id.into(),
            version: 1,
            description: "test".into(),
            config_schema: json!({
                "type": "object",
                "properties": {"focus": {"type": "string"}, "instructions": {"type": "string"}},
                "additionalProperties": false
            }),
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
            operational_kind: kind,
        }
    }

    fn node(id: &str, step_type: &str) -> WorkflowNodeV1 {
        WorkflowNodeV1 {
            id: id.into(),
            step_type: step_type.into(),
            step_version: 1,
            config: json!({}),
            depends_on: Vec::new(),
            inputs: BTreeMap::new(),
            activation: ActivationPolicy::Always,
            dependency_policy: DependencyPolicy::Succeeded,
            required: true,
        }
    }

    fn fixture() -> (AdaptiveWorkflowPolicyV1, StepCatalog) {
        let mut catalog = StepCatalog::new();
        for (id, kind) in [
            ("trusted", StepOperationalKind::Assessment),
            ("analysis", StepOperationalKind::Harness),
        ] {
            catalog
                .register(descriptor(id, kind), Arc::new(Noop))
                .unwrap();
        }
        let policy = AdaptiveWorkflowPolicyV1 {
            schema_version: 1,
            id: "adaptive.test".into(),
            scenario_version: 1,
            description: "adaptive test".into(),
            limits: WorkflowLimits {
                max_nodes: 8,
                technical_retries: 0,
                ..WorkflowLimits::default()
            },
            max_plan_nodes: 4,
            max_plan_depth: 3,
            max_plan_revisions: 2,
            max_instruction_bytes: 64,
            templates: vec![AdaptiveNodeTemplateV1 {
                id: "analysis".into(),
                description: "bounded analysis".into(),
                step_type: "analysis".into(),
                step_version: 1,
                base_config: json!({}),
                inputs: BTreeMap::new(),
                activation: ActivationPolicy::Always,
                dependency_policy: DependencyPolicy::Succeeded,
                required: true,
                allowed_focuses: vec!["logs".into(), "metrics".into()],
                focus_config_key: Some("focus".into()),
                instructions_config_key: Some("instructions".into()),
                min_occurrences: 1,
                max_occurrences: 4,
            }],
            trusted_anchors: vec![
                AdaptiveTrustedAnchorV1 {
                    placement: AdaptiveAnchorPlacement::BeforePlan,
                    terminal: false,
                    node: node("preflight", "trusted"),
                },
                AdaptiveTrustedAnchorV1 {
                    placement: AdaptiveAnchorPlacement::AfterPlan,
                    terminal: true,
                    node: node("finalize", "trusted"),
                },
            ],
            criteria: Vec::new(),
        };
        (policy, catalog)
    }

    fn first_plan(policy: &AdaptiveWorkflowPolicyV1) -> AdaptiveWorkflowPlanV1 {
        AdaptiveWorkflowPlanV1 {
            schema_version: 1,
            policy_sha256: policy.canonical_sha256().unwrap(),
            revision: 1,
            supersedes_sha256: None,
            reason: None,
            evidence_ids: Vec::new(),
            nodes: vec![AdaptivePlanNodeV1 {
                id: "inspect".into(),
                template_id: "analysis".into(),
                depends_on: Vec::new(),
                focus: Some("logs".into()),
                instructions: Some("inspect retry evidence".into()),
            }],
        }
    }

    #[test]
    fn materializes_only_allowlisted_templates_between_trusted_anchors() {
        let (policy, catalog) = fixture();
        let materialized = policy
            .materialize(&[first_plan(&policy)], &BTreeSet::new(), &catalog)
            .unwrap();
        assert_eq!(
            materialized
                .definition
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["preflight", "inspect", "finalize"]
        );
        assert_eq!(
            materialized.definition.nodes[1].depends_on,
            vec!["preflight"]
        );
        assert_eq!(materialized.definition.nodes[2].depends_on, vec!["inspect"]);
        assert_eq!(materialized.definition.nodes[1].step_type, "analysis");
    }

    #[test]
    fn rejects_unknown_templates_cycles_and_excess_depth() {
        let (policy, catalog) = fixture();
        let mut plan = first_plan(&policy);
        plan.nodes[0].template_id = "raw.function".into();
        assert!(policy
            .materialize(&[plan], &BTreeSet::new(), &catalog)
            .unwrap_err()
            .to_string()
            .contains("unknown template"));

        let mut plan = first_plan(&policy);
        plan.nodes.push(AdaptivePlanNodeV1 {
            id: "other".into(),
            template_id: "analysis".into(),
            depends_on: vec!["inspect".into()],
            focus: Some("metrics".into()),
            instructions: None,
        });
        plan.nodes[0].depends_on = vec!["other".into()];
        assert!(policy
            .materialize(&[plan], &BTreeSet::new(), &catalog)
            .unwrap_err()
            .to_string()
            .contains("cycle"));
    }

    #[test]
    fn second_revision_requires_evidence_and_preserves_completed_nodes() {
        let (policy, catalog) = fixture();
        let first = first_plan(&policy);
        let mut second = first.clone();
        second.revision = 2;
        second.supersedes_sha256 = Some(first.canonical_sha256().unwrap());
        second.reason = Some("initial hypothesis was falsified".into());
        second.evidence_ids = vec!["falsification.receipt".into()];
        second.nodes.push(AdaptivePlanNodeV1 {
            id: "verify".into(),
            template_id: "analysis".into(),
            depends_on: vec!["inspect".into()],
            focus: Some("metrics".into()),
            instructions: None,
        });
        let completed = BTreeSet::from(["inspect".to_string()]);
        policy
            .materialize(&[first.clone(), second.clone()], &completed, &catalog)
            .unwrap();

        second.nodes[0].focus = Some("metrics".into());
        assert!(policy
            .materialize(&[first, second], &completed, &catalog)
            .unwrap_err()
            .to_string()
            .contains("changed completed node"));
    }

    #[test]
    fn agent_plan_wire_shape_rejects_runner_owned_fields() {
        let value = json!({
            "schema_version": 1,
            "policy_sha256": format!("sha256:{}", "1".repeat(64)),
            "revision": 1,
            "nodes": [{
                "id": "inspect",
                "template_id": "analysis",
                "step_type": "raw.function",
                "depends_on": []
            }],
            "criteria": []
        });
        let error = serde_json::from_value::<AdaptiveWorkflowPlanV1>(value)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unknown field") || error.contains("step_type"),
            "{error}"
        );
    }
}
