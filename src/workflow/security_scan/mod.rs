use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::process::Command;

use crate::context::E2eContext;

use super::{
    ActivationPolicy, BooleanCondition, CapturedWorkflowAsset, ControlSource, DependencyPolicy,
    PortValueKind, ReplayPolicy, RequiredFunctionContract, StepCatalog, StepEvaluation,
    StepExecutor, StepExecutorContext, StepExecutorOutput, StepOperationalKind, StepPortDescriptor,
    StepTypeDescriptor, TypedPortValue, WorkflowAssetContent, WorkflowCleanupContext,
    WorkflowCleanupHook, WorkflowCriterionDeclaration, WorkflowDefinition,
    WorkflowEvaluationOutcome, WorkflowEvaluationResult, WorkflowGateResult, WorkflowInputBinding,
    WorkflowLimits, WorkflowNode, WorkflowProvenance,
};

mod definition;
mod evaluation;
mod executor;
mod fixture;
mod helpers;
mod local_adapter;
mod operations;
#[cfg(test)]
mod tests;

pub use definition::definition;
use definition::descriptors;
pub(crate) use definition::required_contract;
pub(crate) use evaluation::{
    evaluate_patch_applicability, evaluate_reconciliation, evaluate_reconciliation_filters,
    evaluate_report, gate, reconciliation_infrastructure_failure,
};
use executor::{SecurityExecutor, SecurityStepKind};
use fixture::{FixtureState, SecurityReviewCleanup};
pub(crate) use helpers::{
    append_operation, bool_value, config_bool, config_string, config_u64, ensure_clean,
    fixture_path, git, git_success, input_string, input_value, json_value, object_schema,
    operation_context, operation_output, operation_output_bool, operation_output_string,
    operation_output_value, output_with_asset, output_with_internal_evaluation, port,
    required_string, text_value, typed_inputs, validate_contract_info, validate_sha,
};
pub(crate) use local_adapter::register_local_adapter_if_configured;

pub const FIXTURE_PATH_ENV: &str = "HARNESS_E2E_SECURITY_FIXTURE_PATH";
pub const SCENARIO_ID: &str = "security_review";
const REPOSITORY: &str = "iii-hq/security-scan-e2e-fixture";
const COMMIT_B_REF: &str = "security-scan-e2e-commit-b";
const SEEDED_PATHS: [&str; 4] = [
    "src/vulnerable.rs",
    "package.json",
    ".env.example",
    ".github/workflows/insecure.yml",
];
const REQUEST_FUNCTION: &str = "security-scan::request";
const READ_FUNCTION: &str = "security-scan::read";
const LIST_FUNCTION: &str = "security-scan::list";
const RECONCILIATION_FUNCTION: &str = "security-scan::reconciliation";

const SECURITY_SCAN_CONTRACT_HASHES: [(&str, &str, &str); 4] = [
    (
        REQUEST_FUNCTION,
        "sha256:7c7ff97a1a4d519a5a0a366ca0d3a9e72528661a96a34158f6f9728c271259d1",
        "sha256:a7e608920717c0f83e3297dd2909de437f0b929d9d697c9f2f05273e150e7de4",
    ),
    (
        READ_FUNCTION,
        "sha256:dc5c52abf1e842caf85a3a0a69f320a032dda8b00af1436fcaa6ffffe022c4e2",
        "sha256:3cdc2c2692282860937f19a4e88548e104099f00556545435b14d0184b296331",
    ),
    (
        LIST_FUNCTION,
        "sha256:ed2a5e73fc56b3d0c7e7bf5da5b161a7282aa3ba8ae6e5efda73d10b4b8e5f5e",
        "sha256:8552f5be274e550f9518f59406d3ea246ebd4b23380e2de68544d9e5bc91a79b",
    ),
    (
        RECONCILIATION_FUNCTION,
        "sha256:1b20499e10dd19f70a548b5234a2d39991b9eee1e0c15b2061f96629c5a3c3f0",
        "sha256:7797fa6e8a508281e879adc2fc793e498d8841116e41137ca464cce26dae1c36",
    ),
];

pub fn register_security_scan_steps(
    catalog: &mut StepCatalog,
    context: Arc<E2eContext>,
) -> Result<Arc<dyn WorkflowCleanupHook>> {
    let fixture = Arc::new(FixtureState::default());
    for (descriptor, kind) in descriptors() {
        catalog.register(
            descriptor,
            Arc::new(SecurityExecutor {
                context: context.clone(),
                kind,
                fixture: fixture.clone(),
            }),
        )?;
    }
    Ok(Arc::new(SecurityReviewCleanup { fixture }))
}

pub fn descriptors_only() -> Vec<StepTypeDescriptor> {
    descriptors()
        .into_iter()
        .map(|(descriptor, _)| descriptor)
        .collect()
}
