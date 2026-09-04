//! Native, data-defined benchmark tasks for measuring Harness evolution.
//!
//! A task owns its fixture, instruction, execution envelope and deterministic
//! verifier. It does not delegate authority to a code-defined scenario. The
//! same task identity can therefore be executed against multiple Harness
//! system identities and compared longitudinally.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;
use uuid::Uuid;

use crate::artifact::{self, sha256_bytes};
use crate::context::E2eContext;
use crate::scenarios::ExecutionPolicy;
use crate::wire::{
    FunctionPolicy, MessageInput, SendOptions, SendRequest, SendResponse, SessionInit,
    SessionMetricsResponse, StatusReport,
};

pub const TASK_SCHEMA: &str = "harness-e2e-task/v2";
pub const TASK_RESULT_SCHEMA: &str = "harness-e2e-task-result/v2";
pub const TASK_COMPARISON_SCHEMA: &str = "harness-e2e-task-comparison/v2";
pub const TASK_SUITE_SCHEMA: &str = "harness-e2e-task-suite/v2";
pub const TASK_SUITE_RESULT_SCHEMA: &str = "harness-e2e-task-suite-result/v2";
pub const TASK_SUITE_COMPARISON_SCHEMA: &str = "harness-e2e-task-suite-comparison/v1";
pub const OFFICIAL_VERIFIER_BUNDLE_SCHEMA: &str = "harness-e2e-official-verifier-bundle/v1";
const LEGACY_TASK_RESULT_SCHEMA: &str = "harness-e2e-task-result/v1";

