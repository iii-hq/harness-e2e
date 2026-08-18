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
    WorkflowCleanupHook, WorkflowCriterionDeclaration, WorkflowDefinitionV1,
    WorkflowEvaluationOutcome, WorkflowEvaluationResult, WorkflowGateResult, WorkflowInputBinding,
    WorkflowLimits, WorkflowNodeV1, WorkflowProvenance,
};

mod definition;
mod evaluation;
mod executor;
mod fixture;
mod helpers;
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
        "sha256:98d05e7144cf148707bfcf79382fda5cbd9c493424b7ce3aed934db61acf2994",
        "sha256:c749c1c1255471b8137115962184b1fbb8d4f15c15fb5ccb2a446fcd373aca98",
    ),
    (
        READ_FUNCTION,
        "sha256:20c305053371e147a2bd1802e81533bbccebf2f3d29d8269c27102713e7bcb0a",
        "sha256:d065d73944025a81235fe967ea837ffbee6dfeef9ae02cc61c57fe3c2197ea7c",
    ),
    (
        LIST_FUNCTION,
        "sha256:12bb3e62ac2c77b318d843ac9c62a86158d179118c36226eaef9ca3f0526a44b",
        "sha256:289b33d4b74d53f02fafa1e3d7d6f6d1494dcdf58fe397d0b5f83f611d9c73b6",
    ),
    (
        RECONCILIATION_FUNCTION,
        "sha256:ab5b929ded2087de6932a40a93c7d547854687e274c79a01ebc913efe92d6ab3",
        "sha256:532b0d7b93389c1b4598141a863dc789edb6be95a7af5cdd4555a691940f00a1",
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
