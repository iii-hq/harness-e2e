use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::context::E2eContext;

use super::{
    ActivationPolicy, BooleanCondition, CapturedWorkflowAsset, ControlSource, DependencyPolicy,
    HarnessStepPolicy, PortValueKind, ReplayPolicy, RequiredFunctionContract, StepCatalog,
    StepEvaluation, StepExecutor, StepExecutorContext, StepExecutorOutput, StepOperationalKind,
    StepPortDescriptor, StepTypeDescriptor, TypedPortValue, WorkflowAssetContent,
    WorkflowCleanupContext, WorkflowCleanupHook, WorkflowCriterionDeclaration,
    WorkflowDefinitionV1, WorkflowEvaluationOutcome, WorkflowEvaluationResult, WorkflowGateResult,
    WorkflowInputBinding, WorkflowLimits, WorkflowNodeV1, WorkflowProvenance,
};

mod definition;
mod evaluation;
mod executor;
mod fixture;
mod helpers;
mod local_adapter;
mod prompts;
mod schemas;
#[cfg(test)]
mod tests;

pub use definition::definition;
use definition::descriptors;
use executor::{IncidentExecutor, IncidentStepKind};
use fixture::{IncidentFixtureState, IncidentResponseCleanup};

pub const PREFLIGHT_FUNCTION: &str = "incident-fixture::preflight";
pub const BASELINE_FUNCTION: &str = "incident-fixture::baseline";
pub const ALERT_FUNCTION: &str = "incident-fixture::alert";
pub const REPRODUCE_FUNCTION: &str = "incident-fixture::reproduce";
pub const TELEMETRY_FUNCTION: &str = "incident-fixture::telemetry";
pub const VALIDATE_FUNCTION: &str = "incident-fixture::validate";
pub const DEPLOY_FUNCTION: &str = "incident-fixture::deploy";
pub const RECONCILE_FUNCTION: &str = "incident-fixture::reconcile";
pub const RESET_FUNCTION: &str = "incident-fixture::reset";

pub const FIXTURE_FUNCTIONS: [&str; 9] = [
    PREFLIGHT_FUNCTION,
    BASELINE_FUNCTION,
    ALERT_FUNCTION,
    REPRODUCE_FUNCTION,
    TELEMETRY_FUNCTION,
    VALIDATE_FUNCTION,
    DEPLOY_FUNCTION,
    RECONCILE_FUNCTION,
    RESET_FUNCTION,
];

pub fn descriptors_only() -> Result<Vec<StepTypeDescriptor>> {
    Ok(descriptors()?
        .into_iter()
        .map(|(descriptor, _)| descriptor)
        .collect())
}

pub fn fixture_root() -> Result<PathBuf> {
    helpers::fixture_path()
}

pub fn harness_policy() -> Result<HarnessStepPolicy> {
    HarnessStepPolicy::new(
        [fixture_root()?],
        [
            "e2e::*".to_string(),
            "incident-fixture::reset".to_string(),
            "incident-fixture::deploy".to_string(),
        ],
    )
}

pub fn register_incident_response_steps(
    catalog: &mut StepCatalog,
    context: Arc<E2eContext>,
) -> Result<Arc<dyn WorkflowCleanupHook>> {
    local_adapter::register(context.as_ref())?;
    let fixture = Arc::new(IncidentFixtureState::default());
    for (descriptor, kind) in descriptors()? {
        catalog.register(
            descriptor,
            Arc::new(IncidentExecutor {
                context: context.clone(),
                kind,
                fixture: fixture.clone(),
            }),
        )?;
    }
    Ok(Arc::new(IncidentResponseCleanup { context, fixture }))
}

pub(crate) fn required_contract(function_id: &str) -> Result<RequiredFunctionContract> {
    helpers::required_contract(function_id)
}