#[derive(RustEmbed)]
#[folder = "tasks/"]
struct EmbeddedTasks;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub kind: TaskKind,
    #[serde(default)]
    pub execution_mode: TaskExecutionMode,
    pub source: TaskSource,
    pub execution: ExecutionPolicy,
    pub workspace: WorkspacePolicy,
    pub verifier: VerifierReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    CodePatch,
    Feature,
    CodeReview,
    Planning,
    OperationalRecovery,
    Endurance,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecutionMode {
    #[default]
    SingleTurn,
    StatefulSimulation,
    CheckpointLadder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSource {
    GitCheckout {
        repository: String,
        revision: String,
        subtree: String,
        manifest_sha256: String,
        path_env: String,
        required_paths: Vec<String>,
    },
    EmbeddedDirectory {
        path: String,
        manifest_sha256: String,
        required_paths: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePolicy {
    pub mutation: MutationPolicy,
    pub allowed_paths: Vec<String>,
    pub protected_paths: Vec<String>,
    pub ignored_paths: Vec<String>,
    pub minimum_changed_files: u16,
    pub maximum_changed_files: u16,
    pub maximum_patch_lines: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MutationPolicy {
    Allowlisted,
    ArtifactOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifierReference {
    pub profile: VerifierProfile,
    pub spec: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VerifierProfile {
    CodePatch,
    StructuredArtifact,
    StateRecovery,
    StateSimulation,
    CheckpointLadder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "profile", rename_all = "snake_case", deny_unknown_fields)]
pub enum VerifierSpec {
    CodePatch {
        baseline_must_fail: bool,
        public_commands: Vec<TaskCommand>,
        hidden_commands: Vec<TaskCommand>,
    },
    StructuredArtifact {
        artifact_path: String,
        schema: Value,
        assertions: Vec<JsonAssertion>,
    },
    StateRecovery {
        state_path: String,
        report_path: String,
        state_schema: Value,
        report_schema: Value,
        state_assertions: Vec<JsonAssertion>,
        report_assertions: Vec<JsonAssertion>,
    },
    StateSimulation {
        initial_state_path: String,
        actions_path: String,
    },
    CheckpointLadder {
        scenario_id: String,
        projection_schema: Value,
    },
}

impl VerifierSpec {
    fn profile(&self) -> VerifierProfile {
        match self {
            Self::CodePatch { .. } => VerifierProfile::CodePatch,
            Self::StructuredArtifact { .. } => VerifierProfile::StructuredArtifact,
            Self::StateRecovery { .. } => VerifierProfile::StateRecovery,
            Self::StateSimulation { .. } => VerifierProfile::StateSimulation,
            Self::CheckpointLadder { .. } => VerifierProfile::CheckpointLadder,
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VerificationDimension {
    #[default]
    Functional,
    StructuralIntegrity,
    Grounding,
    TechnicalReliability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskCommand {
    pub id: String,
    #[serde(default)]
    pub dimension: VerificationDimension,
    pub program: String,
    pub args: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JsonAssertion {
    pub id: String,
    #[serde(default)]
    pub dimension: VerificationDimension,
    pub pointer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals_from: Option<SourcePointer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourcePointer {
    pub file: String,
    pub pointer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompiledTask {
    pub source_path: String,
    pub instruction_path: String,
    pub verifier_path: String,
    pub instruction: String,
    pub instruction_sha256: String,
    pub verifier_sha256: String,
    pub behavior_sha256: String,
    pub definition: TaskDefinition,
    pub verifier: VerifierSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceEvidence {
    pub kind: String,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub root: String,
    pub subtree: String,
    pub manifest_sha256: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandResult {
    pub program: String,
    pub args: Vec<String>,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stdout_preview: String,
    pub stderr_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationCheck {
    pub id: String,
    #[serde(default)]
    pub dimension: VerificationDimension,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskVerification {
    pub passed: bool,
    pub changed_paths: Vec<String>,
    pub patch_lines: u32,
    pub checks: Vec<VerificationCheck>,
    pub commands: Vec<CommandResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    Passed,
    Failed,
    InfrastructureError,
    ResourceLimit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskVerifierMode {
    #[default]
    Development,
    Official,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskSystemManifest {
    pub lane: String,
    pub comparison_series: String,
    pub stack_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    pub runner_revision: String,
    pub platform: String,
    #[serde(default)]
    pub components: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfficialVerifierBundle {
    pub schema: String,
    pub id: String,
    pub tasks: BTreeMap<String, VerifierSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskRunResult {
    pub schema: String,
    pub execution_id: String,
    pub task_id: String,
    pub task_version: u32,
    pub task_kind: TaskKind,
    pub behavior_sha256: String,
    pub case_fingerprint: String,
    pub system_identity_sha256: String,
    #[serde(default)]
    pub cohort_identity_sha256: String,
    #[serde(default)]
    pub lane: String,
    #[serde(default)]
    pub comparison_series: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    #[serde(default)]
    pub verifier_mode: TaskVerifierMode,
    #[serde(default)]
    pub verifier_sha256: String,
    pub engine_version: String,
    pub harness_version: String,
    pub model: String,
    pub provider: String,
    pub status: TaskRunStatus,
    pub product_passed: bool,
    #[serde(default)]
    pub structural_integrity: bool,
    #[serde(default)]
    pub grounding_integrity: bool,
    #[serde(default)]
    pub technical_failure: bool,
    pub infrastructure_valid: bool,
    pub budget_passed: bool,
    pub coverage_complete: bool,
    pub wall_time_ms: u64,
    pub workspace: String,
    pub source: SourceEvidence,
    pub metrics: Option<SessionMetricsResponse>,
    pub transcript_sha256: Option<String>,
    pub verifier: Option<TaskVerification>,
    pub cleanup_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_failure: Option<String>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskRunConfig {
    pub url: String,
    pub task_id: String,
    pub model: String,
    pub provider: String,
    pub output: PathBuf,
    pub system: TaskSystemManifest,
    pub official_verifier_bundle: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskComparison {
    pub schema: String,
    pub task_id: String,
    pub case_fingerprint: String,
    pub baseline_system_identity_sha256: String,
    pub candidate_system_identity_sha256: String,
    pub baseline_passed: bool,
    pub candidate_passed: bool,
    pub capability_delta: Option<i8>,
    pub token_delta: Option<i64>,
    pub turn_delta: Option<i64>,
    pub function_error_delta: Option<i64>,
    pub wall_time_delta_ms: Option<i64>,
    pub comparable: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskSuiteDefinition {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub lane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_channel: Option<String>,
    #[serde(default)]
    pub official_verifier_required: bool,
    pub repetitions: u32,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskSuiteResult {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub suite_behavior_sha256: String,
    pub execution_id: String,
    pub lane: String,
    pub comparison_series: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    pub verifier_mode: TaskVerifierMode,
    pub verifier_sha256: String,
    pub cohort_identity_sha256: String,
    pub model: String,
    pub provider: String,
    pub requested_runs: u32,
    pub completed_runs: u32,
    pub product_passed_runs: u32,
    pub infrastructure_invalid_runs: u32,
    pub resource_limited_runs: u32,
    pub coverage_incomplete_runs: u32,
    pub task_results: Vec<String>,
    pub task_aggregates: Vec<TaskAggregate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RateEstimate {
    pub successes: u32,
    pub sample_size: u32,
    pub rate: f64,
    pub ci95_lower: f64,
    pub ci95_upper: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskAggregate {
    pub task_id: String,
    pub requested_runs: u32,
    pub included_runs: u32,
    pub excluded_infrastructure_runs: u32,
    pub maturity: String,
    pub product_success: Option<RateEstimate>,
    pub structural_integrity: Option<RateEstimate>,
    pub grounding_integrity: Option<RateEstimate>,
    pub technical_failure: Option<RateEstimate>,
    pub flaky_rate: Option<f64>,
    pub p50_total_tokens: Option<f64>,
    pub p95_total_tokens: Option<f64>,
    pub p50_turns: Option<f64>,
    pub p95_turns: Option<f64>,
    pub p50_wall_time_ms: Option<f64>,
    pub p95_wall_time_ms: Option<f64>,
    pub p50_billable_tokens: Option<f64>,
    pub p95_billable_tokens: Option<f64>,
    pub p50_function_calls: Option<f64>,
    pub p95_function_calls: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskAggregateDelta {
    pub task_id: String,
    pub baseline_maturity: String,
    pub candidate_maturity: String,
    pub product_success_rate: Option<f64>,
    pub structural_integrity_rate: Option<f64>,
    pub grounding_integrity_rate: Option<f64>,
    pub technical_failure_rate: Option<f64>,
    pub p50_total_tokens_ratio: Option<f64>,
    pub p50_turns_ratio: Option<f64>,
    pub p50_wall_time_ratio: Option<f64>,
    pub p50_billable_tokens_ratio: Option<f64>,
    pub p50_function_calls_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_cost_usd_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskSuiteComparison {
    pub schema: String,
    pub baseline_execution_id: String,
    pub candidate_execution_id: String,
    pub comparable: bool,
    pub advisory: bool,
    pub reason: String,
    pub deltas: Vec<TaskAggregateDelta>,
}

impl TaskSuiteDefinition {
    pub fn read(path: &Path) -> Result<Self> {
        let suite: Self = serde_json::from_slice(
            &fs::read(path).with_context(|| format!("read task suite {}", path.display()))?,
        )
        .with_context(|| format!("decode task suite {}", path.display()))?;
        suite.validate()?;
        Ok(suite)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != TASK_SUITE_SCHEMA {
            bail!("unsupported task suite schema '{}'", self.schema);
        }
        validate_slug(&self.id, "task suite id")?;
        validate_lane(&self.lane)?;
        if self.lane == "remote_release" && self.release_channel.is_none() {
            bail!("remote release suite '{}' needs a release channel", self.id);
        }
        if self.version == 0 || self.repetitions == 0 || self.repetitions > 20 {
            bail!(
                "task suite '{}' has invalid version or repetitions",
                self.id
            );
        }
        if self.tasks.is_empty() {
            bail!("task suite '{}' has no tasks", self.id);
        }
        let catalog = embedded_catalog()?
            .into_iter()
            .map(|task| task.definition.id)
            .collect::<BTreeSet<_>>();
        let mut selected = BTreeSet::new();
        for task in &self.tasks {
            validate_slug(task, "task suite member")?;
            if !catalog.contains(task) {
                bail!("task suite '{}' selects unknown task '{task}'", self.id);
            }
            if !selected.insert(task) {
                bail!("task suite '{}' repeats task '{task}'", self.id);
            }
        }
        Ok(())
    }
}

pub fn checked_in_task_suites() -> Result<Vec<(String, TaskSuiteDefinition)>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/task-suites");
    let mut paths = fs::read_dir(&directory)
        .with_context(|| format!("read task suite directory {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let suite = TaskSuiteDefinition::read(&path)?;
            Ok((path.display().to_string(), suite))
        })
        .collect()
}

impl TaskDefinition {
    pub fn validate(&self) -> Result<()> {
        if self.schema != TASK_SCHEMA {
            bail!(
                "task '{}' uses unsupported schema '{}'",
                self.id,
                self.schema
            );
        }
        validate_slug(&self.id, "task id")?;
        if self.version == 0 {
            bail!("task '{}' version must be positive", self.id);
        }
        validate_execution(self.execution, &self.id)?;
        validate_source(&self.source)?;
        self.workspace.validate(&self.id)?;
        validate_relative_path(&self.verifier.spec, "verifier spec")?;
        if Path::new(&self.verifier.spec).components().count() != 1 {
            bail!(
                "task '{}' verifier spec must be adjacent to task.toml",
                self.id
            );
        }
        match (self.kind, self.execution_mode, self.verifier.profile) {
            (
                TaskKind::CodePatch | TaskKind::Feature,
                TaskExecutionMode::SingleTurn,
                VerifierProfile::CodePatch,
            )
            | (
                TaskKind::CodeReview | TaskKind::Planning,
                TaskExecutionMode::SingleTurn,
                VerifierProfile::StructuredArtifact,
            )
            | (
                TaskKind::OperationalRecovery,
                TaskExecutionMode::SingleTurn,
                VerifierProfile::StateRecovery,
            )
            | (
                TaskKind::OperationalRecovery,
                TaskExecutionMode::StatefulSimulation,
                VerifierProfile::StateSimulation,
            )
            | (
                TaskKind::Endurance,
                TaskExecutionMode::CheckpointLadder,
                VerifierProfile::CheckpointLadder,
            ) => {}
            _ => bail!(
                "task '{}' kind and verifier profile are incompatible",
                self.id
            ),
        }
        Ok(())
    }
}

impl WorkspacePolicy {
    fn validate(&self, task_id: &str) -> Result<()> {
        if self.allowed_paths.is_empty() {
            bail!("task '{task_id}' has no allowed paths");
        }
        if self.minimum_changed_files == 0
            || self.minimum_changed_files > self.maximum_changed_files
            || self.maximum_patch_lines == 0
        {
            bail!("task '{task_id}' has invalid patch bounds");
        }
        for path in self.allowed_paths.iter().chain(&self.protected_paths) {
            validate_relative_path(path, "workspace policy path")?;
        }
        for pattern in &self.ignored_paths {
            if pattern.is_empty()
                || pattern.contains(['\\', '\n', '\r'])
                || pattern.starts_with(['/', '!'])
                || pattern.split('/').any(|component| component == "..")
            {
                bail!("task '{task_id}' has unsafe ignored path pattern '{pattern}'");
            }
        }
        let allowed = self.allowed_paths.iter().collect::<BTreeSet<_>>();
        if allowed.len() != self.allowed_paths.len() {
            bail!("task '{task_id}' repeats an allowed path");
        }
        Ok(())
    }
}

pub fn embedded_catalog() -> Result<Vec<CompiledTask>> {
    let mut paths = EmbeddedTasks::iter()
        .map(|path| path.into_owned())
        .filter(|path| path.ends_with("/task.toml"))
        .collect::<Vec<_>>();
    paths.sort();
    let tasks = paths
        .into_iter()
        .map(|path| compile_embedded(&path))
        .collect::<Result<Vec<_>>>()?;
    let mut ids = BTreeSet::new();
    for task in &tasks {
        if !ids.insert(task.definition.id.as_str()) {
            bail!("task catalog repeats id '{}'", task.definition.id);
        }
    }
    Ok(tasks)
}

pub fn embedded_task(id: &str) -> Result<CompiledTask> {
    embedded_catalog()?
        .into_iter()
        .find(|task| task.definition.id == id)
        .with_context(|| format!("unknown benchmark task '{id}'"))
}

pub fn compile(
    source_path: &str,
    source: &str,
    instruction: &str,
    verifier_bytes: &[u8],
) -> Result<CompiledTask> {
    let definition: TaskDefinition =
        toml::from_str(source).with_context(|| format!("parse task {source_path}"))?;
    definition.validate()?;
    validate_instruction(instruction)?;
    let verifier: VerifierSpec = serde_json::from_slice(verifier_bytes)
        .with_context(|| format!("parse verifier for task '{}'", definition.id))?;
    if verifier.profile() != definition.verifier.profile {
        bail!(
            "task '{}' verifier reference differs from verifier spec",
            definition.id
        );
    }
    validate_verifier(&verifier, &definition)?;
    let parent = Path::new(source_path)
        .parent()
        .context("task manifest has no parent")?;
    let instruction_path = parent
        .join("instruction.md")
        .to_string_lossy()
        .replace('\\', "/");
    let verifier_path = parent
        .join(&definition.verifier.spec)
        .to_string_lossy()
        .replace('\\', "/");
    let instruction_sha256 = sha256_bytes(instruction.as_bytes());
    let verifier_sha256 = sha256_bytes(verifier_bytes);
    let behavior_sha256 = artifact::sha256_value(&json!({
        "definition": definition,
        "instruction": instruction,
        "verifier_sha256": verifier_sha256,
    }))?;
    Ok(CompiledTask {
        source_path: source_path.to_string(),
        instruction_path,
        verifier_path,
        instruction: instruction.to_string(),
        instruction_sha256,
        verifier_sha256,
        behavior_sha256,
        definition,
        verifier,
    })
}

fn compile_embedded(source_path: &str) -> Result<CompiledTask> {
    let source = EmbeddedTasks::get(source_path)
        .with_context(|| format!("embedded task {source_path} is missing"))?;
    let source = std::str::from_utf8(source.data.as_ref())
        .with_context(|| format!("embedded task {source_path} is not UTF-8"))?;
    let parent = Path::new(source_path)
        .parent()
        .context("task manifest has no parent")?;
    let instruction_path = parent
        .join("instruction.md")
        .to_string_lossy()
        .replace('\\', "/");
    let verifier_name: TaskDefinition =
        toml::from_str(source).with_context(|| format!("parse task {source_path}"))?;
    let verifier_path = parent
        .join(&verifier_name.verifier.spec)
        .to_string_lossy()
        .replace('\\', "/");
    let instruction = EmbeddedTasks::get(&instruction_path)
        .with_context(|| format!("embedded task instruction {instruction_path} is missing"))?;
    let verifier = EmbeddedTasks::get(&verifier_path)
        .with_context(|| format!("embedded task verifier {verifier_path} is missing"))?;
    let instruction = std::str::from_utf8(instruction.data.as_ref())
        .with_context(|| format!("embedded task instruction {instruction_path} is not UTF-8"))?;
    compile(source_path, source, instruction, verifier.data.as_ref())
}

fn validate_verifier(verifier: &VerifierSpec, task: &TaskDefinition) -> Result<()> {
    match verifier {
        VerifierSpec::CodePatch {
            public_commands,
            hidden_commands,
            ..
        } => {
            if public_commands.is_empty() {
                bail!("task '{}' code verifier needs public commands", task.id);
            }
            let mut ids = BTreeSet::new();
            for command in public_commands.iter().chain(hidden_commands) {
                validate_command(command)?;
                if !ids.insert(command.id.as_str()) {
                    bail!(
                        "task '{}' repeats verifier command id '{}'",
                        task.id,
                        command.id
                    );
                }
            }
        }
        VerifierSpec::StructuredArtifact {
            artifact_path,
            schema,
            assertions,
        } => {
            validate_relative_path(artifact_path, "artifact path")?;
            validate_json_schema(schema, &task.id)?;
            validate_assertions(assertions)?;
            if !task
                .workspace
                .allowed_paths
                .iter()
                .any(|path| path == artifact_path)
            {
                bail!("task '{}' artifact is not an allowed path", task.id);
            }
        }
        VerifierSpec::StateRecovery {
            state_path,
            report_path,
            state_schema,
            report_schema,
            state_assertions,
            report_assertions,
        } => {
            validate_relative_path(state_path, "recovered state path")?;
            validate_relative_path(report_path, "recovery report path")?;
            validate_json_schema(state_schema, &task.id)?;
            validate_json_schema(report_schema, &task.id)?;
            validate_assertions(state_assertions)?;
            validate_assertions(report_assertions)?;
            for path in [state_path, report_path] {
                if !task
                    .workspace
                    .allowed_paths
                    .iter()
                    .any(|allowed| allowed == path)
                {
                    bail!("task '{}' recovery artifact is not allowed", task.id);
                }
            }
        }
        VerifierSpec::StateSimulation {
            initial_state_path,
            actions_path,
        } => {
            validate_relative_path(initial_state_path, "simulation initial state")?;
            validate_relative_path(actions_path, "simulation actions")?;
            if !task
                .workspace
                .allowed_paths
                .iter()
                .any(|allowed| allowed == actions_path)
            {
                bail!("task '{}' simulation actions are not allowed", task.id);
            }
        }
        VerifierSpec::CheckpointLadder {
            scenario_id,
            projection_schema,
        } => {
            validate_slug(scenario_id, "checkpoint scenario id")?;
            validate_json_schema(projection_schema, &task.id)?;
        }
    }
    Ok(())
}

fn validate_command(command: &TaskCommand) -> Result<()> {
    validate_slug(&command.id, "verifier command id")?;
    if command.program.trim().is_empty()
        || command.program.contains('/')
        || command.timeout_seconds == 0
        || command.timeout_seconds > 300
    {
        bail!("task verifier contains an invalid command envelope");
    }
    if command.args.iter().any(|argument| argument.contains('\0')) {
        bail!("task verifier command contains a NUL argument");
    }
    Ok(())
}

fn validate_json_schema(schema: &Value, task_id: &str) -> Result<()> {
    jsonschema::JSONSchema::compile(schema)
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("task '{task_id}' has invalid JSON schema: {error}"))
}

fn validate_assertions(assertions: &[JsonAssertion]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for assertion in assertions {
        validate_slug(&assertion.id, "verifier assertion id")?;
        if !ids.insert(assertion.id.as_str()) {
            bail!("task verifier repeats assertion id '{}'", assertion.id);
        }
        if !assertion.pointer.starts_with('/') {
            bail!("task verifier assertion pointer must start with '/'");
        }
        if assertion.equals.is_some() == assertion.equals_from.is_some() {
            bail!("task verifier assertion must define exactly one expected source");
        }
        if let Some(source) = &assertion.equals_from {
            validate_relative_path(&source.file, "assertion source file")?;
            if !source.pointer.starts_with('/') {
                bail!("task verifier source pointer must start with '/'");
            }
        }
    }
    Ok(())
}

fn validate_lane(value: &str) -> Result<()> {
    if !matches!(value, "local_development" | "remote_release") {
        bail!("task lane '{value}' must be local_development or remote_release");
    }
    Ok(())
}

impl Default for TaskSystemManifest {
    fn default() -> Self {
        Self {
            lane: "local_development".into(),
            comparison_series: "local".into(),
            stack_mode: "source".into(),
            release_channel: None,
            release_tag: None,
            runner_revision: option_env!("GIT_COMMIT").unwrap_or("unknown").into(),
            platform: std::env::consts::ARCH.into(),
            components: BTreeMap::new(),
        }
    }
}

impl TaskSystemManifest {
    pub fn read(path: &Path) -> Result<Self> {
        let manifest: Self = serde_json::from_slice(
            &fs::read(path)
                .with_context(|| format!("read task system manifest {}", path.display()))?,
        )
        .with_context(|| format!("decode task system manifest {}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        validate_lane(&self.lane)?;
        for (label, value) in [
            ("stack mode", self.stack_mode.as_str()),
            ("comparison series", self.comparison_series.as_str()),
            ("runner revision", self.runner_revision.as_str()),
            ("platform", self.platform.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("task system {label} is empty");
            }
        }
        if self.lane == "local_development" {
            if self.comparison_series != "local"
                || self.release_channel.is_some()
                || self.release_tag.is_some()
            {
                bail!("local task system identity must use the local comparison series");
            }
            return Ok(());
        }
        let channel = self
            .release_channel
            .as_deref()
            .filter(|value| !value.is_empty())
            .context("remote release system identity needs a channel")?;
        let tag = self
            .release_tag
            .as_deref()
            .filter(|value| !value.is_empty())
            .context("remote release system identity needs an immutable tag")?;
        let expected_series = match channel {
            "stable" => "stable".to_string(),
            "rc" => format!("rc:{}", tag.split("-rc.").next().unwrap_or(tag)),
            _ => bail!("remote release channel must be rc or stable"),
        };
        if self.comparison_series != expected_series {
            bail!(
                "remote release comparison series '{}' must be '{}'",
                self.comparison_series,
                expected_series
            );
        }
        Ok(())
    }
}

impl OfficialVerifierBundle {
    pub fn read(path: &Path) -> Result<(Self, String)> {
        let bytes = fs::read(path)
            .with_context(|| format!("read official verifier bundle {}", path.display()))?;
        let digest = sha256_bytes(&bytes);
        let bundle: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode official verifier bundle {}", path.display()))?;
        if bundle.schema != OFFICIAL_VERIFIER_BUNDLE_SCHEMA || bundle.id.trim().is_empty() {
            bail!("official verifier bundle has an invalid schema or id");
        }
        if bundle.tasks.is_empty() {
            bail!("official verifier bundle has no tasks");
        }
        Ok((bundle, digest))
    }

    fn verifier_for(&self, task: &CompiledTask) -> Result<VerifierSpec> {
        let verifier = self
            .tasks
            .get(&task.definition.id)
            .with_context(|| {
                format!(
                    "official verifier bundle omits task '{}'",
                    task.definition.id
                )
            })?
            .clone();
        if verifier.profile() != task.definition.verifier.profile {
            bail!(
                "official verifier profile differs for task '{}'",
                task.definition.id
            );
        }
        validate_verifier(&verifier, &task.definition)?;
        Ok(verifier)
    }
}

fn validate_instruction(instruction: &str) -> Result<()> {
    if instruction.trim().is_empty() {
        bail!("task instruction is empty");
    }
    let rendered = instruction.replace("{{workspace}}", "workspace");
    if rendered.contains("{{") || rendered.contains("}}") {
        bail!("task instruction contains an unknown or unclosed template");
    }
    Ok(())
}

pub fn observe_source(source: &TaskSource) -> Result<SourceEvidence> {
    match source {
        TaskSource::GitCheckout {
            repository,
            revision,
            subtree,
            manifest_sha256,
            path_env,
            required_paths,
        } => {
            let root = std::env::var_os(path_env)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .with_context(|| format!("{path_env} must point to the task fixture checkout"))?;
            if !root.is_absolute() {
                bail!("task fixture checkout must be absolute: {}", root.display());
            }
            let root = root
                .canonicalize()
                .with_context(|| format!("canonicalize task checkout {}", root.display()))?;
            let output = StdCommand::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "HEAD"])
                .stdin(Stdio::null())
                .output()
                .context("read task fixture revision")?;
            if !output.status.success() {
                bail!("task fixture is not a readable Git checkout");
            }
            let observed_revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if &observed_revision != revision {
                bail!("task fixture revision {observed_revision} differs from pinned {revision}");
            }
            observe_directory(
                &root.join(subtree),
                "git_checkout",
                Some(repository),
                Some(revision),
                subtree,
                manifest_sha256,
                required_paths,
            )
        }
        TaskSource::EmbeddedDirectory {
            path,
            manifest_sha256,
            required_paths,
        } => observe_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join(path),
            "embedded_directory",
            None,
            None,
            "",
            manifest_sha256,
            required_paths,
        ),
    }
}

fn observe_directory(
    directory: &Path,
    kind: &str,
    repository: Option<&str>,
    revision: Option<&str>,
    subtree: &str,
    manifest_sha256: &str,
    required_paths: &[String],
) -> Result<SourceEvidence> {
    if !directory.is_dir() {
        bail!("task fixture directory is missing: {}", directory.display());
    }
    let paths = collect_files(directory)?;
    let observed = paths.iter().cloned().collect::<BTreeSet<_>>();
    let expected = required_paths.iter().cloned().collect::<BTreeSet<_>>();
    if observed != expected {
        bail!("task fixture paths differ: expected {expected:?}, observed {observed:?}");
    }
    let observed_manifest = compute_manifest_sha256(directory, &paths)?;
    if observed_manifest != manifest_sha256 {
        bail!("task fixture manifest {observed_manifest} differs from pinned {manifest_sha256}");
    }
    Ok(SourceEvidence {
        kind: kind.to_string(),
        repository: repository.map(str::to_string),
        revision: revision.map(str::to_string),
        root: directory.display().to_string(),
        subtree: subtree.to_string(),
        manifest_sha256: observed_manifest,
        paths,
    })
}

pub fn compute_manifest_sha256(root: &Path, paths: &[String]) -> Result<String> {
    let mut concatenation = Vec::new();
    for relative in paths {
        let bytes = fs::read(root.join(relative))
            .with_context(|| format!("read task fixture asset {relative}"))?;
        let file_hash = sha256_bytes(&bytes);
        let hex = file_hash
            .strip_prefix("sha256:")
            .context("task fixture asset hash is not sha256-prefixed")?;
        concatenation.extend_from_slice(relative.as_bytes());
        concatenation.push(b'\n');
        concatenation.extend_from_slice(hex.as_bytes());
        concatenation.push(b'\n');
    }
    Ok(sha256_bytes(&concatenation))
}

fn collect_files(root: &Path) -> Result<Vec<String>> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(directory)
            .with_context(|| format!("read task fixture {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                bail!("task fixture contains symlink: {}", path.display());
            }
            if metadata.is_dir() {
                visit(root, &path, paths)?;
            } else if metadata.is_file() {
                paths.push(
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(root, root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn source_root(source: &TaskSource) -> Result<PathBuf> {
    match source {
        TaskSource::EmbeddedDirectory { path, .. } => {
            Ok(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        }
        TaskSource::GitCheckout {
            path_env, subtree, ..
        } => Ok(PathBuf::from(
            std::env::var_os(path_env)
                .filter(|value| !value.is_empty())
                .with_context(|| format!("{path_env} is required"))?,
        )
        .join(subtree)),
    }
}

fn materialize_workspace(task: &CompiledTask, destination: &Path) -> Result<SourceEvidence> {
    let evidence = observe_source(&task.definition.source)?;
    let source = source_root(&task.definition.source)?;
    if destination.exists() {
        bail!("task workspace already exists: {}", destination.display());
    }
    fs::create_dir_all(destination)
        .with_context(|| format!("create task workspace {}", destination.display()))?;
    copy_directory(&source, destination)?;
    run_git(destination, &["init", "--initial-branch", "benchmark"])?;
    write_git_excludes(destination, &task.definition.workspace.ignored_paths)?;
    run_git(destination, &["config", "user.name", "Harness E2E"])?;
    run_git(
        destination,
        &["config", "user.email", "harness-e2e@localhost"],
    )?;
    run_git(destination, &["add", "--all"])?;
    run_git(destination, &["commit", "-m", "Benchmark baseline"])?;
    Ok(evidence)
}

fn write_git_excludes(workspace: &Path, ignored_paths: &[String]) -> Result<()> {
    let mut contents = String::from("# Runner-owned transient task artifacts\n");
    for pattern in ignored_paths {
        contents.push_str(pattern);
        contents.push('\n');
    }
    let path = workspace.join(".git/info/exclude");
    fs::write(&path, contents)
        .with_context(|| format!("write task Git excludes {}", path.display()))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    for entry in
        fs::read_dir(source).with_context(|| format!("read task source {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            bail!("task source contains symlink: {}", source_path.display());
        }
        if metadata.is_dir() {
            fs::create_dir_all(&destination_path)?;
            copy_directory(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy task asset {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn validate_source(source: &TaskSource) -> Result<()> {
    let (manifest, paths) = match source {
        TaskSource::GitCheckout {
            repository,
            revision,
            subtree,
            manifest_sha256,
            path_env,
            required_paths,
        } => {
            validate_repository(repository)?;
            if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("task Git revision must be a full hexadecimal SHA");
            }
            validate_relative_path(subtree, "fixture subtree")?;
            if path_env.is_empty()
                || !path_env
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            {
                bail!("task fixture environment name is invalid");
            }
            (manifest_sha256, required_paths)
        }
        TaskSource::EmbeddedDirectory {
            path,
            manifest_sha256,
            required_paths,
        } => {
            validate_relative_path(path, "embedded source path")?;
            (manifest_sha256, required_paths)
        }
    };
    validate_sha256(manifest, "source manifest")?;
    if paths.is_empty() {
        bail!("task source has no required paths");
    }
    let mut unique = BTreeSet::new();
    for path in paths {
        validate_relative_path(path, "required source path")?;
        if !unique.insert(path) {
            bail!("task source repeats required path '{path}'");
        }
    }
    Ok(())
}

fn validate_execution(execution: ExecutionPolicy, task_id: &str) -> Result<()> {
    if execution.max_turns == 0 || execution.stuck_timeout_seconds == 0 {
        bail!("task '{task_id}' execution limits must be positive");
    }
    if execution.max_output_tokens == Some(0) || execution.max_total_tokens == Some(0) {
        bail!("task '{task_id}' token limits must be positive");
    }
    Ok(())
}

async fn run_task_command(root: &Path, command: &TaskCommand) -> CommandResult {
    let output = tokio::time::timeout(
        Duration::from_secs(command.timeout_seconds),
        Command::new(&command.program)
            .args(&command.args)
            .current_dir(root)
            .stdin(Stdio::null())
            .output(),
    )
    .await;
    match output {
        Ok(Ok(output)) => CommandResult {
            program: command.program.clone(),
            args: command.args.clone(),
            success: output.status.success(),
            exit_code: output.status.code(),
            timed_out: false,
            stdout_sha256: sha256_bytes(&output.stdout),
            stderr_sha256: sha256_bytes(&output.stderr),
            stdout_preview: bounded_preview(&output.stdout),
            stderr_preview: bounded_preview(&output.stderr),
        },
        Ok(Err(error)) => CommandResult {
            program: command.program.clone(),
            args: command.args.clone(),
            success: false,
            exit_code: None,
            timed_out: false,
            stdout_sha256: sha256_bytes(&[]),
            stderr_sha256: sha256_bytes(error.to_string().as_bytes()),
            stdout_preview: String::new(),
            stderr_preview: error.to_string(),
        },
        Err(_) => CommandResult {
            program: command.program.clone(),
            args: command.args.clone(),
            success: false,
            exit_code: None,
            timed_out: true,
            stdout_sha256: sha256_bytes(&[]),
            stderr_sha256: sha256_bytes(b"command timed out"),
            stdout_preview: String::new(),
            stderr_preview: "command timed out".into(),
        },
    }
}

fn bounded_preview(bytes: &[u8]) -> String {
    let end = bytes.len().min(2048);
    let preview = String::from_utf8_lossy(&bytes[..end]);
    crate::redaction::RedactionPolicy::from_environment()
        .redact_text(&preview)
        .0
}

async fn validate_baseline(task: &CompiledTask, workspace: &Path) -> Result<()> {
    if let VerifierSpec::CodePatch {
        baseline_must_fail,
        public_commands,
        ..
    } = &task.verifier
    {
        let mut results = Vec::new();
        for command in public_commands {
            results.push(run_task_command(workspace, command).await);
        }
        if *baseline_must_fail && results.iter().all(|result| result.success) {
            bail!(
                "task '{}' baseline is unexpectedly green",
                task.definition.id
            );
        }
        if results.iter().any(|result| result.timed_out) {
            bail!("task '{}' baseline command timed out", task.definition.id);
        }
    }
    Ok(())
}

pub async fn verify_workspace(task: &CompiledTask, workspace: &Path) -> Result<TaskVerification> {
    let changed_paths = changed_paths(workspace)?;
    let patch_lines = patch_lines(workspace, &changed_paths)?;
    let mut checks = workspace_checks(task, &changed_paths, patch_lines);
    let mut commands = Vec::new();
    match &task.verifier {
        VerifierSpec::CodePatch {
            public_commands,
            hidden_commands,
            ..
        } => {
            for command in public_commands.iter().chain(hidden_commands) {
                let result = run_task_command(workspace, command).await;
                checks.push(verification_check(
                    &command.id,
                    command.dimension,
                    result.success,
                    format!(
                        "{} verifier command exited {:?}, timed_out={}",
                        if public_commands.iter().any(|item| item.id == command.id) {
                            "public"
                        } else {
                            "runner-private"
                        },
                        result.exit_code,
                        result.timed_out
                    ),
                ));
                commands.push(result);
            }
        }
        VerifierSpec::StructuredArtifact {
            artifact_path,
            schema,
            assertions,
        } => {
            let (read_check, artifact) = subject_json(workspace, artifact_path, "artifact_read");
            checks.push(read_check);
            if let Some(artifact) = artifact {
                checks.push(schema_check("artifact_schema", schema, &artifact));
                checks.extend(assertion_checks(
                    workspace,
                    "artifact_assertion",
                    &artifact,
                    assertions,
                ));
            } else {
                checks.push(skipped_check(
                    "artifact_schema",
                    VerificationDimension::StructuralIntegrity,
                    "artifact schema could not be evaluated because the artifact is missing",
                ));
                checks.extend(assertions.iter().map(|assertion| {
                    skipped_check(
                        &assertion.id,
                        assertion.dimension,
                        "artifact assertion could not be evaluated because the artifact is missing",
                    )
                }));
            }
        }
        VerifierSpec::StateRecovery {
            state_path,
            report_path,
            state_schema,
            report_schema,
            state_assertions,
            report_assertions,
        } => {
            let (state_read, state) = subject_json(workspace, state_path, "recovered_state_read");
            let (report_read, report) =
                subject_json(workspace, report_path, "recovery_report_read");
            checks.push(state_read);
            checks.push(report_read);
            if let Some(state) = state {
                checks.push(schema_check("recovered_state_schema", state_schema, &state));
                checks.extend(assertion_checks(
                    workspace,
                    "state_assertion",
                    &state,
                    state_assertions,
                ));
            } else {
                checks.push(skipped_check(
                    "recovered_state_schema",
                    VerificationDimension::StructuralIntegrity,
                    "state schema could not be evaluated because recovered state is missing",
                ));
                checks.extend(state_assertions.iter().map(|assertion| {
                    skipped_check(
                        &assertion.id,
                        assertion.dimension,
                        "state assertion could not be evaluated because recovered state is missing",
                    )
                }));
            }
            if let Some(report) = report {
                checks.push(schema_check(
                    "recovery_report_schema",
                    report_schema,
                    &report,
                ));
                checks.extend(assertion_checks(
                    workspace,
                    "report_assertion",
                    &report,
                    report_assertions,
                ));
            } else {
                checks.push(skipped_check(
                    "recovery_report_schema",
                    VerificationDimension::StructuralIntegrity,
                    "report schema could not be evaluated because the recovery report is missing",
                ));
                checks.extend(report_assertions.iter().map(|assertion| {
                    skipped_check(
                        &assertion.id,
                        assertion.dimension,
                        "report assertion could not be evaluated because the recovery report is missing",
                    )
                }));
            }
        }
        VerifierSpec::StateSimulation {
            initial_state_path,
            actions_path,
        } => {
            checks.extend(simulate_release_recovery(
                workspace,
                initial_state_path,
                actions_path,
            ));
        }
        VerifierSpec::CheckpointLadder { .. } => {
            checks.push(verification_check(
                "checkpoint_ladder_external_runner",
                VerificationDimension::TechnicalReliability,
                false,
                "checkpoint ladders are executed by the scenario runner and projected in shadow",
            ));
        }
    }
    Ok(TaskVerification {
        passed: checks.iter().all(|check| check.passed),
        changed_paths,
        patch_lines,
        checks,
        commands,
    })
}

fn workspace_checks(
    task: &CompiledTask,
    changed_paths: &[String],
    patch_lines: u32,
) -> Vec<VerificationCheck> {
    let policy = &task.definition.workspace;
    let allowed = changed_paths.iter().all(|path| {
        policy
            .allowed_paths
            .iter()
            .any(|allowed| policy_path_matches(path, allowed))
    });
    let protected = changed_paths.iter().all(|path| {
        !policy
            .protected_paths
            .iter()
            .any(|protected| policy_path_matches(path, protected))
    });
    let changed_count = u16::try_from(changed_paths.len()).unwrap_or(u16::MAX);
    vec![
        VerificationCheck {
            id: "allowed_paths_only".into(),
            dimension: VerificationDimension::StructuralIntegrity,
            passed: allowed,
            detail: format!("changed paths: {changed_paths:?}"),
        },
        VerificationCheck {
            id: "protected_paths_exact".into(),
            dimension: VerificationDimension::StructuralIntegrity,
            passed: protected,
            detail: "protected fixture paths must remain byte-identical".into(),
        },
        VerificationCheck {
            id: "changed_file_budget".into(),
            dimension: VerificationDimension::StructuralIntegrity,
            passed: changed_count >= policy.minimum_changed_files
                && changed_count <= policy.maximum_changed_files,
            detail: format!(
                "changed {changed_count} file(s), expected {}..={}",
                policy.minimum_changed_files, policy.maximum_changed_files
            ),
        },
        VerificationCheck {
            id: "patch_line_budget".into(),
            dimension: VerificationDimension::StructuralIntegrity,
            passed: patch_lines <= policy.maximum_patch_lines,
            detail: format!(
                "patch has {patch_lines} changed line(s), maximum {}",
                policy.maximum_patch_lines
            ),
        },
    ]
}

fn changed_paths(workspace: &Path) -> Result<Vec<String>> {
    let output = run_git(
        workspace,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    let mut paths = output
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .split_once(' ')
                .map(|(_, path)| path.trim_start())
        })
        .map(|path| path.split(" -> ").last().unwrap_or(path).to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn patch_lines(workspace: &Path, paths: &[String]) -> Result<u32> {
    let tracked = run_git(workspace, &["diff", "--numstat", "HEAD", "--"])?;
    let mut total = tracked.lines().try_fold(0_u32, |total, line| {
        let mut fields = line.split('\t');
        let added = fields.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
        let removed = fields.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
        total.checked_add(added.saturating_add(removed))
    });
    for path in paths {
        let full = workspace.join(path);
        if full.is_file() && run_git(workspace, &["ls-files", "--error-unmatch", path]).is_err() {
            let bytes = fs::read(&full)?;
            let lines = bytes.split(|byte| *byte == b'\n').count();
            total = total.and_then(|value| value.checked_add(lines.try_into().ok()?));
        }
    }
    total.context("task patch line count overflowed")
}

fn policy_path_matches(path: &str, policy: &str) -> bool {
    path == policy
        || path
            .strip_prefix(policy)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn read_json(workspace: &Path, relative: &str) -> Result<Value> {
    let bytes = fs::read(workspace.join(relative))
        .with_context(|| format!("read task artifact {relative}"))?;
    if bytes.len() > 256 * 1024 {
        bail!("task artifact {relative} exceeds 256 KiB");
    }
    serde_json::from_slice(&bytes).with_context(|| format!("decode task artifact {relative}"))
}

fn subject_json(
    workspace: &Path,
    relative: &str,
    check_id: &str,
) -> (VerificationCheck, Option<Value>) {
    match read_json(workspace, relative) {
        Ok(value) => (
            VerificationCheck {
                id: check_id.into(),
                dimension: VerificationDimension::StructuralIntegrity,
                passed: true,
                detail: format!("read subject artifact {relative}"),
            },
            Some(value),
        ),
        Err(error) => (
            VerificationCheck {
                id: check_id.into(),
                dimension: VerificationDimension::StructuralIntegrity,
                passed: false,
                detail: format!("{error:#}"),
            },
            None,
        ),
    }
}

fn schema_check(id: &str, schema: &Value, value: &Value) -> VerificationCheck {
    let result = match jsonschema::JSONSchema::compile(schema) {
        Ok(validator) => validator.validate(value).map_err(|mut errors| {
            errors
                .next()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "JSON schema validation failed".into())
        }),
        Err(error) => Err(error.to_string()),
    };
    match result {
        Ok(()) => VerificationCheck {
            id: id.into(),
            dimension: VerificationDimension::StructuralIntegrity,
            passed: true,
            detail: "artifact matches the deterministic JSON schema".into(),
        },
        Err(error) => VerificationCheck {
            id: id.into(),
            dimension: VerificationDimension::StructuralIntegrity,
            passed: false,
            detail: error,
        },
    }
}

fn assertion_checks(
    workspace: &Path,
    _prefix: &str,
    value: &Value,
    assertions: &[JsonAssertion],
) -> Vec<VerificationCheck> {
    assertions
        .iter()
        .map(|assertion| {
            let expected = assertion.equals.clone().or_else(|| {
                let source = assertion.equals_from.as_ref()?;
                read_json(workspace, &source.file)
                    .ok()?
                    .pointer(&source.pointer)
                    .cloned()
            });
            let observed = value.pointer(&assertion.pointer);
            VerificationCheck {
                id: assertion.id.clone(),
                dimension: assertion.dimension,
                passed: observed == expected.as_ref(),
                detail: format!(
                    "{} observed={}, expected={}",
                    assertion.pointer,
                    observed
                        .map(Value::to_string)
                        .unwrap_or_else(|| "missing".into()),
                    expected
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "missing".into())
                ),
            }
        })
        .collect()
}

fn verification_check(
    id: &str,
    dimension: VerificationDimension,
    passed: bool,
    detail: impl Into<String>,
) -> VerificationCheck {
    VerificationCheck {
        id: id.into(),
        dimension,
        passed,
        detail: detail.into(),
    }
}

fn skipped_check(
    id: &str,
    dimension: VerificationDimension,
    detail: impl Into<String>,
) -> VerificationCheck {
    verification_check(id, dimension, false, detail)
}

fn dimension_passed(verification: &TaskVerification, dimension: VerificationDimension) -> bool {
    verification
        .checks
        .iter()
        .filter(|check| check.dimension == dimension)
        .all(|check| check.passed)
}

fn expected_check_ids(task: &CompiledTask) -> BTreeSet<String> {
    let mut expected = BTreeSet::from([
        "allowed_paths_only".into(),
        "protected_paths_exact".into(),
        "changed_file_budget".into(),
        "patch_line_budget".into(),
    ]);
    match &task.verifier {
        VerifierSpec::CodePatch {
            public_commands,
            hidden_commands,
            ..
        } => {
            expected.extend(
                public_commands
                    .iter()
                    .chain(hidden_commands)
                    .map(|command| command.id.clone()),
            );
        }
        VerifierSpec::StructuredArtifact { assertions, .. } => {
            expected.extend(["artifact_read".into(), "artifact_schema".into()]);
            expected.extend(assertions.iter().map(|assertion| assertion.id.clone()));
        }
        VerifierSpec::StateRecovery {
            state_assertions,
            report_assertions,
            ..
        } => {
            expected.extend([
                "recovered_state_read".into(),
                "recovery_report_read".into(),
                "recovered_state_schema".into(),
                "recovery_report_schema".into(),
            ]);
            expected.extend(
                state_assertions
                    .iter()
                    .chain(report_assertions)
                    .map(|assertion| assertion.id.clone()),
            );
        }
        VerifierSpec::StateSimulation { .. } => {
            expected.extend(
                [
                    "simulation_state_read",
                    "simulation_actions_read",
                    "recovery_action_count",
                    "no_forbidden_operation",
                    "recovery_sequence",
                    "reuse_immutable_run",
                    "reuse_immutable_version",
                    "cas_uses_observed_latest",
                    "fresh_operation",
                    "execute_fresh_operation",
                ]
                .into_iter()
                .map(str::to_string),
            );
        }
        VerifierSpec::CheckpointLadder { .. } => {
            expected.insert("checkpoint_ladder_external_runner".into());
        }
    }
    expected
}

fn coverage_is_complete(task: &CompiledTask, verification: &TaskVerification) -> bool {
    let observed = verification
        .checks
        .iter()
        .map(|check| check.id.clone())
        .collect::<BTreeSet<_>>();
    observed.len() == verification.checks.len() && observed == expected_check_ids(task)
}

fn simulate_release_recovery(
    workspace: &Path,
    initial_state_path: &str,
    actions_path: &str,
) -> Vec<VerificationCheck> {
    let (initial_read, initial) =
        subject_json(workspace, initial_state_path, "simulation_state_read");
    let (actions_read, actions) = subject_json(workspace, actions_path, "simulation_actions_read");
    let mut checks = vec![initial_read, actions_read];
    let (Some(initial), Some(actions)) = (initial, actions) else {
        checks.extend([
            skipped_check(
                "recovery_action_count",
                VerificationDimension::StructuralIntegrity,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "no_forbidden_operation",
                VerificationDimension::Grounding,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "recovery_sequence",
                VerificationDimension::Functional,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "reuse_immutable_run",
                VerificationDimension::Grounding,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "reuse_immutable_version",
                VerificationDimension::Grounding,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "cas_uses_observed_latest",
                VerificationDimension::Grounding,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "fresh_operation",
                VerificationDimension::Functional,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
            skipped_check(
                "execute_fresh_operation",
                VerificationDimension::Functional,
                "recovery actions could not be evaluated because simulation input is missing",
            ),
        ]);
        return checks;
    };
    let sequence = actions
        .get("actions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    checks.push(verification_check(
        "recovery_action_count",
        VerificationDimension::StructuralIntegrity,
        sequence.len() == 3,
        format!("observed {} action(s), expected 3", sequence.len()),
    ));
    let names = sequence
        .iter()
        .filter_map(|action| action.get("action").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let forbidden = [
        "retag",
        "bump_version",
        "retry_stale_operation",
        "set_channel_directly",
    ];
    checks.push(verification_check(
        "no_forbidden_operation",
        VerificationDimension::Grounding,
        names.iter().all(|name| !forbidden.contains(name)),
        format!("observed actions: {names:?}"),
    ));
    let expected_names = ["retry_run", "create_gated_operation", "execute_promotion"];
    checks.push(verification_check(
        "recovery_sequence",
        VerificationDimension::Functional,
        names == expected_names,
        format!("observed actions: {names:?}"),
    ));
    let retry = sequence.first();
    let operation = sequence.get(1);
    let execute = sequence.get(2);
    let run_id = initial.pointer("/run/run_id");
    let current_attempt = initial
        .pointer("/run/attempt")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    checks.push(verification_check(
        "reuse_immutable_run",
        VerificationDimension::Grounding,
        retry.and_then(|value| value.get("run_id")) == run_id
            && retry
                .and_then(|value| value.get("run_attempt"))
                .and_then(Value::as_u64)
                == Some(current_attempt.saturating_add(1)),
        "retry must reuse run_id and advance exactly one run attempt",
    ));
    let expected_version = initial.pointer("/tag/version");
    let expected_latest = initial.pointer("/latest_version");
    let stale_operation = initial.pointer("/stale_operation/id");
    let operation_id = operation.and_then(|value| value.get("operation"));
    checks.push(verification_check(
        "reuse_immutable_version",
        VerificationDimension::Grounding,
        operation.and_then(|value| value.get("version")) == expected_version,
        "gated operation must reuse the immutable published version",
    ));
    checks.push(verification_check(
        "cas_uses_observed_latest",
        VerificationDimension::Grounding,
        operation.and_then(|value| value.get("expected_latest")) == expected_latest,
        "CAS expectation must equal the observed latest pointer",
    ));
    checks.push(verification_check(
        "fresh_operation",
        VerificationDimension::Functional,
        operation_id.is_some() && operation_id != stale_operation,
        "promotion must use a fresh operation id",
    ));
    checks.push(verification_check(
        "execute_fresh_operation",
        VerificationDimension::Functional,
        execute.and_then(|value| value.get("operation")) == operation_id,
        "terminal promotion must execute the newly-created operation",
    ));
    checks
}

pub async fn run_task(config: TaskRunConfig) -> Result<(TaskRunResult, PathBuf)> {
    config.system.validate()?;
    let mut task = embedded_task(&config.task_id)?;
    let (verifier_mode, verifier_sha256) = if let Some(path) = &config.official_verifier_bundle {
        let (bundle, digest) = OfficialVerifierBundle::read(path)?;
        task.verifier = bundle.verifier_for(&task)?;
        (TaskVerifierMode::Official, digest)
    } else {
        (TaskVerifierMode::Development, task.verifier_sha256.clone())
    };
    let execution_id = Uuid::new_v4().simple().to_string();
    let run_root =
        create_absolute_directory(&config.output.join(&execution_id).join(&task.definition.id))?;
    let workspace = run_root.join("workspace");
    artifact::write_json(
        &run_root,
        Path::new("run-envelope.json"),
        "task_run_envelope",
        "task_run_envelope",
        &json!({
            "task_id": task.definition.id,
            "task_version": task.definition.version,
            "behavior_sha256": task.behavior_sha256,
            "verifier_mode": verifier_mode,
            "verifier_sha256": verifier_sha256,
            "model": config.model,
            "provider": config.provider,
            "system": config.system,
        }),
    )?;
    let source = materialize_workspace(&task, &workspace)?;
    validate_baseline(&task, &workspace).await?;

    let started = Instant::now();
    let context = E2eContext::connect(&config.url)
        .await
        .context("connect to Harness task runtime")?;
    let versions = context.runtime_versions().await?;
    if !context.function_exists("harness::send").await? {
        context.shutdown().await;
        bail!("task runtime does not expose harness::send");
    }
    let case_fingerprint = artifact::sha256_value(&json!({
        "task_behavior": task.behavior_sha256,
        "source_manifest": source.manifest_sha256,
        "model": config.model,
        "provider": config.provider,
        "execution": task.definition.execution,
        "lane": config.system.lane,
        "comparison_series": config.system.comparison_series,
        "release_channel": config.system.release_channel,
        "verifier_mode": verifier_mode,
        "verifier_sha256": verifier_sha256,
    }))?;
    let system_identity_sha256 = artifact::sha256_value(&json!({
        "engine": versions.engine,
        "harness": versions.harness,
        "declared": config.system,
    }))?;
    let cohort_identity_sha256 = artifact::sha256_value(&json!({
        "engine": versions.engine,
        "lane": config.system.lane,
        "comparison_series": config.system.comparison_series,
        "stack_mode": config.system.stack_mode,
        "release_channel": config.system.release_channel,
        "runner_revision": config.system.runner_revision,
        "platform": config.system.platform,
        "components": config.system.components,
        "model": config.model,
        "provider": config.provider,
    }))?;
    let session_id = format!("benchmark_task_{execution_id}");
    let instruction = task
        .instruction
        .replace("{{workspace}}", &workspace.display().to_string());

    let mut metrics = None;
    let mut transcript_sha256 = None;
    let mut verification = None;
    let mut failure = None;
    let mut subject_failure = None;
    let mut cleanup_valid = false;
    let execution_result: Result<()> = async {
        context.bind_turn_completed().await?;
        let response: SendResponse = context
            .trigger(
                "harness::send",
                SendRequest {
                    session_id: Some(session_id.clone()),
                    message: MessageInput::Text(instruction),
                    model: Some(config.model.clone()),
                    provider: Some(config.provider.clone()),
                    idempotency_key: Some(format!(
                        "benchmark-task:{}:{}:{execution_id}",
                        task.definition.id, task.definition.version
                    )),
                    session: Some(SessionInit {
                        title: Some(format!("Harness benchmark: {}", task.definition.id)),
                        metadata: Some(json!({
                            "benchmark_task_id": task.definition.id,
                            "benchmark_task_version": task.definition.version,
                            "benchmark_case_fingerprint": case_fingerprint,
                        })),
                    }),
                    options: Some(SendOptions {
                        max_turns: Some(task.definition.execution.max_turns),
                        max_output_tokens: task.definition.execution.max_output_tokens,
                        max_total_tokens: task.definition.execution.max_total_tokens,
                        max_validation_retries: task.definition.execution.max_validation_retries,
                        functions: Some(FunctionPolicy {
                            allow: vec![
                                "engine::functions::list".into(),
                                "engine::functions::info".into(),
                                "coder::*".into(),
                                "shell::exec".into(),
                            ],
                            deny: vec!["e2e::*".into(), "harness::filesystem::*".into()],
                            ..FunctionPolicy::default()
                        }),
                        metadata: Some(json!({
                            "fs_scope": { "root": workspace.display().to_string() }
                        })),
                    }),
                },
            )
            .await?;
        if !response.accepted
            || response.session_id != session_id
            || response.merged == Some(true)
            || response.queued == Some(true)
        {
            bail!("harness::send rejected benchmark task: {response:?}");
        }
        match context
            .wait_for_turn(
                &task.definition.id,
                &session_id,
                &response.turn_id,
                Duration::from_secs(task.definition.execution.stuck_timeout_seconds),
                true,
                None,
            )
            .await
        {
            Ok(observed) => metrics = Some(observed),
            Err(error) => {
                subject_failure = Some(format!(
                    "Harness task did not complete successfully: {error:#}"
                ));
            }
        }
        let status: Option<StatusReport> = context
            .trigger("harness::status", json!({"session_id": session_id}))
            .await?;
        let status = status.context("harness::status returned no task session")?;
        if status.result_error.is_some() {
            subject_failure = status.result_error.clone();
        }
        let transcript = context.transcript(&session_id).await?;
        transcript_sha256 = Some(artifact::sha256_value(&transcript)?);
        let mut observed = verify_workspace(&task, &workspace).await?;
        if verifier_mode == TaskVerifierMode::Official {
            redact_official_verification(&mut observed);
        }
        verification = Some(observed);
        Ok(())
    }
    .await;

    if let Err(error) = execution_result {
        failure = Some(format!("{error:#}"));
    }
    if let Err(error) = context.unbind_turn_completed().await {
        failure.get_or_insert_with(|| format!("unbind task observation: {error:#}"));
    }
    match context.teardown(&session_id).await {
        Ok(_) => cleanup_valid = true,
        Err(error) => {
            failure.get_or_insert_with(|| format!("task cleanup failed: {error:#}"));
        }
    }
    context.shutdown().await;

    let product_passed = verification
        .as_ref()
        .is_some_and(|verification| verification.passed)
        && subject_failure.is_none();
    let infrastructure_valid = failure.is_none() && cleanup_valid;
    let budget_passed = metrics.as_ref().is_none_or(|metrics| {
        task.definition
            .execution
            .max_total_tokens
            .is_none_or(|limit| {
                metrics
                    .totals
                    .input_tokens
                    .unwrap_or(0)
                    .saturating_add(metrics.totals.output_tokens.unwrap_or(0))
                    <= limit
            })
    });
    let coverage_complete = verification
        .as_ref()
        .is_some_and(|verification| coverage_is_complete(&task, verification));
    let structural_integrity = verification.as_ref().is_some_and(|verification| {
        dimension_passed(verification, VerificationDimension::StructuralIntegrity)
    });
    let grounding_integrity = verification.as_ref().is_some_and(|verification| {
        dimension_passed(verification, VerificationDimension::Grounding)
    });
    let technical_failure = subject_failure.is_some() || !budget_passed;
    let status = if !infrastructure_valid {
        TaskRunStatus::InfrastructureError
    } else if !budget_passed {
        TaskRunStatus::ResourceLimit
    } else if product_passed && coverage_complete {
        TaskRunStatus::Passed
    } else {
        TaskRunStatus::Failed
    };
    let result = TaskRunResult {
        schema: TASK_RESULT_SCHEMA.into(),
        execution_id,
        task_id: task.definition.id.clone(),
        task_version: task.definition.version,
        task_kind: task.definition.kind,
        behavior_sha256: task.behavior_sha256,
        case_fingerprint,
        system_identity_sha256,
        cohort_identity_sha256,
        lane: config.system.lane,
        comparison_series: config.system.comparison_series,
        release_channel: config.system.release_channel,
        release_tag: config.system.release_tag,
        verifier_mode,
        verifier_sha256,
        engine_version: versions.engine,
        harness_version: versions.harness,
        model: config.model,
        provider: config.provider,
        status,
        product_passed,
        structural_integrity,
        grounding_integrity,
        technical_failure,
        infrastructure_valid,
        budget_passed,
        coverage_complete,
        wall_time_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        workspace: workspace.display().to_string(),
        source,
        metrics,
        transcript_sha256,
        verifier: verification,
        cleanup_valid,
        subject_failure,
        failure,
    };
    let reference = artifact::write_json(
        &run_root,
        Path::new("task-result.json"),
        "task_result",
        "task_result",
        &result,
    )?;
    let path = run_root.join(reference.path);
    Ok((result, path))
}

pub async fn run_task_suite(
    suite: &TaskSuiteDefinition,
    url: &str,
    model: &str,
    provider: &str,
    output: &Path,
    system: &TaskSystemManifest,
    official_verifier_bundle: Option<&Path>,
) -> Result<(TaskSuiteResult, PathBuf)> {
    suite.validate()?;
    system.validate()?;
    if suite.lane != system.lane || suite.release_channel != system.release_channel {
        bail!("task suite lane or release channel differs from the system manifest");
    }
    if suite.official_verifier_required && official_verifier_bundle.is_none() {
        bail!(
            "task suite '{}' requires an official verifier bundle",
            suite.id
        );
    }
    let suite_execution_id = Uuid::new_v4().simple().to_string();
    let suite_root = create_absolute_directory(&output.join(&suite.id).join(&suite_execution_id))?;
    let catalog = embedded_catalog()?
        .into_iter()
        .map(|task| (task.definition.id.clone(), task))
        .collect::<BTreeMap<_, _>>();
    let suite_behavior_sha256 = artifact::sha256_value(&json!({
        "suite": suite,
        "tasks": suite.tasks.iter().map(|id| &catalog[id].behavior_sha256).collect::<Vec<_>>(),
    }))?;
    let (verifier_mode, verifier_sha256) = match official_verifier_bundle {
        Some(path) => {
            let (bundle, digest) = OfficialVerifierBundle::read(path)?;
            for task_id in &suite.tasks {
                bundle.verifier_for(&catalog[task_id])?;
            }
            (TaskVerifierMode::Official, digest)
        }
        None => {
            let digest = artifact::sha256_value(
                &suite
                    .tasks
                    .iter()
                    .map(|id| (&catalog[id].definition.id, &catalog[id].verifier_sha256))
                    .collect::<Vec<_>>(),
            )?;
            (TaskVerifierMode::Development, digest)
        }
    };
    let requested_runs = u32::try_from(suite.tasks.len())
        .unwrap_or(u32::MAX)
        .saturating_mul(suite.repetitions);
    let mut results = Vec::new();
    let mut paths = Vec::new();
    for repetition in 0..suite.repetitions {
        for task_id in &suite.tasks {
            let task_output = suite_root.join(format!("repetition-{}", repetition + 1));
            let task_config = TaskRunConfig {
                url: url.to_string(),
                task_id: task_id.clone(),
                model: model.to_string(),
                provider: provider.to_string(),
                output: task_output.clone(),
                system: system.clone(),
                official_verifier_bundle: official_verifier_bundle.map(Path::to_path_buf),
            };
            let (result, path) = match run_task(task_config.clone()).await {
                Ok(value) => value,
                Err(error) => persist_infrastructure_failure(
                    &task_output,
                    &catalog[task_id],
                    &task_config,
                    verifier_mode,
                    &verifier_sha256,
                    &format!("{error:#}"),
                )?,
            };
            paths.push(
                path.strip_prefix(&suite_root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            results.push(result);
        }
    }
    let summary = TaskSuiteResult {
        schema: TASK_SUITE_RESULT_SCHEMA.into(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        suite_behavior_sha256,
        execution_id: suite_execution_id,
        lane: suite.lane.clone(),
        comparison_series: system.comparison_series.clone(),
        release_channel: suite.release_channel.clone(),
        release_tag: system.release_tag.clone(),
        verifier_mode,
        verifier_sha256,
        cohort_identity_sha256: common_cohort_identity(&results),
        model: model.to_string(),
        provider: provider.to_string(),
        requested_runs,
        completed_runs: u32::try_from(results.len()).unwrap_or(u32::MAX),
        product_passed_runs: u32::try_from(
            results
                .iter()
                .filter(|result| result.product_passed)
                .count(),
        )
        .unwrap_or(u32::MAX),
        infrastructure_invalid_runs: u32::try_from(
            results
                .iter()
                .filter(|result| !result.infrastructure_valid)
                .count(),
        )
        .unwrap_or(u32::MAX),
        resource_limited_runs: u32::try_from(
            results
                .iter()
                .filter(|result| !result.budget_passed)
                .count(),
        )
        .unwrap_or(u32::MAX),
        coverage_incomplete_runs: u32::try_from(
            results
                .iter()
                .filter(|result| !result.coverage_complete)
                .count(),
        )
        .unwrap_or(u32::MAX),
        task_results: paths,
        task_aggregates: aggregate_task_results(suite, &results),
    };
    let reference = artifact::write_json(
        &suite_root,
        Path::new("suite-result.json"),
        "task_suite_result",
        "task_suite_result",
        &summary,
    )?;
    let path = suite_root.join(reference.path);
    Ok((summary, path))
}

fn persist_infrastructure_failure(
    output: &Path,
    task: &CompiledTask,
    config: &TaskRunConfig,
    verifier_mode: TaskVerifierMode,
    verifier_sha256: &str,
    failure: &str,
) -> Result<(TaskRunResult, PathBuf)> {
    let execution_id = Uuid::new_v4().simple().to_string();
    let run_root =
        create_absolute_directory(&output.join(&execution_id).join(&task.definition.id))?;
    let source = source_without_materialization(&task.definition.source);
    let case_fingerprint = artifact::sha256_value(&json!({
        "task_behavior": task.behavior_sha256,
        "source_manifest": source.manifest_sha256,
        "model": config.model,
        "provider": config.provider,
        "execution": task.definition.execution,
        "lane": config.system.lane,
        "comparison_series": config.system.comparison_series,
        "release_channel": config.system.release_channel,
        "verifier_mode": verifier_mode,
        "verifier_sha256": verifier_sha256,
    }))?;
    let cohort_identity_sha256 =
        declared_cohort_identity(&config.system, &config.model, &config.provider)?;
    let system_identity_sha256 = artifact::sha256_value(&json!({
        "engine": "unobserved",
        "harness": "unobserved",
        "declared": config.system,
    }))?;
    let result = TaskRunResult {
        schema: TASK_RESULT_SCHEMA.into(),
        execution_id,
        task_id: task.definition.id.clone(),
        task_version: task.definition.version,
        task_kind: task.definition.kind,
        behavior_sha256: task.behavior_sha256.clone(),
        case_fingerprint,
        system_identity_sha256,
        cohort_identity_sha256,
        lane: config.system.lane.clone(),
        comparison_series: config.system.comparison_series.clone(),
        release_channel: config.system.release_channel.clone(),
        release_tag: config.system.release_tag.clone(),
        verifier_mode,
        verifier_sha256: verifier_sha256.into(),
        engine_version: "unobserved".into(),
        harness_version: "unobserved".into(),
        model: config.model.clone(),
        provider: config.provider.clone(),
        status: TaskRunStatus::InfrastructureError,
        product_passed: false,
        structural_integrity: false,
        grounding_integrity: false,
        technical_failure: false,
        infrastructure_valid: false,
        budget_passed: true,
        coverage_complete: false,
        wall_time_ms: 0,
        workspace: String::new(),
        source,
        metrics: None,
        transcript_sha256: None,
        verifier: None,
        cleanup_valid: false,
        subject_failure: None,
        failure: Some(failure.into()),
    };
    let reference = artifact::write_json(
        &run_root,
        Path::new("task-result.json"),
        "task_result",
        "task_result",
        &result,
    )?;
    let path = run_root.join(reference.path);
    Ok((result, path))
}

fn source_without_materialization(source: &TaskSource) -> SourceEvidence {
    match source {
        TaskSource::GitCheckout {
            repository,
            revision,
            subtree,
            manifest_sha256,
            required_paths,
            ..
        } => SourceEvidence {
            kind: "git_checkout".into(),
            repository: Some(repository.clone()),
            revision: Some(revision.clone()),
            root: String::new(),
            subtree: subtree.clone(),
            manifest_sha256: manifest_sha256.clone(),
            paths: required_paths.clone(),
        },
        TaskSource::EmbeddedDirectory {
            path,
            manifest_sha256,
            required_paths,
        } => SourceEvidence {
            kind: "embedded_directory".into(),
            repository: None,
            revision: None,
            root: String::new(),
            subtree: path.clone(),
            manifest_sha256: manifest_sha256.clone(),
            paths: required_paths.clone(),
        },
    }
}

fn create_absolute_directory(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path).with_context(|| format!("create directory {}", path.display()))?;
    path.canonicalize()
        .with_context(|| format!("canonicalize directory {}", path.display()))
}

fn declared_cohort_identity(
    system: &TaskSystemManifest,
    model: &str,
    provider: &str,
) -> Result<String> {
    artifact::sha256_value(&json!({
        "lane": system.lane,
        "comparison_series": system.comparison_series,
        "stack_mode": system.stack_mode,
        "release_channel": system.release_channel,
        "runner_revision": system.runner_revision,
        "platform": system.platform,
        "components": system.components,
        "model": model,
        "provider": provider,
    }))
}

fn common_cohort_identity(results: &[TaskRunResult]) -> String {
    let identities = results
        .iter()
        .filter(|result| result.infrastructure_valid)
        .map(|result| result.cohort_identity_sha256.as_str())
        .collect::<BTreeSet<_>>();
    if identities.len() == 1 {
        identities.into_iter().next().unwrap_or_default().into()
    } else {
        String::new()
    }
}

fn aggregate_task_results(
    suite: &TaskSuiteDefinition,
    results: &[TaskRunResult],
) -> Vec<TaskAggregate> {
    suite
        .tasks
        .iter()
        .map(|task_id| {
            let all = results
                .iter()
                .filter(|result| &result.task_id == task_id)
                .collect::<Vec<_>>();
            aggregate_task(task_id, suite.repetitions, &all)
        })
        .collect()
}

/// Recompute a stored cohort's aggregates from the per-run evidence it already
/// references. Retained runs keep the full metric totals, so a suite persisted
/// before a metric existed stays comparable without re-executing the model.
pub fn reaggregate_suite_result(path: &Path) -> Result<TaskSuiteResult> {
    let mut summary: TaskSuiteResult = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read suite result {}", path.display()))?,
    )
    .with_context(|| format!("decode suite result {}", path.display()))?;
    let root = path
        .parent()
        .context("suite result has no parent directory")?;
    let mut results = Vec::with_capacity(summary.task_results.len());
    for relative in &summary.task_results {
        let full = root.join(relative);
        let result: TaskRunResult = serde_json::from_slice(
            &fs::read(&full).with_context(|| format!("read task result {}", full.display()))?,
        )
        .with_context(|| format!("decode task result {}", full.display()))?;
        results.push(result);
    }
    summary.task_aggregates = summary
        .task_aggregates
        .iter()
        .map(|aggregate| {
            let all = results
                .iter()
                .filter(|result| result.task_id == aggregate.task_id)
                .collect::<Vec<_>>();
            aggregate_task(&aggregate.task_id, aggregate.requested_runs, &all)
        })
        .collect();
    Ok(summary)
}

fn aggregate_task(task_id: &str, requested_runs: u32, all: &[&TaskRunResult]) -> TaskAggregate {
    let included = all
        .iter()
        .copied()
        .filter(|result| result.infrastructure_valid && result.coverage_complete)
        .collect::<Vec<_>>();
    let n = included.len();
    let mut unavailable = BTreeMap::new();
    let tokens = collect_task_metric(&included, total_tokens);
    let turns = collect_task_metric(&included, |result| {
        result.metrics.as_ref().map(|metrics| metrics.totals.turns)
    });
    let wall_times = included
        .iter()
        .map(|result| result.wall_time_ms as f64)
        .collect::<Vec<_>>();
    let billable = collect_task_metric(&included, billable_tokens);
    let calls = collect_task_metric(&included, function_calls);
    let cost = collect_task_cost(&included);
    let p95_total_tokens = tail_metric(&tokens, "p95_total_tokens", &mut unavailable);
    let p95_turns = tail_metric(&turns, "p95_turns", &mut unavailable);
    let p95_billable_tokens = tail_metric(&billable, "p95_billable_tokens", &mut unavailable);
    let p95_function_calls = tail_metric(&calls, "p95_function_calls", &mut unavailable);
    let p95_cost_usd = tail_metric(&cost, "p95_cost_usd", &mut unavailable);
    let p95_wall_time_ms = tail_metric(
        &Some(wall_times.clone()),
        "p95_wall_time_ms",
        &mut unavailable,
    );
    if tokens.is_none() {
        unavailable.insert(
            "total_tokens".into(),
            "one or more included runs lacks token metrics".into(),
        );
    }
    if turns.is_none() {
        unavailable.insert(
            "turns".into(),
            "one or more included runs lacks turn metrics".into(),
        );
    }
    if billable.is_none() {
        unavailable.insert(
            "billable_tokens".into(),
            "one or more included runs lacks token metrics".into(),
        );
    }
    if calls.is_none() {
        unavailable.insert(
            "function_calls".into(),
            "one or more included runs lacks function-call metrics".into(),
        );
    }
    if cost.is_none() {
        unavailable.insert(
            "cost_usd".into(),
            "provider did not report monetary cost".into(),
        );
    }
    let flaky_rate = if n >= 2 {
        let passed = included
            .iter()
            .filter(|result| result.product_passed)
            .count();
        Some(passed.min(n.saturating_sub(passed)) as f64 / n as f64)
    } else {
        unavailable.insert(
            "flaky_rate".into(),
            "requires at least two included runs".into(),
        );
        None
    };
    TaskAggregate {
        task_id: task_id.into(),
        requested_runs,
        included_runs: n.try_into().unwrap_or(u32::MAX),
        excluded_infrastructure_runs: all.len().saturating_sub(n).try_into().unwrap_or(u32::MAX),
        maturity: evidence_maturity(n).into(),
        product_success: rate_for(&included, |result| result.product_passed),
        structural_integrity: rate_for(&included, |result| result.structural_integrity),
        grounding_integrity: rate_for(&included, |result| result.grounding_integrity),
        technical_failure: rate_for(&included, |result| result.technical_failure),
        flaky_rate,
        p50_total_tokens: tokens.as_deref().and_then(median),
        p95_total_tokens,
        p50_turns: turns.as_deref().and_then(median),
        p95_turns,
        p50_wall_time_ms: median(&wall_times),
        p95_wall_time_ms,
        p50_billable_tokens: billable.as_deref().and_then(median),
        p95_billable_tokens,
        p50_function_calls: calls.as_deref().and_then(median),
        p95_function_calls,
        p50_cost_usd: cost.as_deref().and_then(median),
        p95_cost_usd,
        unavailable,
    }
}

fn rate_for(
    runs: &[&TaskRunResult],
    predicate: impl Fn(&TaskRunResult) -> bool,
) -> Option<RateEstimate> {
    (!runs.is_empty()).then(|| {
        rate_estimate(
            runs.iter().filter(|result| predicate(result)).count(),
            runs.len(),
        )
    })
}

fn rate_estimate(successes: usize, sample_size: usize) -> RateEstimate {
    let n = sample_size as f64;
    let rate = successes as f64 / n;
    let z = 1.959_963_984_540_054_f64;
    let denominator = 1.0 + z * z / n;
    let center = (rate + z * z / (2.0 * n)) / denominator;
    let margin = z * ((rate * (1.0 - rate) / n + z * z / (4.0 * n * n)).sqrt()) / denominator;
    RateEstimate {
        successes: successes.try_into().unwrap_or(u32::MAX),
        sample_size: sample_size.try_into().unwrap_or(u32::MAX),
        rate,
        ci95_lower: (center - margin).max(0.0),
        ci95_upper: (center + margin).min(1.0),
    }
}

fn collect_task_metric(
    runs: &[&TaskRunResult],
    metric: impl Fn(&TaskRunResult) -> Option<u64>,
) -> Option<Vec<f64>> {
    runs.iter()
        .map(|result| metric(result).map(|value| value as f64))
        .collect()
}

fn median(values: &[f64]) -> Option<f64> {
    percentile(values, 0.5)
}

fn tail_metric(
    values: &Option<Vec<f64>>,
    name: &str,
    unavailable: &mut BTreeMap<String, String>,
) -> Option<f64> {
    let Some(values) = values else {
        return None;
    };
    if values.len() < 20 {
        unavailable.insert(name.into(), "requires at least 20 included runs".into());
        return None;
    }
    percentile(values, 0.95)
}

fn percentile(values: &[f64], percentile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted.get(index).copied()
}

fn evidence_maturity(sample_size: usize) -> &'static str {
    if sample_size >= 20 {
        "validated"
    } else if sample_size >= 5 {
        "repeatable"
    } else {
        "directional"
    }
}

impl TaskComparison {
    pub fn compare(baseline: &TaskRunResult, candidate: &TaskRunResult) -> Self {
        let same_case = baseline.task_id == candidate.task_id
            && baseline.task_version == candidate.task_version
            && baseline.case_fingerprint == candidate.case_fingerprint
            && baseline.cohort_identity_sha256 == candidate.cohort_identity_sha256
            && baseline.verifier_mode == candidate.verifier_mode
            && baseline.verifier_sha256 == candidate.verifier_sha256
            && baseline.lane == candidate.lane
            && baseline.comparison_series == candidate.comparison_series;
        let evidence_valid = baseline.infrastructure_valid
            && candidate.infrastructure_valid
            && baseline.coverage_complete
            && candidate.coverage_complete;
        let comparable = same_case && evidence_valid;
        let reason = if comparable {
            "same task, fixture, verifier, model, provider and execution envelope".into()
        } else if same_case {
            "case fingerprints match, but infrastructure validity or verifier coverage is incomplete; result is descriptive only".into()
        } else {
            "task case fingerprints differ; result is descriptive only".into()
        };
        let baseline_tokens = total_tokens(baseline);
        let candidate_tokens = total_tokens(candidate);
        let baseline_turns = baseline
            .metrics
            .as_ref()
            .map(|metrics| metrics.totals.turns);
        let candidate_turns = candidate
            .metrics
            .as_ref()
            .map(|metrics| metrics.totals.turns);
        let baseline_errors = baseline
            .metrics
            .as_ref()
            .map(|metrics| metrics.totals.function_call_errors);
        let candidate_errors = candidate
            .metrics
            .as_ref()
            .map(|metrics| metrics.totals.function_call_errors);
        Self {
            schema: TASK_COMPARISON_SCHEMA.into(),
            task_id: candidate.task_id.clone(),
            case_fingerprint: candidate.case_fingerprint.clone(),
            baseline_system_identity_sha256: baseline.system_identity_sha256.clone(),
            candidate_system_identity_sha256: candidate.system_identity_sha256.clone(),
            baseline_passed: baseline.product_passed,
            candidate_passed: candidate.product_passed,
            capability_delta: comparable
                .then(|| i8::from(candidate.product_passed) - i8::from(baseline.product_passed)),
            token_delta: comparable
                .then(|| option_delta(baseline_tokens, candidate_tokens))
                .flatten(),
            turn_delta: comparable
                .then(|| option_delta(baseline_turns, candidate_turns))
                .flatten(),
            function_error_delta: comparable
                .then(|| option_delta(baseline_errors, candidate_errors))
                .flatten(),
            wall_time_delta_ms: comparable
                .then(|| signed_delta(baseline.wall_time_ms, candidate.wall_time_ms)),
            comparable,
            reason,
        }
    }
}

impl TaskSuiteComparison {
    pub fn compare(baseline: &TaskSuiteResult, candidate: &TaskSuiteResult) -> Self {
        let same_experiment = baseline.suite_id == candidate.suite_id
            && baseline.suite_version == candidate.suite_version
            && baseline.suite_behavior_sha256 == candidate.suite_behavior_sha256
            && baseline.lane == candidate.lane
            && baseline.comparison_series == candidate.comparison_series
            && baseline.release_channel == candidate.release_channel
            && baseline.verifier_mode == candidate.verifier_mode
            && baseline.verifier_sha256 == candidate.verifier_sha256
            && !baseline.cohort_identity_sha256.is_empty()
            && baseline.cohort_identity_sha256 == candidate.cohort_identity_sha256
            && baseline.model == candidate.model
            && baseline.provider == candidate.provider;
        let baseline_tasks = baseline
            .task_aggregates
            .iter()
            .map(|aggregate| (aggregate.task_id.as_str(), aggregate))
            .collect::<BTreeMap<_, _>>();
        let candidate_tasks = candidate
            .task_aggregates
            .iter()
            .map(|aggregate| (aggregate.task_id.as_str(), aggregate))
            .collect::<BTreeMap<_, _>>();
        let same_tasks = baseline_tasks.keys().eq(candidate_tasks.keys());
        let comparable = same_experiment && same_tasks;
        let deltas = if comparable {
            candidate_tasks
                .iter()
                .map(|(task_id, to)| {
                    let from = baseline_tasks[task_id];
                    TaskAggregateDelta {
                        task_id: (*task_id).into(),
                        baseline_maturity: from.maturity.clone(),
                        candidate_maturity: to.maturity.clone(),
                        product_success_rate: rate_delta(
                            &from.product_success,
                            &to.product_success,
                        ),
                        structural_integrity_rate: rate_delta(
                            &from.structural_integrity,
                            &to.structural_integrity,
                        ),
                        grounding_integrity_rate: rate_delta(
                            &from.grounding_integrity,
                            &to.grounding_integrity,
                        ),
                        technical_failure_rate: rate_delta(
                            &from.technical_failure,
                            &to.technical_failure,
                        ),
                        p50_total_tokens_ratio: ratio_delta(
                            from.p50_total_tokens,
                            to.p50_total_tokens,
                        ),
                        p50_turns_ratio: ratio_delta(from.p50_turns, to.p50_turns),
                        p50_wall_time_ratio: ratio_delta(
                            from.p50_wall_time_ms,
                            to.p50_wall_time_ms,
                        ),
                        p50_billable_tokens_ratio: ratio_delta(
                            from.p50_billable_tokens,
                            to.p50_billable_tokens,
                        ),
                        p50_function_calls_ratio: ratio_delta(
                            from.p50_function_calls,
                            to.p50_function_calls,
                        ),
                        p50_cost_usd_ratio: ratio_delta(from.p50_cost_usd, to.p50_cost_usd),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            schema: TASK_SUITE_COMPARISON_SCHEMA.into(),
            baseline_execution_id: baseline.execution_id.clone(),
            candidate_execution_id: candidate.execution_id.clone(),
            comparable,
            advisory: true,
            reason: if comparable {
                "same lane, task behavior, verifier, model, provider and non-Harness cohort identity".into()
            } else {
                "suite identities differ or one side lacks a complete comparable cohort; result is descriptive only".into()
            },
            deltas,
        }
    }
}

fn rate_delta(from: &Option<RateEstimate>, to: &Option<RateEstimate>) -> Option<f64> {
    Some(to.as_ref()?.rate - from.as_ref()?.rate)
}

fn ratio_delta(from: Option<f64>, to: Option<f64>) -> Option<f64> {
    let from = from?;
    (from != 0.0).then_some(to? / from - 1.0)
}

pub fn read_task_result(path: &Path) -> Result<TaskRunResult> {
    let result: TaskRunResult = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read task result {}", path.display()))?,
    )
    .with_context(|| format!("decode task result {}", path.display()))?;
    if result.schema != TASK_RESULT_SCHEMA && result.schema != LEGACY_TASK_RESULT_SCHEMA {
        bail!("unsupported task result schema '{}'", result.schema);
    }
    Ok(result)
}

pub fn read_task_suite_result(path: &Path) -> Result<TaskSuiteResult> {
    let result: TaskSuiteResult = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read task suite result {}", path.display()))?,
    )
    .with_context(|| format!("decode task suite result {}", path.display()))?;
    if result.schema != TASK_SUITE_RESULT_SCHEMA {
        bail!("unsupported task suite result schema '{}'", result.schema);
    }
    Ok(result)
}

fn total_tokens(result: &TaskRunResult) -> Option<u64> {
    let totals = &result.metrics.as_ref()?.totals;
    Some(totals.input_tokens?.saturating_add(totals.output_tokens?))
}

/// Every token the turn moved, including the reasoning and cache volume that
/// `total_tokens` leaves out. Cache reads dominate real workloads, so the two
/// series are kept side by side rather than one replacing the other.
fn billable_tokens(result: &TaskRunResult) -> Option<u64> {
    let totals = &result.metrics.as_ref()?.totals;
    let mut sum = totals.input_tokens?.saturating_add(totals.output_tokens?);
    for extra in [
        totals.reasoning_tokens,
        totals.cache_read_tokens,
        totals.cache_write_tokens,
    ] {
        sum = sum.saturating_add(extra.unwrap_or(0));
    }
    Some(sum)
}

fn function_calls(result: &TaskRunResult) -> Option<u64> {
    result
        .metrics
        .as_ref()
        .map(|metrics| metrics.totals.function_calls)
}

/// Monetary cost is passthrough only: a provider that reports nothing leaves
/// the series absent instead of being imputed from a price table.
fn collect_task_cost(runs: &[&TaskRunResult]) -> Option<Vec<f64>> {
    runs.iter()
        .map(|result| {
            result
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.totals.cost_usd)
        })
        .collect()
}

fn redact_official_verification(verification: &mut TaskVerification) {
    for (index, check) in verification.checks.iter_mut().enumerate() {
        check.id = format!("official-check-{:03}", index + 1);
        check.detail = format!(
            "official verifier check {}; private detail retained by the trusted runner",
            if check.passed { "passed" } else { "failed" }
        );
    }
    for command in &mut verification.commands {
        command.program = "<official-verifier>".into();
        command.args.clear();
        command.stdout_preview = "<official-verifier-output-redacted>".into();
        command.stderr_preview = "<official-verifier-output-redacted>".into();
    }
}

fn option_delta(baseline: Option<u64>, candidate: Option<u64>) -> Option<i64> {
    Some(signed_delta(baseline?, candidate?))
}

fn signed_delta(baseline: u64, candidate: u64) -> i64 {
    i128::from(candidate)
        .saturating_sub(i128::from(baseline))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn validate_slug(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("{label} '{value}' is not lowercase snake_case");
    }
    Ok(())
}

fn validate_repository(value: &str) -> Result<()> {
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        bail!("task repository '{value}' must use owner/name form");
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        bail!("task {label} must use a sha256: prefix");
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("task {label} must contain 64 hexadecimal digits");
    }
    Ok(())
}

fn validate_relative_path(value: &str, label: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("task {label} '{value}' is not a safe relative path");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn materialized(id: &str) -> (tempfile::TempDir, CompiledTask, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let task = embedded_task(id).unwrap();
        let workspace = temporary.path().join("workspace");
        materialize_workspace(&task, &workspace).unwrap();
        (temporary, task, workspace)
    }

    #[test]
    fn pilot_catalog_has_seven_native_tasks_and_four_profiles() {
        let tasks = embedded_catalog().unwrap();
        assert_eq!(tasks.len(), 7);
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.definition.id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "bugfix_cache_invalidation",
                "bugfix_config_precedence",
                "contract_migration_plan",
                "feature_batch_replay",
                "release_train_recovery",
                "release_train_recovery_simulated",
                "security_code_review",
            ])
        );
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.definition.verifier.profile)
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        for task in tasks {
            assert!(task.behavior_sha256.starts_with("sha256:"));
            observe_source(&task.definition.source).unwrap();
        }
    }

    #[test]
    fn task_output_directories_are_canonicalized_for_function_scopes() {
        let relative = Path::new("target")
            .join("task-path-tests")
            .join(Uuid::new_v4().simple().to_string());
        let absolute = create_absolute_directory(&relative).unwrap();
        assert!(absolute.is_absolute());
        assert!(absolute.is_dir());
        fs::remove_dir(&absolute).unwrap();
    }

    #[tokio::test]
    async fn every_code_task_has_a_red_baseline() {
        for id in [
            "bugfix_cache_invalidation",
            "bugfix_config_precedence",
            "feature_batch_replay",
        ] {
            let (_temporary, task, workspace) = materialized(id);
            validate_baseline(&task, &workspace).await.unwrap();
        }
    }

    #[test]
    fn transient_language_artifacts_do_not_pollute_patch_scope() {
        let (_temporary, _task, workspace) = materialized("bugfix_config_precedence");
        fs::create_dir_all(workspace.join("src/__pycache__")).unwrap();
        fs::create_dir_all(workspace.join(".pytest_cache")).unwrap();
        fs::write(
            workspace.join("src/__pycache__/config.cpython-314.pyc"),
            b"cache",
        )
        .unwrap();
        fs::write(workspace.join(".pytest_cache/state"), b"cache").unwrap();
        assert!(changed_paths(&workspace).unwrap().is_empty());
    }

    #[tokio::test]
    async fn multi_file_bugfix_verifier_accepts_only_the_complete_fix() {
        let (_temporary, task, workspace) = materialized("bugfix_config_precedence");
        fs::write(
            workspace.join("src/config_loader.py"),
            r#"# Configuration precedence rules.

def merge_config(file_values, env_values, cli_values):
    merged = dict(file_values)
    merged.update(env_values)
    merged.update(cli_values)
    return merged
"#,
        )
        .unwrap();
        fs::write(
            workspace.join("src/config_cache.py"),
            r#"# Small configuration cache used by the benchmark fixture.
from src.config_loader import merge_config

class ConfigCache:
    def __init__(self):
        self._values = {}
    def resolve(self, name, file_values, env_values, cli_values):
        key = (name, tuple(sorted(env_values.items())), tuple(sorted(cli_values.items())))
        if key not in self._values:
            self._values[key] = merge_config(file_values, env_values, cli_values)
        return dict(self._values[key])
"#,
        )
        .unwrap();
        let verification = verify_workspace(&task, &workspace).await.unwrap();
        assert!(
            verification.passed,
            "checks={:#?}\ncommands={:#?}",
            verification.checks, verification.commands
        );
        assert_eq!(verification.changed_paths.len(), 2);
    }

    #[tokio::test]
    async fn cache_bugfix_and_feature_have_known_green_candidates() {
        let (_temporary, task, workspace) = materialized("bugfix_cache_invalidation");
        fs::write(
            workspace.join("src/cache_store.py"),
            r#"class CacheStore:
    def __init__(self):
        self._entries = {}
    def put(self, key, value, version):
        self._entries[key] = {"value": value, "version": version, "stale": False}
    def invalidate(self, key, version):
        entry = self._entries.get(key)
        if entry is None or version < entry["version"]:
            return False
        entry["stale"] = True
        return True
    def get(self, key):
        entry = self._entries.get(key)
        return None if entry is None or entry["stale"] else entry["value"]
"#,
        )
        .unwrap();
        fs::write(
            workspace.join("src/profile_service.py"),
            r#"class ProfileService:
    def __init__(self, store, loader):
        self.store = store
        self.loader = loader
    def profile(self, user_id, version):
        cached = self.store.get(user_id)
        if cached is not None:
            return cached
        value = self.loader(user_id, version)
        self.store.put(user_id, value, version)
        return value
    def on_profile_changed(self, user_id, event_version):
        return self.store.invalidate(user_id, event_version)
"#,
        )
        .unwrap();
        assert!(verify_workspace(&task, &workspace).await.unwrap().passed);

        let (_temporary, task, workspace) = materialized("feature_batch_replay");
        fs::write(
            workspace.join("src/replay.py"),
            r#"def replay_all(store, handler, batch_size=100, start_cursor=None):
    if batch_size <= 0:
        raise ValueError("batch_size must be positive")
    cursor = start_cursor
    handled = 0
    while True:
        page, next_cursor = store.after(cursor, batch_size)
        for event in page:
            handler(event)
            handled += 1
        if next_cursor is None:
            return handled
        cursor = next_cursor
"#,
        )
        .unwrap();
        assert!(verify_workspace(&task, &workspace).await.unwrap().passed);
    }

    #[tokio::test]
    async fn structured_review_is_read_only_and_evidence_bound() {
        let (_temporary, task, workspace) = materialized("security_code_review");
        fs::write(
            workspace.join("review.json"),
            serde_json::to_vec_pretty(&json!({
                "summary": "Three concrete repository-backed security findings require remediation.",
                "findings": [
                    {
                        "id": "shell-command-injection",
                        "severity": "high",
                        "path": "src/vulnerable.rs",
                        "line": 8,
                        "explanation": "Untrusted user input is interpolated into a shell command and executed through sh.",
                        "remediation": "Replace shell interpolation with a direct process invocation and validated fixed arguments."
                    },
                    {
                        "id": "unpinned-workflow-action",
                        "severity": "medium",
                        "path": ".github/workflows/insecure.yml",
                        "line": 14,
                        "explanation": "The workflow resolves an action through a mutable branch instead of immutable content.",
                        "remediation": "Pin the action to a reviewed full commit SHA and update it through controlled review."
                    },
                    {
                        "id": "unverified-remote-script",
                        "severity": "high",
                        "path": ".github/workflows/insecure.yml",
                        "line": 16,
                        "explanation": "The workflow downloads an unverified remote script and executes it directly through a shell.",
                        "remediation": "Replace the remote script pipeline with a reviewed artifact pinned and verified by immutable digest."
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let verification = verify_workspace(&task, &workspace).await.unwrap();
        assert!(verification.passed, "{:#?}", verification.checks);
        assert_eq!(verification.changed_paths, vec!["review.json"]);
    }

    #[tokio::test]
    async fn missing_subject_artifact_is_a_product_failure() {
        let (_temporary, task, workspace) = materialized("security_code_review");
        let verification = verify_workspace(&task, &workspace).await.unwrap();
        assert!(!verification.passed);
        assert!(coverage_is_complete(&task, &verification));
        assert!(verification
            .checks
            .iter()
            .find(|check| check.id == "artifact_read")
            .is_some_and(|check| !check.passed));
        assert!(verification
            .checks
            .iter()
            .find(|check| check.id == "artifact_schema")
            .is_some_and(|check| !check.passed));
    }

    #[tokio::test]
    async fn migration_plan_has_a_known_schema_valid_candidate() {
        let (_temporary, task, workspace) = materialized("contract_migration_plan");
        fs::write(
            workspace.join("migration-plan.json"),
            serde_json::to_vec_pretty(&json!({
                "objective": "Migrate producer and consumer while preserving the v1 contract during adoption.",
                "evidence": [
                    {"path": "consumer/contract.json", "reason": "The current consumer requires the complete v1 response shape."},
                    {"path": "producer/contract.json", "reason": "The producer declares the target v2 response contract."}
                ],
                "steps": [
                    {"id": "add-v1-v2-compatibility", "action": "Publish v2 fields alongside the complete v1 response shape.", "completion_signal": "Both response schemas pass."},
                    {"id": "validate-dual-contract", "action": "Run producer and consumer checks against both response versions.", "completion_signal": "Dual-contract matrix is green."},
                    {"id": "migrate-consumer", "action": "Switch the consumer to v2 fields behind a reversible rollout.", "completion_signal": "Consumer v2 adoption is observed."},
                    {"id": "retire-v1-compatibility", "action": "Remove v1 compatibility only after adoption evidence is complete.", "completion_signal": "No remaining v1 consumers exist."}
                ],
                "validation": ["Run producer schema tests for v1 and v2.", "Run consumer compatibility and rollback canaries."],
                "rollback": "Restore the consumer selector before removing compatibility and retain the dual response until recovery is proven."
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(verify_workspace(&task, &workspace).await.unwrap().passed);
    }

    #[tokio::test]
    async fn operational_recovery_is_bound_to_the_initial_identity() {
        let (_temporary, task, workspace) = materialized("release_train_recovery");
        fs::write(
            workspace.join("recovered-state.json"),
            serde_json::to_vec_pretty(&json!({
                "version": "0.21.8",
                "tag": "workers/v0.21.8",
                "run_id": 424242,
                "run_attempt": 2,
                "latest_before": "0.20.4",
                "latest_after": "0.21.8",
                "operation": "promotion-fresh-002",
                "published": true,
                "promoted": true,
                "locks_released": true
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            workspace.join("recovery-report.json"),
            serde_json::to_vec_pretty(&json!({
                "reused_immutable_identity": true,
                "fresh_gated_operation": true,
                "cas_expected_latest": "0.20.4",
                "direct_channel_mutation": false,
                "retagged": false,
                "version_bumped": false,
                "stale_operation_retried": false
            }))
            .unwrap(),
        )
        .unwrap();
        let verification = verify_workspace(&task, &workspace).await.unwrap();
        assert!(verification.passed, "{:#?}", verification.checks);
    }

    #[tokio::test]
    async fn simulated_recovery_executes_only_the_fresh_gated_path() {
        let (_temporary, task, workspace) = materialized("release_train_recovery_simulated");
        fs::write(
            workspace.join("recovery-actions.json"),
            serde_json::to_vec_pretty(&json!({
                "actions": [
                    {"action": "retry_run", "run_id": 424242, "run_attempt": 2},
                    {"action": "create_gated_operation", "operation": "promotion-fresh-002", "version": "0.21.8", "expected_latest": "0.20.4"},
                    {"action": "execute_promotion", "operation": "promotion-fresh-002"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let verification = verify_workspace(&task, &workspace).await.unwrap();
        assert!(verification.passed, "{:#?}", verification.checks);
        assert!(coverage_is_complete(&task, &verification));

        fs::write(
            workspace.join("recovery-actions.json"),
            serde_json::to_vec_pretty(&json!({
                "actions": [{"action": "set_channel_directly"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let rejected = verify_workspace(&task, &workspace).await.unwrap();
        assert!(!rejected.passed);
        assert!(
            !rejected
                .checks
                .iter()
                .find(|check| check.id == "no_forbidden_operation")
                .unwrap()
                .passed
        );
    }

    #[test]
    fn comparison_requires_the_same_case_fingerprint() {
        let result = |case: &str, system: &str, passed: bool| {
            let mut result = sample_run_result();
            result.execution_id = system.into();
            result.case_fingerprint = case.into();
            result.system_identity_sha256 = system.into();
            result.harness_version = system.into();
            result.status = if passed {
                TaskRunStatus::Passed
            } else {
                TaskRunStatus::Failed
            };
            result.product_passed = passed;
            result
        };
        let baseline = result("case-a", "system-a", false);
        let candidate = result("case-a", "system-b", true);
        let comparison = TaskComparison::compare(&baseline, &candidate);
        assert!(comparison.comparable);
        assert_eq!(comparison.capability_delta, Some(1));
        let changed_case = TaskComparison::compare(&baseline, &result("case-b", "system-c", true));
        assert!(!changed_case.comparable);
        assert_eq!(changed_case.capability_delta, None);
        assert_eq!(changed_case.wall_time_delta_ms, None);
        let mut invalid_evidence = result("case-a", "system-c", true);
        invalid_evidence.infrastructure_valid = false;
        let invalid = TaskComparison::compare(&baseline, &invalid_evidence);
        assert!(!invalid.comparable);
        assert_eq!(invalid.capability_delta, None);
    }

    fn sample_run_result() -> TaskRunResult {
        TaskRunResult {
            schema: TASK_RESULT_SCHEMA.into(),
            execution_id: "system".into(),
            task_id: "feature_batch_replay".into(),
            task_version: 1,
            task_kind: TaskKind::Feature,
            behavior_sha256: "sha256:behavior".into(),
            case_fingerprint: "case".into(),
            system_identity_sha256: "system".into(),
            cohort_identity_sha256: "cohort-a".into(),
            lane: "local_development".into(),
            comparison_series: "local".into(),
            release_channel: None,
            release_tag: None,
            verifier_mode: TaskVerifierMode::Development,
            verifier_sha256: "sha256:verifier".into(),
            engine_version: "engine".into(),
            harness_version: "system".into(),
            model: "model".into(),
            provider: "provider".into(),
            status: TaskRunStatus::Passed,
            product_passed: true,
            structural_integrity: true,
            grounding_integrity: true,
            technical_failure: false,
            infrastructure_valid: true,
            budget_passed: true,
            coverage_complete: true,
            wall_time_ms: 10,
            workspace: "/workspace".into(),
            source: SourceEvidence {
                kind: "embedded_directory".into(),
                repository: None,
                revision: None,
                root: "/source".into(),
                subtree: String::new(),
                manifest_sha256: "sha256:source".into(),
                paths: vec!["file".into()],
            },
            metrics: None,
            transcript_sha256: None,
            verifier: None,
            cleanup_valid: true,
            subject_failure: None,
            failure: None,
        }
    }

    fn metrics_totals(cost_usd: Option<f64>) -> SessionMetricsResponse {
        let mut totals = serde_json::json!({
            "sessions": 1,
            "turns": 9,
            "function_calls": 14,
            "function_call_errors": 1,
            "input_tokens": 18_595,
            "output_tokens": 3_197,
            "reasoning_tokens": 686,
            "cache_read_tokens": 64_512,
            "cache_write_tokens": 0,
        });
        if let Some(cost) = cost_usd {
            totals["cost_usd"] = serde_json::json!(cost);
        }
        serde_json::from_value(serde_json::json!({
            "complete": true,
            "root_session_id": "session",
            "totals": totals,
            "by_session": [],
            "traces": [],
        }))
        .expect("metrics fixture decodes")
    }

    #[test]
    fn aggregates_expose_billable_tokens_calls_and_passthrough_cost() {
        let run = |cost: Option<f64>| {
            let mut result = sample_run_result();
            result.metrics = Some(metrics_totals(cost));
            result
        };

        let priced = [run(Some(0.25)), run(Some(0.35))];
        let borrowed = priced.iter().collect::<Vec<_>>();
        let aggregate = aggregate_task("feature_batch_replay", 2, &borrowed);

        // input + output alone is 21_792; the cache and reasoning volume the
        // provider also moved brings the billable series to 86_990.
        assert_eq!(aggregate.p50_total_tokens, Some(21_792.0));
        assert_eq!(aggregate.p50_billable_tokens, Some(86_990.0));
        assert_eq!(aggregate.p50_function_calls, Some(14.0));
        assert_eq!(aggregate.p50_cost_usd, Some(0.35));
        assert!(!aggregate.unavailable.contains_key("cost_usd"));

        let unpriced = [run(None), run(Some(0.25))];
        let borrowed = unpriced.iter().collect::<Vec<_>>();
        let aggregate = aggregate_task("feature_batch_replay", 2, &borrowed);

        // One silent run voids the series rather than averaging over a hole,
        // and no price table stands in for what the provider never reported.
        assert_eq!(aggregate.p50_cost_usd, None);
        assert_eq!(
            aggregate.unavailable.get("cost_usd").map(String::as_str),
            Some("provider did not report monetary cost")
        );
        assert_eq!(aggregate.p50_billable_tokens, Some(86_990.0));
    }

    #[test]
    fn checked_in_pilot_suite_selects_the_complete_catalog() {
        let suite = TaskSuiteDefinition::read(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("config/task-suites/pilot.json"),
        )
        .unwrap();
        assert_eq!(suite.tasks.len(), embedded_catalog().unwrap().len());
        assert_eq!(suite.repetitions, 1);
        let suites = checked_in_task_suites().unwrap();
        assert_eq!(suites.len(), 4);
        assert!(suites.iter().all(|(_, suite)| suite.validate().is_ok()));
    }

    #[test]
    fn official_bundle_is_digested_and_bound_to_each_task() {
        let task = embedded_task("feature_batch_replay").unwrap();
        let bundle = OfficialVerifierBundle {
            schema: OFFICIAL_VERIFIER_BUNDLE_SCHEMA.into(),
            id: "official-test".into(),
            tasks: BTreeMap::from([(task.definition.id.clone(), task.verifier.clone())]),
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("official.json");
        fs::write(&path, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();
        let (loaded, digest) = OfficialVerifierBundle::read(&path).unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(loaded.verifier_for(&task).unwrap(), task.verifier);
        assert!(loaded
            .verifier_for(&embedded_task("security_code_review").unwrap())
            .is_err());

        let mut verification = TaskVerification {
            passed: false,
            changed_paths: vec!["subject.json".into()],
            patch_lines: 1,
            checks: vec![verification_check(
                "secret_check_name",
                VerificationDimension::Grounding,
                false,
                "secret expected value",
            )],
            commands: vec![CommandResult {
                program: "secret-program".into(),
                args: vec!["secret-argument".into()],
                success: false,
                exit_code: Some(1),
                timed_out: false,
                stdout_sha256: "sha256:stdout".into(),
                stderr_sha256: "sha256:stderr".into(),
                stdout_preview: "secret stdout".into(),
                stderr_preview: "secret stderr".into(),
            }],
        };
        redact_official_verification(&mut verification);
        let public = serde_json::to_string(&verification).unwrap();
        assert!(!public.contains("secret"));
        assert_eq!(verification.checks[0].id, "official-check-001");
        assert_eq!(
            verification.checks[0].dimension,
            VerificationDimension::Grounding
        );
        assert!(!verification.checks[0].passed);
        assert_eq!(verification.commands[0].stdout_sha256, "sha256:stdout");
    }

    #[test]
    fn longitudinal_statistics_preserve_evidence_maturity() {
        assert_eq!(evidence_maturity(1), "directional");
        assert_eq!(evidence_maturity(5), "repeatable");
        assert_eq!(evidence_maturity(20), "validated");
        let estimate = rate_estimate(4, 5);
        assert_eq!(estimate.sample_size, 5);
        assert!((estimate.rate - 0.8).abs() < f64::EPSILON);
        assert!(estimate.ci95_lower < estimate.rate);
        assert!(estimate.ci95_upper > estimate.rate);
        let mut unavailable = BTreeMap::new();
        assert!(tail_metric(&Some(vec![1.0; 19]), "p95", &mut unavailable).is_none());
        assert_eq!(
            tail_metric(&Some(vec![1.0; 20]), "p95", &mut unavailable),
            Some(1.0)
        );
    }

    #[test]
    fn suite_comparison_requires_one_immutable_experiment_series() {
        let suite_result = |execution: &str, series: &str, successes: usize| TaskSuiteResult {
            schema: TASK_SUITE_RESULT_SCHEMA.into(),
            suite_id: "remote_rc".into(),
            suite_version: 1,
            suite_behavior_sha256: "sha256:suite".into(),
            execution_id: execution.into(),
            lane: "remote_release".into(),
            comparison_series: series.into(),
            release_channel: Some("rc".into()),
            release_tag: Some(format!("harness/v1.2.3-rc.{execution}")),
            verifier_mode: TaskVerifierMode::Official,
            verifier_sha256: "sha256:official".into(),
            cohort_identity_sha256: "sha256:cohort".into(),
            model: "model".into(),
            provider: "provider".into(),
            requested_runs: 5,
            completed_runs: 5,
            product_passed_runs: successes as u32,
            infrastructure_invalid_runs: 0,
            resource_limited_runs: 0,
            coverage_incomplete_runs: 0,
            task_results: Vec::new(),
            task_aggregates: vec![TaskAggregate {
                task_id: "feature_batch_replay".into(),
                requested_runs: 5,
                included_runs: 5,
                excluded_infrastructure_runs: 0,
                maturity: "repeatable".into(),
                product_success: Some(rate_estimate(successes, 5)),
                structural_integrity: Some(rate_estimate(5, 5)),
                grounding_integrity: Some(rate_estimate(5, 5)),
                technical_failure: Some(rate_estimate(0, 5)),
                flaky_rate: Some(0.0),
                p50_total_tokens: Some(100.0),
                p95_total_tokens: None,
                p50_turns: Some(4.0),
                p95_turns: None,
                p50_wall_time_ms: Some(1000.0),
                p95_wall_time_ms: None,
                p50_billable_tokens: Some(500.0),
                p95_billable_tokens: None,
                p50_function_calls: Some(6.0),
                p95_function_calls: None,
                p50_cost_usd: None,
                p95_cost_usd: None,
                unavailable: BTreeMap::new(),
            }],
        };
        let baseline = suite_result("1", "rc:harness/v1.2.3", 3);
        let candidate = suite_result("2", "rc:harness/v1.2.3", 4);
        let comparison = TaskSuiteComparison::compare(&baseline, &candidate);
        assert!(comparison.comparable);
        assert!(comparison.deltas[0]
            .product_success_rate
            .is_some_and(|delta| (delta - 0.2).abs() < 1e-12));

        let other_line = suite_result("3", "rc:harness/v1.2.4", 5);
        let incompatible = TaskSuiteComparison::compare(&baseline, &other_line);
        assert!(!incompatible.comparable);
        assert!(incompatible.deltas.is_empty());
    }

    #[test]
    fn local_and_remote_system_identities_cannot_be_mixed() {
        assert!(TaskSystemManifest::default().validate().is_ok());
        let remote_without_tag = TaskSystemManifest {
            lane: "remote_release".into(),
            comparison_series: "rc:harness/v1.2.3".into(),
            stack_mode: "registry".into(),
            release_channel: Some("rc".into()),
            release_tag: None,
            runner_revision: "runner-sha".into(),
            platform: "linux-amd64".into(),
            components: BTreeMap::new(),
        };
        assert!(remote_without_tag.validate().is_err());
        let mut wrong_rc_line = remote_without_tag;
        wrong_rc_line.release_tag = Some("harness/v1.2.4-rc.1".into());
        assert!(wrong_rc_line.validate().is_err());
    }
}
