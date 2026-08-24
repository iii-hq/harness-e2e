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
    ArtifactExpectation, CapturedDeliverable, CapturedDeliverableContent, CapturedInvariant,
    CleanupFuture, ComplexityProfile, CriterionSpec, DeliverableCaptureFuture, DeliverableContract,
    EvaluationFuture, ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const SIMPLE_ID: &str = "todo_worker_simple";
pub const PLANNED_ID: &str = "todo_worker_planned";
pub const VERSION: u32 = 2;
pub const VALIDATION_ASSET_ID: &str = "todo_validation_evidence";
pub const RAW_PLAN_FILE: &str = "validation-plan.json";
pub const OWNER_MARKER: &str = ".harness-e2e-owner";
pub const REQUIRED_PROBES: [&str; 5] = [
    "manifest_valid",
    "worker_live",
    "function_surface",
    "todo_crud_isolated",
    "todo_invalid_contracts",
];
pub const OPTIONAL_PROBES: [&str; 2] = ["todo_repeatability", "todo_concurrent_create"];

const SIMPLE_ASSESSMENTS: &[AssessmentSpec] = &[
    AssessmentSpec::hard_gated_in(
        "manifest_valid",
        15,
        "The generated iii.worker.yaml is valid, run-scoped, starts explicitly, and has no setup script.",
        EvaluationDimension::Deliverable,
    ),
    AssessmentSpec::hard_gated("worker_live", 15, "The expected local worker is installed and running."),
    AssessmentSpec::hard_gated("function_surface", 15, "All four Todo functions expose the exact descriptions and schemas."),
    AssessmentSpec::hard_gated("todo_crud_isolated", 30, "Create, list, update, and delete preserve identity and unrelated items."),
    AssessmentSpec::hard_gated("todo_invalid_contracts", 15, "Empty titles and unknown IDs are rejected."),
    AssessmentSpec::hard_gated_in(
        "evidence_complete",
        10,
        "The validation bundle is complete, bounded, and bound to the observed candidate.",
        EvaluationDimension::Deliverable,
    ),
];

pub const PLANNED_CRITERIA: [CriterionSpec; 4] = [
    CriterionSpec::required_deterministic(
        "planning_contract",
        25,
        "The planner emits a bounded, compilable plan with complete mandatory validation coverage.",
        EvaluationDimension::StructuralIntegrity,
    ),
    CriterionSpec::required_deterministic(
        "worker_construction",
        25,
        "The separate builder materializes the exact run-scoped worker contract and brings it live.",
        EvaluationDimension::Deliverable,
    ),
    CriterionSpec::required_deterministic(
        "validation_coverage",
        25,
        "Every planned check is executed by the independent runner with immutable evidence.",
        EvaluationDimension::StructuralIntegrity,
    ),
    CriterionSpec::required_deterministic(
        "functional_correctness",
        25,
        "The compiled hard gates prove lifecycle, function contracts, CRUD isolation, and invalid-input behavior.",
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
