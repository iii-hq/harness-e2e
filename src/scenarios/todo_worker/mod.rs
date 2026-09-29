//! Todo Worker scenarios split by contract, evidence, probing, lifecycle, and cleanup.
//!
//! The public surface remains intentionally small: `scenarios/mod.rs` consumes
//! the scenario constructors and the workflow consumes the contract/evidence
//! types. The implementation details live in focused sibling modules.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures_util::future::join_all;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedDeliverableContent,
    CapturedInvariant, CriterionSpec, DeliverableContract, ExecutionPolicy, InvariantSpec,
    ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase, ScenarioCharacterization,
    ScenarioExecutionKind, ScenarioObservation, ScenarioSpec,
};

pub const SIMPLE_ID: &str = "todo_worker_simple";
pub const PLANNED_ID: &str = "todo_worker_planned";
pub const VALIDATION_ASSET_ID: &str = "todo_validation_evidence";
pub const RAW_PLAN_FILE: &str = "validation-plan.json";
pub const OWNER_MARKER: &str = ".harness-e2e-owner";
pub const REQUIRED_PROBES: [&str; 5] = [
    "compose_valid",
    "worker_live",
    "function_surface",
    "todo_crud_isolated",
    "todo_invalid_contracts",
];
pub const OPTIONAL_PROBES: [&str; 2] = ["todo_repeatability", "todo_concurrent_create"];

const SIMPLE_ASSESSMENTS: &[AssessmentSpec] = &[
    AssessmentSpec::scored_in(
        "compose_valid",
        15,
        "Is the generated worker-compose.yaml valid, scoped to this run, configured with an explicit runtime, and aligned with the stack it exposes?",
        EvaluationDimension::Deliverable,
    ),
    AssessmentSpec::scored("worker_live", 15, "Is the expected local worker installed and running?"),
    AssessmentSpec::scored("function_surface", 15, "Do all four Todo functions expose the required descriptions and schemas?"),
    AssessmentSpec::scored("todo_crud_isolated", 30, "Do create, list, update, and delete preserve item identity and leave unrelated items unchanged?"),
    AssessmentSpec::scored("todo_invalid_contracts", 15, "Are empty titles and unknown IDs rejected?"),
    AssessmentSpec::scored_in(
        "evidence_complete",
        10,
        "Is the validation bundle complete, bounded, and bound to the observed candidate?",
        EvaluationDimension::Deliverable,
    ),
];

pub const PLANNED_CRITERIA: [CriterionSpec; 4] = [
    CriterionSpec::scored(
        "planning_contract",
        25,
        "Does the planner emit a bounded, compilable plan that covers every mandatory validation?",
        EvaluationDimension::StructuralIntegrity,
    ),
    CriterionSpec::scored(
        "worker_construction",
        25,
        "Does the separate builder materialize the exact run-scoped worker contract and bring the worker live?",
        EvaluationDimension::Deliverable,
    ),
    CriterionSpec::scored(
        "validation_coverage",
        25,
        "Does the independent runner execute every planned check and retain immutable evidence?",
        EvaluationDimension::StructuralIntegrity,
    ),
    CriterionSpec::scored(
        "functional_correctness",
        25,
        "Do the compiled hard gates prove the worker lifecycle, function contracts, CRUD isolation, and invalid-input behavior?",
        EvaluationDimension::Deliverable,
    ),
];

mod contracts;
mod evidence;
mod probe_runner;
mod probes;
mod scenarios;
mod workspace;

pub use contracts::*;
pub use evidence::*;
pub use probe_runner::*;
#[cfg(test)]
pub(crate) use probes::{
    is_remote_invocation_failure, worker_mechanism_unavailable_in_error,
    worker_mechanism_unavailable_in_status,
};
pub use scenarios::*;
pub use workspace::*;

#[cfg(test)]
mod tests;
