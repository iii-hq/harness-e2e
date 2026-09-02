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

pub const TASK_SCHEMA: &str = "harness-e2e-task/v1";
pub const TASK_RESULT_SCHEMA: &str = "harness-e2e-task-result/v1";
pub const TASK_COMPARISON_SCHEMA: &str = "harness-e2e-task-comparison/v1";
pub const TASK_SUITE_SCHEMA: &str = "harness-e2e-task-suite/v1";
pub const TASK_SUITE_RESULT_SCHEMA: &str = "harness-e2e-task-suite-result/v1";

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
}

impl VerifierSpec {
    fn profile(&self) -> VerifierProfile {
        match self {
            Self::CodePatch { .. } => VerifierProfile::CodePatch,
            Self::StructuredArtifact { .. } => VerifierProfile::StructuredArtifact,
            Self::StateRecovery { .. } => VerifierProfile::StateRecovery,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskCommand {
    pub program: String,
    pub args: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JsonAssertion {
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
    pub engine_version: String,
    pub harness_version: String,
    pub model: String,
    pub provider: String,
    pub status: TaskRunStatus,
    pub product_passed: bool,
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
    pub failure: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskRunConfig {
    pub url: String,
    pub task_id: String,
    pub model: String,
    pub provider: String,
    pub output: PathBuf,
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
    pub repetitions: u32,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskSuiteResult {
    pub schema: String,
    pub suite_id: String,
    pub suite_version: u32,
    pub suite_behavior_sha256: String,
    pub model: String,
    pub provider: String,
    pub requested_runs: u32,
    pub completed_runs: u32,
    pub product_passed_runs: u32,
    pub infrastructure_invalid_runs: u32,
    pub resource_limited_runs: u32,
    pub coverage_incomplete_runs: u32,
    pub task_results: Vec<String>,
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
        match (self.kind, self.verifier.profile) {
            (TaskKind::CodePatch | TaskKind::Feature, VerifierProfile::CodePatch)
            | (TaskKind::CodeReview | TaskKind::Planning, VerifierProfile::StructuredArtifact)
            | (TaskKind::OperationalRecovery, VerifierProfile::StateRecovery) => {}
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
            if public_commands.is_empty() || hidden_commands.is_empty() {
                bail!(
                    "task '{}' code verifier needs public and hidden commands",
                    task.id
                );
            }
            for command in public_commands.iter().chain(hidden_commands) {
                validate_command(command)?;
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
    }
    Ok(())
}

fn validate_command(command: &TaskCommand) -> Result<()> {
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
    for assertion in assertions {
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
                commands.push(run_task_command(workspace, command).await);
            }
            let public_count = public_commands.len();
            checks.push(VerificationCheck {
                id: "public_commands".into(),
                passed: commands[..public_count].iter().all(|result| result.success),
                detail: format!("executed {public_count} public verifier command(s)"),
            });
            checks.push(VerificationCheck {
                id: "hidden_commands".into(),
                passed: commands[public_count..].iter().all(|result| result.success),
                detail: format!(
                    "executed {} runner-private verifier command(s)",
                    hidden_commands.len()
                ),
            });
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
            }
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
            passed: allowed,
            detail: format!("changed paths: {changed_paths:?}"),
        },
        VerificationCheck {
            id: "protected_paths_exact".into(),
            passed: protected,
            detail: "protected fixture paths must remain byte-identical".into(),
        },
        VerificationCheck {
            id: "changed_file_budget".into(),
            passed: changed_count >= policy.minimum_changed_files
                && changed_count <= policy.maximum_changed_files,
            detail: format!(
                "changed {changed_count} file(s), expected {}..={}",
                policy.minimum_changed_files, policy.maximum_changed_files
            ),
        },
        VerificationCheck {
            id: "patch_line_budget".into(),
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
                passed: true,
                detail: format!("read subject artifact {relative}"),
            },
            Some(value),
        ),
        Err(error) => (
            VerificationCheck {
                id: check_id.into(),
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
            passed: true,
            detail: "artifact matches the deterministic JSON schema".into(),
        },
        Err(error) => VerificationCheck {
            id: id.into(),
            passed: false,
            detail: error,
        },
    }
}

fn assertion_checks(
    workspace: &Path,
    prefix: &str,
    value: &Value,
    assertions: &[JsonAssertion],
) -> Vec<VerificationCheck> {
    assertions
        .iter()
        .enumerate()
        .map(|(index, assertion)| {
            let expected = assertion.equals.clone().or_else(|| {
                let source = assertion.equals_from.as_ref()?;
                read_json(workspace, &source.file)
                    .ok()?
                    .pointer(&source.pointer)
                    .cloned()
            });
            let observed = value.pointer(&assertion.pointer);
            VerificationCheck {
                id: format!("{prefix}_{index}"),
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

pub async fn run_task(config: TaskRunConfig) -> Result<(TaskRunResult, PathBuf)> {
    let task = embedded_task(&config.task_id)?;
    let execution_id = Uuid::new_v4().simple().to_string();
    let run_root = config.output.join(&execution_id).join(&task.definition.id);
    let workspace = run_root.join("workspace");
    fs::create_dir_all(&run_root)
        .with_context(|| format!("create task run root {}", run_root.display()))?;
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
    }))?;
    let system_identity_sha256 = artifact::sha256_value(&json!({
        "engine": versions.engine,
        "harness": versions.harness,
    }))?;
    let session_id = format!("benchmark_task_{execution_id}");
    let instruction = task
        .instruction
        .replace("{{workspace}}", &workspace.display().to_string());

    let mut metrics = None;
    let mut transcript_sha256 = None;
    let mut verification = None;
    let mut failure = None;
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
        metrics = Some(
            context
                .wait_for_turn(
                    &task.definition.id,
                    &session_id,
                    &response.turn_id,
                    Duration::from_secs(task.definition.execution.stuck_timeout_seconds),
                    true,
                    None,
                )
                .await?,
        );
        let status: Option<StatusReport> = context
            .trigger("harness::status", json!({"session_id": session_id}))
            .await?;
        let status = status.context("harness::status returned no task session")?;
        if status.result_error.is_some() {
            bail!(
                "Harness task session failed: {}",
                status.result_error.as_deref().unwrap_or("unknown failure")
            );
        }
        let transcript = context.transcript(&session_id).await?;
        transcript_sha256 = Some(artifact::sha256_value(&transcript)?);
        verification = Some(verify_workspace(&task, &workspace).await?);
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
        .is_some_and(|verification| verification.passed);
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
    let coverage_complete = verification.as_ref().is_some_and(|verification| {
        !verification.checks.is_empty()
            && verification
                .checks
                .iter()
                .all(|check| !check.id.trim().is_empty())
    });
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
        engine_version: versions.engine,
        harness_version: versions.harness,
        model: config.model,
        provider: config.provider,
        status,
        product_passed,
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
) -> Result<(TaskSuiteResult, PathBuf)> {
    suite.validate()?;
    let suite_execution_id = Uuid::new_v4().simple().to_string();
    let suite_root = output.join(&suite.id).join(&suite_execution_id);
    fs::create_dir_all(&suite_root)
        .with_context(|| format!("create task suite root {}", suite_root.display()))?;
    let catalog = embedded_catalog()?
        .into_iter()
        .map(|task| (task.definition.id.clone(), task))
        .collect::<BTreeMap<_, _>>();
    let suite_behavior_sha256 = artifact::sha256_value(&json!({
        "suite": suite,
        "tasks": suite.tasks.iter().map(|id| &catalog[id].behavior_sha256).collect::<Vec<_>>(),
    }))?;
    let requested_runs = u32::try_from(suite.tasks.len())
        .unwrap_or(u32::MAX)
        .saturating_mul(suite.repetitions);
    let mut results = Vec::new();
    let mut paths = Vec::new();
    for repetition in 0..suite.repetitions {
        for task_id in &suite.tasks {
            let (result, path) = run_task(TaskRunConfig {
                url: url.to_string(),
                task_id: task_id.clone(),
                model: model.to_string(),
                provider: provider.to_string(),
                output: suite_root.join(format!("repetition-{}", repetition + 1)),
            })
            .await
            .with_context(|| {
                format!(
                    "execute task suite '{}' task '{}' repetition {}",
                    suite.id,
                    task_id,
                    repetition + 1
                )
            })?;
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

impl TaskComparison {
    pub fn compare(baseline: &TaskRunResult, candidate: &TaskRunResult) -> Self {
        let same_case = baseline.task_id == candidate.task_id
            && baseline.task_version == candidate.task_version
            && baseline.case_fingerprint == candidate.case_fingerprint;
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

pub fn read_task_result(path: &Path) -> Result<TaskRunResult> {
    let result: TaskRunResult = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read task result {}", path.display()))?,
    )
    .with_context(|| format!("decode task result {}", path.display()))?;
    if result.schema != TASK_RESULT_SCHEMA {
        bail!("unsupported task result schema '{}'", result.schema);
    }
    Ok(result)
}

fn total_tokens(result: &TaskRunResult) -> Option<u64> {
    let totals = &result.metrics.as_ref()?.totals;
    Some(totals.input_tokens?.saturating_add(totals.output_tokens?))
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
    fn pilot_catalog_has_six_native_tasks_and_three_profiles() {
        let tasks = embedded_catalog().unwrap();
        assert_eq!(tasks.len(), 6);
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
                "security_code_review",
            ])
        );
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.definition.verifier.profile)
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
        for task in tasks {
            assert!(task.behavior_sha256.starts_with("sha256:"));
            observe_source(&task.definition.source).unwrap();
        }
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
                "summary": "Two concrete repository-backed security findings require remediation.",
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
        assert_eq!(verification.checks.last().unwrap().id, "artifact_read");
        assert!(!verification.checks.last().unwrap().passed);
    }

    #[tokio::test]
    async fn migration_plan_has_a_known_schema_valid_candidate() {
        let (_temporary, task, workspace) = materialized("contract_migration_plan");
        fs::write(
            workspace.join("migration-plan.json"),
            serde_json::to_vec_pretty(&json!({
                "objective": "Migrate producer and consumer while preserving the v1 contract during adoption.",
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

    #[test]
    fn comparison_requires_the_same_case_fingerprint() {
        let result = |case: &str, system: &str, passed: bool| TaskRunResult {
            schema: TASK_RESULT_SCHEMA.into(),
            execution_id: system.into(),
            task_id: "feature_batch_replay".into(),
            task_version: 1,
            task_kind: TaskKind::Feature,
            behavior_sha256: "sha256:behavior".into(),
            case_fingerprint: case.into(),
            system_identity_sha256: system.into(),
            engine_version: "engine".into(),
            harness_version: system.into(),
            model: "model".into(),
            provider: "provider".into(),
            status: if passed {
                TaskRunStatus::Passed
            } else {
                TaskRunStatus::Failed
            },
            product_passed: passed,
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
            failure: None,
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

    #[test]
    fn checked_in_pilot_suite_selects_the_complete_catalog() {
        let suite = TaskSuiteDefinition::read(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("config/task-suites/pilot.json"),
        )
        .unwrap();
        assert_eq!(suite.tasks.len(), embedded_catalog().unwrap().len());
        assert_eq!(suite.repetitions, 1);
    }
}
