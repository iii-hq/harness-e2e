//! Declarative repository-task contracts and non-authoritative shadow evaluation.
//!
//! Repository tasks are deliberately separate from Markdown scenarios: they may
//! describe code workspaces and runner-owned verifiers, while Markdown keeps its
//! narrow database/state tool policy. During the first delivery these contracts
//! shadow an existing built-in scenario and cannot affect its verdict.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::artifact::{self, sha256_bytes};
use crate::scenarios::{ExecutionPolicy, ObjectiveEvaluation, ScenarioCase};

pub const REPOSITORY_TASK_SCHEMA: &str = "harness-e2e-repository-task/v1";
pub const SHADOW_REPORT_SCHEMA: &str = "harness-e2e-repository-task-shadow/v1";
pub const VERIFIER_PROTOCOL: &str = "harness-e2e-verifier/v1";

#[derive(RustEmbed)]
#[folder = "repository-tasks/"]
struct EmbeddedRepositoryTasks;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositoryTaskV1 {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub legacy_scenario: String,
    pub mode: RepositoryTaskMode,
    pub source: RepositoryTaskSource,
    pub execution: ExecutionPolicy,
    pub assessments: Vec<RepositoryTaskAssessment>,
    pub verifier: RepositoryTaskVerifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryTaskMode {
    Shadow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepositoryTaskSource {
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
pub struct RepositoryTaskAssessment {
    pub id: String,
    pub weight: u8,
    pub policy: RepositoryTaskAssessmentPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryTaskAssessmentPolicy {
    HardGate,
    Advisory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositoryTaskVerifier {
    pub kind: RepositoryTaskVerifierKind,
    pub protocol: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryTaskVerifierKind {
    ObservationProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompiledRepositoryTask {
    pub source_path: String,
    pub instruction_path: String,
    pub instruction: String,
    pub instruction_sha256: String,
    pub behavior_sha256: String,
    pub definition: RepositoryTaskV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepositoryTaskAssessmentObservation {
    pub id: String,
    pub gate_passed: Option<bool>,
    pub awarded: u8,
    pub reason: String,
}

impl RepositoryTaskAssessmentObservation {
    pub fn hard_gate(
        id: impl Into<String>,
        passed: bool,
        awarded: u8,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            gate_passed: Some(passed),
            awarded,
            reason: reason.into(),
        }
    }

    pub fn advisory(id: impl Into<String>, awarded: u8, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            gate_passed: None,
            awarded,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryTaskShadowAvailability {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepositoryTaskSourceEvidence {
    pub kind: String,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub root: String,
    pub subtree: String,
    pub manifest_sha256: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepositoryTaskProjection {
    pub passed: bool,
    pub score: u8,
    pub gates: BTreeMap<String, bool>,
    pub awards: BTreeMap<String, u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepositoryTaskShadowReport {
    pub schema: String,
    pub availability: RepositoryTaskShadowAvailability,
    pub task_id: String,
    pub task_version: u32,
    pub behavior_sha256: Option<String>,
    pub instruction_sha256: Option<String>,
    pub verifier_protocol: Option<String>,
    pub source: Option<RepositoryTaskSourceEvidence>,
    pub equivalent: bool,
    pub legacy: Option<RepositoryTaskProjection>,
    pub generic: Option<RepositoryTaskProjection>,
    pub mismatches: Vec<String>,
}

impl RepositoryTaskV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != REPOSITORY_TASK_SCHEMA {
            bail!(
                "repository task '{}' uses schema '{}'; expected '{}'",
                self.id,
                self.schema,
                REPOSITORY_TASK_SCHEMA
            );
        }
        validate_slug(&self.id, "repository task id")?;
        validate_slug(&self.legacy_scenario, "legacy scenario id")?;
        if self.id != self.legacy_scenario {
            bail!(
                "repository task '{}' must shadow the same legacy scenario id, observed '{}'",
                self.id,
                self.legacy_scenario
            );
        }
        if self.version == 0 {
            bail!("repository task '{}' version must be at least 1", self.id);
        }
        validate_execution(self.execution, &self.id)?;
        validate_source(&self.source)?;
        if self.verifier.protocol != VERIFIER_PROTOCOL {
            bail!(
                "repository task '{}' verifier protocol '{}' differs from '{}'",
                self.id,
                self.verifier.protocol,
                VERIFIER_PROTOCOL
            );
        }
        if self.assessments.is_empty() {
            bail!("repository task '{}' has no assessments", self.id);
        }
        let mut ids = BTreeSet::new();
        let mut total = 0_u16;
        for assessment in &self.assessments {
            validate_slug(&assessment.id, "assessment id")?;
            if assessment.weight == 0 {
                bail!(
                    "repository task '{}' assessment '{}' has zero weight",
                    self.id,
                    assessment.id
                );
            }
            if !ids.insert(assessment.id.as_str()) {
                bail!(
                    "repository task '{}' repeats assessment '{}'",
                    self.id,
                    assessment.id
                );
            }
            total += u16::from(assessment.weight);
        }
        if total != 100 {
            bail!(
                "repository task '{}' assessment weights total {total}; expected 100",
                self.id
            );
        }
        Ok(())
    }
}

pub fn embedded_catalog() -> Result<Vec<CompiledRepositoryTask>> {
    let mut paths = EmbeddedRepositoryTasks::iter()
        .map(|path| path.into_owned())
        .filter(|path| path.ends_with("/task.toml"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| compile_embedded(&path))
        .collect()
}

pub fn embedded_for_legacy(legacy_scenario: &str) -> Result<Option<CompiledRepositoryTask>> {
    let matches = embedded_catalog()?
        .into_iter()
        .filter(|task| task.definition.legacy_scenario == legacy_scenario)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [task] => Ok(Some(task.clone())),
        _ => bail!("multiple repository tasks shadow legacy scenario '{legacy_scenario}'"),
    }
}

pub fn compile(
    source_path: &str,
    source: &str,
    instruction: &str,
) -> Result<CompiledRepositoryTask> {
    let definition: RepositoryTaskV1 =
        toml::from_str(source).with_context(|| format!("parse repository task {source_path}"))?;
    definition.validate()?;
    validate_instruction(instruction)?;
    let instruction_path = Path::new(source_path)
        .parent()
        .context("repository task manifest has no parent")?
        .join("instruction.md")
        .to_string_lossy()
        .replace('\\', "/");
    let instruction_sha256 = sha256_bytes(instruction.as_bytes());
    let behavior_sha256 = artifact::sha256_value(&json!({
        "definition": definition,
        "instruction": instruction,
    }))?;
    Ok(CompiledRepositoryTask {
        source_path: source_path.to_string(),
        instruction_path,
        instruction: instruction.to_string(),
        instruction_sha256,
        behavior_sha256,
        definition,
    })
}

fn compile_embedded(source_path: &str) -> Result<CompiledRepositoryTask> {
    let source = EmbeddedRepositoryTasks::get(source_path)
        .with_context(|| format!("embedded repository task {source_path} is missing"))?;
    let source = std::str::from_utf8(source.data.as_ref())
        .with_context(|| format!("embedded repository task {source_path} is not UTF-8"))?;
    let instruction_path = Path::new(source_path)
        .parent()
        .context("repository task manifest has no parent")?
        .join("instruction.md")
        .to_string_lossy()
        .replace('\\', "/");
    let instruction = EmbeddedRepositoryTasks::get(&instruction_path).with_context(|| {
        format!("embedded repository task instruction {instruction_path} is missing")
    })?;
    let instruction = std::str::from_utf8(instruction.data.as_ref()).with_context(|| {
        format!("embedded repository task instruction {instruction_path} is not UTF-8")
    })?;
    compile(source_path, source, instruction)
}

pub fn evaluate_shadow(
    legacy_scenario: &str,
    execution: ExecutionPolicy,
    case: &ScenarioCase,
    observations: &[RepositoryTaskAssessmentObservation],
    legacy: &ObjectiveEvaluation,
) -> RepositoryTaskShadowReport {
    match evaluate_shadow_inner(legacy_scenario, execution, case, observations, legacy) {
        Ok(report) => report,
        Err(error) => RepositoryTaskShadowReport {
            schema: SHADOW_REPORT_SCHEMA.to_string(),
            availability: RepositoryTaskShadowAvailability::Unavailable,
            task_id: legacy_scenario.to_string(),
            task_version: case.scenario_version,
            behavior_sha256: None,
            instruction_sha256: None,
            verifier_protocol: None,
            source: None,
            equivalent: false,
            legacy: Some(project_legacy(legacy)),
            generic: None,
            mismatches: vec![format!("shadow unavailable: {error:#}")],
        },
    }
}

fn evaluate_shadow_inner(
    legacy_scenario: &str,
    execution: ExecutionPolicy,
    case: &ScenarioCase,
    observations: &[RepositoryTaskAssessmentObservation],
    legacy: &ObjectiveEvaluation,
) -> Result<RepositoryTaskShadowReport> {
    let task = embedded_for_legacy(legacy_scenario)?
        .with_context(|| format!("no repository task shadows '{legacy_scenario}'"))?;
    validate_runtime_contract(&task.definition, execution, case)?;
    let source = observe_source(&task.definition.source)?;
    let legacy_projection = project_legacy(legacy);
    let generic_projection = project_generic(&task.definition, observations)?;
    let mut mismatches = Vec::new();
    if legacy_projection.passed != generic_projection.passed {
        mismatches.push(format!(
            "passed differs: legacy={}, generic={}",
            legacy_projection.passed, generic_projection.passed
        ));
    }
    if legacy_projection.score != generic_projection.score {
        mismatches.push(format!(
            "score differs: legacy={}, generic={}",
            legacy_projection.score, generic_projection.score
        ));
    }
    if legacy_projection.gates != generic_projection.gates {
        mismatches.push("hard gate projection differs".to_string());
    }
    if legacy_projection.awards != generic_projection.awards {
        mismatches.push("criterion award projection differs".to_string());
    }
    Ok(RepositoryTaskShadowReport {
        schema: SHADOW_REPORT_SCHEMA.to_string(),
        availability: RepositoryTaskShadowAvailability::Available,
        task_id: task.definition.id,
        task_version: task.definition.version,
        behavior_sha256: Some(task.behavior_sha256),
        instruction_sha256: Some(task.instruction_sha256),
        verifier_protocol: Some(task.definition.verifier.protocol),
        source: Some(source),
        equivalent: mismatches.is_empty(),
        legacy: Some(legacy_projection),
        generic: Some(generic_projection),
        mismatches,
    })
}

fn validate_runtime_contract(
    task: &RepositoryTaskV1,
    execution: ExecutionPolicy,
    case: &ScenarioCase,
) -> Result<()> {
    if task.id != case.scenario_id || task.version != case.scenario_version {
        bail!(
            "repository task identity {}@{} differs from materialized case {}@{}",
            task.id,
            task.version,
            case.scenario_id,
            case.scenario_version
        );
    }
    if task.execution != execution {
        bail!(
            "repository task '{}' execution policy differs from legacy scenario",
            task.id
        );
    }
    if let RepositoryTaskSource::GitCheckout {
        repository,
        revision,
        subtree,
        manifest_sha256,
        ..
    } = &task.source
    {
        for (field, expected) in [
            ("fixture_repository", repository.as_str()),
            ("fixture_revision", revision.as_str()),
            ("fixture_subtree", subtree.as_str()),
            ("fixture_manifest_sha256", manifest_sha256.as_str()),
        ] {
            if case.inputs.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
                bail!(
                    "repository task '{}' source field '{}' differs from materialized case",
                    task.id,
                    field
                );
            }
        }
    }
    Ok(())
}

fn project_legacy(evaluation: &ObjectiveEvaluation) -> RepositoryTaskProjection {
    let gates = evaluation
        .hard_gates
        .iter()
        .map(|gate| (gate.id.clone(), gate.passed))
        .collect::<BTreeMap<_, _>>();
    let awards = evaluation
        .awards
        .iter()
        .map(|award| (award.id.clone(), award.awarded))
        .collect::<BTreeMap<_, _>>();
    RepositoryTaskProjection {
        passed: gates.values().all(|passed| *passed),
        score: awards.values().copied().fold(0_u8, u8::saturating_add),
        gates,
        awards,
    }
}

fn project_generic(
    task: &RepositoryTaskV1,
    observations: &[RepositoryTaskAssessmentObservation],
) -> Result<RepositoryTaskProjection> {
    let observations = observations
        .iter()
        .map(|observation| (observation.id.as_str(), observation))
        .collect::<BTreeMap<_, _>>();
    if observations.len() != task.assessments.len() {
        bail!(
            "repository task '{}' observed {} unique assessments; expected {}",
            task.id,
            observations.len(),
            task.assessments.len()
        );
    }
    let mut gates = BTreeMap::new();
    let mut awards = BTreeMap::new();
    for assessment in &task.assessments {
        let observed = observations.get(assessment.id.as_str()).with_context(|| {
            format!(
                "repository task '{}' did not observe assessment '{}'",
                task.id, assessment.id
            )
        })?;
        if observed.reason.trim().is_empty() {
            bail!(
                "repository task '{}' assessment '{}' has no evidence reason",
                task.id,
                assessment.id
            );
        }
        if observed.awarded > assessment.weight {
            bail!(
                "repository task '{}' assessment '{}' awarded {} above weight {}",
                task.id,
                assessment.id,
                observed.awarded,
                assessment.weight
            );
        }
        match assessment.policy {
            RepositoryTaskAssessmentPolicy::HardGate => {
                gates.insert(
                    assessment.id.clone(),
                    observed.gate_passed.with_context(|| {
                        format!(
                            "repository task '{}' hard gate '{}' has no verdict",
                            task.id, assessment.id
                        )
                    })?,
                );
            }
            RepositoryTaskAssessmentPolicy::Advisory => {
                if observed.gate_passed.is_some() {
                    bail!(
                        "repository task '{}' advisory assessment '{}' emitted a hard gate",
                        task.id,
                        assessment.id
                    );
                }
            }
        }
        awards.insert(assessment.id.clone(), observed.awarded);
    }
    Ok(RepositoryTaskProjection {
        passed: gates.values().all(|passed| *passed),
        score: awards.values().copied().fold(0_u8, u8::saturating_add),
        gates,
        awards,
    })
}

fn observe_source(source: &RepositoryTaskSource) -> Result<RepositoryTaskSourceEvidence> {
    match source {
        RepositoryTaskSource::GitCheckout {
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
                .with_context(|| format!("{path_env} must point to the repository task fixture"))?;
            observe_git_source_at(
                &root,
                repository,
                revision,
                subtree,
                manifest_sha256,
                required_paths,
            )
        }
        RepositoryTaskSource::EmbeddedDirectory {
            path,
            manifest_sha256,
            required_paths,
        } => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
            observe_directory_source(
                &root,
                "embedded_directory",
                None,
                None,
                "",
                manifest_sha256,
                required_paths,
            )
        }
    }
}

fn observe_git_source_at(
    root: &Path,
    repository: &str,
    revision: &str,
    subtree: &str,
    manifest_sha256: &str,
    required_paths: &[String],
) -> Result<RepositoryTaskSourceEvidence> {
    if !root.is_absolute() {
        bail!(
            "repository task fixture root must be absolute: {}",
            root.display()
        );
    }
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize repository task fixture {}", root.display()))?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| {
            format!(
                "read repository task fixture revision from {}",
                root.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "repository task fixture is not a readable Git checkout: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let observed_revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if observed_revision != revision {
        bail!(
            "repository task fixture revision {observed_revision} differs from pinned {revision}"
        );
    }
    observe_directory_source(
        &root.join(subtree),
        "git_checkout",
        Some(repository),
        Some(revision),
        subtree,
        manifest_sha256,
        required_paths,
    )
}

fn observe_directory_source(
    directory: &Path,
    kind: &str,
    repository: Option<&str>,
    revision: Option<&str>,
    subtree: &str,
    manifest_sha256: &str,
    required_paths: &[String],
) -> Result<RepositoryTaskSourceEvidence> {
    if !directory.is_dir() {
        bail!(
            "repository task fixture directory is missing: {}",
            directory.display()
        );
    }
    let paths = collect_files(directory)?;
    let observed = paths.iter().cloned().collect::<BTreeSet<_>>();
    let expected = required_paths.iter().cloned().collect::<BTreeSet<_>>();
    if observed != expected {
        bail!("repository task fixture paths differ: expected {expected:?}, observed {observed:?}");
    }
    let observed_manifest = compute_manifest_sha256(directory, &paths)?;
    if observed_manifest != manifest_sha256 {
        bail!(
            "repository task fixture manifest {observed_manifest} differs from pinned {manifest_sha256}"
        );
    }
    Ok(RepositoryTaskSourceEvidence {
        kind: kind.to_string(),
        repository: repository.map(str::to_string),
        revision: revision.map(str::to_string),
        root: directory.display().to_string(),
        subtree: subtree.to_string(),
        manifest_sha256: observed_manifest,
        paths,
    })
}

fn compute_manifest_sha256(root: &Path, paths: &[String]) -> Result<String> {
    let mut concatenation = Vec::new();
    for relative in paths {
        let bytes = fs::read(root.join(relative))
            .with_context(|| format!("read repository task fixture asset {relative}"))?;
        let file_hash = sha256_bytes(&bytes);
        let hex = file_hash
            .strip_prefix("sha256:")
            .context("repository task fixture asset hash is not sha256-prefixed")?;
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
            .with_context(|| format!("read repository task fixture {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                bail!(
                    "repository task fixture contains symlink: {}",
                    path.display()
                );
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

fn validate_source(source: &RepositoryTaskSource) -> Result<()> {
    let (manifest_sha256, required_paths) = match source {
        RepositoryTaskSource::GitCheckout {
            repository,
            revision,
            subtree,
            manifest_sha256,
            path_env,
            required_paths,
        } => {
            validate_repository(repository)?;
            validate_git_revision(revision)?;
            validate_relative_path(subtree, "fixture subtree")?;
            validate_environment_name(path_env)?;
            (manifest_sha256, required_paths)
        }
        RepositoryTaskSource::EmbeddedDirectory {
            path,
            manifest_sha256,
            required_paths,
        } => {
            validate_relative_path(path, "embedded fixture path")?;
            (manifest_sha256, required_paths)
        }
    };
    validate_sha256(manifest_sha256, "fixture manifest")?;
    if required_paths.is_empty() {
        bail!("repository task source required_paths must not be empty");
    }
    let mut paths = BTreeSet::new();
    for path in required_paths {
        validate_relative_path(path, "required fixture path")?;
        if !paths.insert(path.as_str()) {
            bail!("repository task source repeats required path '{path}'");
        }
    }
    Ok(())
}

fn validate_execution(execution: ExecutionPolicy, id: &str) -> Result<()> {
    if execution.max_turns == 0 || execution.stuck_timeout_seconds == 0 {
        bail!("repository task '{id}' execution limits must be positive");
    }
    if execution.max_output_tokens == Some(0) || execution.max_total_tokens == Some(0) {
        bail!("repository task '{id}' token limits must be positive when present");
    }
    if execution.max_output_tokens.is_some_and(|output| {
        execution
            .max_total_tokens
            .is_some_and(|total| total < output)
    }) {
        bail!("repository task '{id}' total token limit is below output token limit");
    }
    Ok(())
}

fn validate_instruction(instruction: &str) -> Result<()> {
    if instruction.trim().is_empty() {
        bail!("repository task instruction is empty");
    }
    let mut remainder = instruction;
    while let Some(start) = remainder.find("{{") {
        let after = &remainder[start + 2..];
        let end = after
            .find("}}")
            .context("repository task instruction contains an unclosed placeholder")?;
        let placeholder = &after[..end];
        if !matches!(placeholder, "workspace" | "sandbox_name") {
            bail!("repository task instruction uses unknown placeholder '{{{{{placeholder}}}}}'");
        }
        remainder = &after[end + 2..];
    }
    if remainder.contains("}}") {
        bail!("repository task instruction contains an unmatched closing placeholder");
    }
    Ok(())
}

fn validate_slug(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("{label} '{value}' is not a lowercase snake_case identifier");
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
        bail!("repository task repository '{value}' must be an owner/name identifier");
    }
    Ok(())
}

fn validate_git_revision(value: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("repository task revision '{value}' must be a full 40-character Git SHA");
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        bail!("repository task {label} '{value}' must use a sha256: prefix");
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("repository task {label} '{value}' must contain 64 hexadecimal digits");
    }
    Ok(())
}

fn validate_environment_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("repository task path environment '{value}' is invalid");
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
        bail!("repository task {label} '{value}' is not a safe relative path");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{EvaluationDimension, HardGateReport};
    use crate::scenarios::CriterionAward;

    fn shell_task() -> CompiledRepositoryTask {
        embedded_catalog()
            .unwrap()
            .into_iter()
            .find(|task| task.definition.id == "shell_coder_sandbox")
            .unwrap()
    }

    fn performance_task() -> CompiledRepositoryTask {
        embedded_catalog()
            .unwrap()
            .into_iter()
            .find(|task| task.definition.id == "performance_regression")
            .unwrap()
    }

    fn passing_observations(task: &RepositoryTaskV1) -> Vec<RepositoryTaskAssessmentObservation> {
        task.assessments
            .iter()
            .map(|assessment| match assessment.policy {
                RepositoryTaskAssessmentPolicy::HardGate => {
                    RepositoryTaskAssessmentObservation::hard_gate(
                        &assessment.id,
                        true,
                        assessment.weight,
                        "observed",
                    )
                }
                RepositoryTaskAssessmentPolicy::Advisory => {
                    RepositoryTaskAssessmentObservation::advisory(
                        &assessment.id,
                        assessment.weight,
                        "observed",
                    )
                }
            })
            .collect()
    }

    fn passing_legacy(task: &RepositoryTaskV1) -> ObjectiveEvaluation {
        ObjectiveEvaluation {
            hard_gates: task
                .assessments
                .iter()
                .filter(|assessment| assessment.policy == RepositoryTaskAssessmentPolicy::HardGate)
                .map(|assessment| HardGateReport {
                    id: assessment.id.clone(),
                    dimension: EvaluationDimension::StructuralIntegrity,
                    passed: true,
                    reason: "observed".into(),
                })
                .collect(),
            awards: task
                .assessments
                .iter()
                .map(|assessment| CriterionAward {
                    id: assessment.id.clone(),
                    awarded: assessment.weight,
                    reason: "observed".into(),
                })
                .collect(),
            advisory_evidence: Vec::new(),
        }
    }

    #[test]
    fn embedded_tasks_are_valid_and_hash_addressed() {
        let tasks = embedded_catalog().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.definition.id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["performance_regression", "shell_coder_sandbox"])
        );
        for task in tasks {
            assert!(task.behavior_sha256.starts_with("sha256:"));
            assert!(task.instruction_sha256.starts_with("sha256:"));
        }
    }

    #[test]
    fn parser_rejects_unknown_fields_and_unsafe_paths() {
        let task = shell_task();
        let mut value = toml::Value::try_from(&task.definition).unwrap();
        value
            .as_table_mut()
            .unwrap()
            .insert("unexpected".into(), toml::Value::Boolean(true));
        let source = toml::to_string(&value).unwrap();
        assert!(compile("bad/task.toml", &source, "instruction").is_err());

        let mut definition = task.definition;
        match &mut definition.source {
            RepositoryTaskSource::GitCheckout { subtree, .. } => *subtree = "../escape".into(),
            RepositoryTaskSource::EmbeddedDirectory { .. } => unreachable!(),
        }
        assert!(definition.validate().is_err());
    }

    #[test]
    fn generic_projection_matches_an_equivalent_legacy_projection() {
        let task = shell_task().definition;
        let observations = passing_observations(&task);
        let projected = project_generic(&task, &observations).unwrap();
        assert!(projected.passed);
        assert_eq!(projected.score, 100);
        assert_eq!(projected.gates.len(), 6);
    }

    #[test]
    fn shell_task_pins_the_remote_fixture_commit() {
        let task = shell_task().definition;
        let RepositoryTaskSource::GitCheckout {
            repository,
            revision,
            subtree,
            manifest_sha256,
            required_paths,
            ..
        } = task.source
        else {
            panic!("expected Git fixture")
        };
        assert_eq!(repository, "iii-hq/e2e-fixture");
        assert_eq!(revision, "16f6b9e05e34e09c824191eed0631d77f85be6a9");
        assert_eq!(subtree, "shell-coder-sandbox");
        assert_eq!(
            manifest_sha256,
            "sha256:cf8c9afcdf9a52feaee0cf5264c6b4268efe8a7c54ae013ebbd4bf43c44d3b84"
        );
        assert_eq!(
            required_paths,
            vec![
                "TASK.md".to_string(),
                "src/reconcile.py".to_string(),
                "tests/test_reconcile.py".to_string(),
            ]
        );
    }

    #[test]
    fn embedded_performance_source_is_accepted() {
        let task = performance_task().definition;
        let evidence = observe_source(&task.source).unwrap();
        assert_eq!(evidence.kind, "embedded_directory");
        assert_eq!(
            evidence.manifest_sha256,
            "sha256:64243debc670bb4800a1e5a4e271e59c032aa30a9b8eddd4694238b16dac8257"
        );
    }

    #[test]
    fn performance_shadow_is_available_and_equivalent() {
        let compiled = performance_task();
        let materialized = crate::scenarios::performance_regression::materialize(
            "repository-task-shadow",
            crate::scenarios::performance_regression::CANONICAL_SEED,
        )
        .unwrap();
        let observations = passing_observations(&compiled.definition);
        let legacy = passing_legacy(&compiled.definition);
        let report = evaluate_shadow(
            "performance_regression",
            materialized.spec.execution,
            &materialized.case,
            &observations,
            &legacy,
        );
        assert_eq!(
            report.availability,
            RepositoryTaskShadowAvailability::Available
        );
        assert!(report.equivalent, "{:?}", report.mismatches);
        assert!(report.source.is_some());
    }

    #[test]
    fn shadow_reports_projection_mismatches_without_changing_legacy() {
        let compiled = performance_task();
        let materialized = crate::scenarios::performance_regression::materialize(
            "repository-task-mismatch",
            crate::scenarios::performance_regression::CANONICAL_SEED,
        )
        .unwrap();
        let mut observations = passing_observations(&compiled.definition);
        observations[0].awarded = 0;
        observations[0].gate_passed = Some(false);
        let legacy = passing_legacy(&compiled.definition);
        let report = evaluate_shadow(
            "performance_regression",
            materialized.spec.execution,
            &materialized.case,
            &observations,
            &legacy,
        );
        assert_eq!(
            report.availability,
            RepositoryTaskShadowAvailability::Available
        );
        assert!(!report.equivalent);
        assert!(!report.mismatches.is_empty());
        assert!(report.legacy.unwrap().passed);
    }
}
