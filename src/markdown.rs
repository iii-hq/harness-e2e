//! Deterministic compiler and embedded catalog for Markdown-authored scenarios.
//!
//! Authors own only `scenarios/*.md`. This module converts the section-oriented
//! document into a closed, hash-addressed runtime representation. It never asks
//! a model to rewrite or interpret author text.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use rust_embed::RustEmbed;
use schemars::{gen::SchemaGenerator, schema::Schema, JsonSchema};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;

use crate::artifact;
use crate::scenarios::{ExecutionPolicy, ScenarioExecutionKind, ScenarioId};

const REQUIRED_SECTIONS: [&str; 5] = ["Plans", "Version", "Before Test", "Prompt", "Validations"];
pub const LOCAL_SCENARIO_REQUIRED_SECTIONS: [&str; 5] = REQUIRED_SECTIONS;
pub const LOCAL_SCENARIO_PLAN_ID: &str = "local";
pub const LOCAL_SCENARIO_DIRECTORY: &str = "local-scenarios";
pub const LOCAL_SCENARIO_MAX_BYTES: usize = 256 * 1024;
pub const LOCAL_SCENARIO_TEMPLATE: &str = "# Local scenario

## Plans

- local

## Version

1

## Before Test

Prepare the isolated state required by this test. Keep every mutation run-scoped and reversible.

## Prompt

Describe the task the Harness must complete.

## Validations

### Expected outcome (70%)

Describe the evidence that proves the requested outcome is correct.

### Safe execution (30%)

Confirm the run stayed within the intended scope and left no residual state.
";

#[derive(RustEmbed)]
#[folder = "scenarios/"]
#[include = "*.md"]
struct EmbeddedScenarios;

#[derive(RustEmbed)]
#[folder = "config/campaigns/"]
#[include = "*.json"]
struct EmbeddedCampaigns;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkdownCriterion {
    pub id: String,
    pub title: String,
    pub weight: u8,
    pub instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompiledMarkdownScenario {
    pub id: String,
    pub title: String,
    pub source_path: String,
    pub version: u32,
    pub plans: Vec<String>,
    pub before_test: String,
    pub prompt: String,
    pub validations: Vec<MarkdownCriterion>,
    pub source_sha256: String,
    pub behavior_sha256: String,
    pub compiled_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MarkdownScenarioSource {
    pub scenario: CompiledMarkdownScenario,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedMarkdownScenario {
    pub run_id: String,
    pub seed: u64,
    pub before_test: String,
    pub prompt: String,
    pub validations: Vec<MarkdownCriterion>,
}

impl CompiledMarkdownScenario {
    pub fn execution_kind(&self) -> ScenarioExecutionKind {
        ScenarioExecutionKind::HarnessTurn
    }

    pub fn validation_weight(&self) -> u8 {
        self.validations
            .iter()
            .map(|criterion| criterion.weight)
            .sum()
    }
}

pub fn render(
    scenario: &CompiledMarkdownScenario,
    run_id: &str,
    seed: u64,
) -> RenderedMarkdownScenario {
    let render_text = |value: &str| {
        value
            .replace("{{run_id}}", run_id)
            .replace("{{seed}}", &seed.to_string())
    };
    RenderedMarkdownScenario {
        run_id: run_id.to_string(),
        seed,
        before_test: render_text(&scenario.before_test),
        prompt: render_text(&scenario.prompt),
        validations: scenario
            .validations
            .iter()
            .map(|criterion| MarkdownCriterion {
                id: criterion.id.clone(),
                title: criterion.title.clone(),
                weight: criterion.weight,
                instructions: render_text(&criterion.instructions),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioKey {
    BuiltIn(ScenarioId),
    Markdown(String),
}

impl ScenarioKey {
    pub fn as_str(&self) -> &str {
        match self {
            Self::BuiltIn(id) => id.as_str(),
            Self::Markdown(id) => id,
        }
    }

    pub fn built_in(&self) -> Option<ScenarioId> {
        match self {
            Self::BuiltIn(id) => Some(*id),
            Self::Markdown(_) => None,
        }
    }

    pub fn execution_kind(&self) -> ScenarioExecutionKind {
        self.built_in()
            .map(ScenarioId::execution_kind)
            .unwrap_or(ScenarioExecutionKind::HarnessTurn)
    }

    pub fn canonical_seed(&self) -> u64 {
        self.built_in()
            .map(ScenarioId::canonical_seed)
            .unwrap_or_else(|| stable_seed(self.as_str()))
    }

    pub fn canonical_seed_only(&self) -> bool {
        self.built_in().is_some_and(ScenarioId::canonical_seed_only)
    }
}

impl std::fmt::Display for ScenarioKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<ScenarioId> for ScenarioKey {
    fn from(value: ScenarioId) -> Self {
        Self::BuiltIn(value)
    }
}

impl FromStr for ScenarioKey {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        if let Some(id) = ScenarioId::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
        {
            return Ok(Self::BuiltIn(id));
        }
        if let Some(scenario) = embedded_catalog()?
            .into_iter()
            .find(|scenario| scenario.id == value)
        {
            return Ok(Self::Markdown(scenario.id));
        }
        if let Some(local_id) = value.strip_prefix("local_") {
            if !local_id.is_empty() && slug(local_id)? == local_id {
                return Ok(Self::Markdown(value.to_string()));
            }
        }
        bail!("unknown E2E scenario '{value}'")
    }
}

impl Serialize for ScenarioKey {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ScenarioKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(D::Error::custom)
    }
}

impl JsonSchema for ScenarioKey {
    fn schema_name() -> String {
        "ScenarioKey".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        String::json_schema(generator)
    }
}

#[derive(Debug, Clone)]
struct Heading {
    level: usize,
    title: String,
    line_start: usize,
    body_start: usize,
}

pub fn embedded_catalog() -> Result<Vec<CompiledMarkdownScenario>> {
    let plans = embedded_plan_ids()?;
    let mut scenarios = EmbeddedScenarios::iter()
        .map(|path| {
            let bytes = EmbeddedScenarios::get(path.as_ref())
                .with_context(|| format!("read embedded Markdown scenario {path}"))?;
            let source = std::str::from_utf8(bytes.data.as_ref())
                .with_context(|| format!("scenario {path} is not UTF-8"))?;
            compile(path.as_ref(), source, &plans)
        })
        .collect::<Result<Vec<_>>>()?;
    validate_unique_ids(&scenarios)?;
    scenarios.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(scenarios)
}

pub fn embedded_scenario(id: &str) -> Result<CompiledMarkdownScenario> {
    embedded_catalog()?
        .into_iter()
        .find(|scenario| scenario.id == id)
        .with_context(|| format!("unknown Markdown scenario '{id}'"))
}

pub fn embedded_source(scenario: &CompiledMarkdownScenario) -> Result<Vec<u8>> {
    let bytes = EmbeddedScenarios::get(scenario.source_path.as_str())
        .with_context(|| format!("read embedded Markdown scenario {}", scenario.source_path))?;
    Ok(bytes.data.into_owned())
}

pub fn embedded_definition(id: &str) -> Result<MarkdownScenarioSource> {
    let scenario = embedded_scenario(id)?;
    let source = String::from_utf8(embedded_source(&scenario)?)
        .with_context(|| format!("embedded Markdown scenario {id} is not UTF-8"))?;
    Ok(MarkdownScenarioSource { scenario, source })
}

pub fn local_scenario_directory(data_root: &Path) -> PathBuf {
    data_root.join(LOCAL_SCENARIO_DIRECTORY)
}

pub fn local_catalog(data_root: &Path) -> Result<Vec<MarkdownScenarioSource>> {
    let directory = local_scenario_directory(data_root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(&directory)
        .with_context(|| format!("inspect local scenario directory {}", directory.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "local scenario directory {} must be a real directory",
            directory.display()
        );
    }

    let mut paths = fs::read_dir(&directory)
        .with_context(|| format!("read local scenario directory {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().and_then(|extension| extension.to_str()) == Some("md"));
    paths.sort();

    let mut definitions = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect local Markdown scenario {}", path.display()))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!(
                "local Markdown scenario {} must be a real file",
                path.display()
            );
        }
        if metadata.len() > LOCAL_SCENARIO_MAX_BYTES as u64 {
            bail!(
                "local Markdown scenario {} exceeds the {} byte limit",
                path.display(),
                LOCAL_SCENARIO_MAX_BYTES
            );
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("local Markdown scenario must have a UTF-8 file name")?;
        validate_local_file_name(file_name)?;
        let source = fs::read_to_string(&path)
            .with_context(|| format!("read local Markdown scenario {}", path.display()))?;
        let source_path = format!("{LOCAL_SCENARIO_DIRECTORY}/{file_name}");
        let scenario = compile_local(&source_path, &source)?;
        definitions.push(MarkdownScenarioSource { scenario, source });
    }
    validate_local_catalog(&definitions)?;
    definitions.sort_by(|left, right| left.scenario.id.cmp(&right.scenario.id));
    Ok(definitions)
}

pub fn local_scenario(data_root: &Path, id: &str) -> Result<MarkdownScenarioSource> {
    local_catalog(data_root)?
        .into_iter()
        .find(|definition| definition.scenario.id == id)
        .with_context(|| format!("unknown local Markdown scenario '{id}'"))
}

pub fn create_local_scenario(
    data_root: &Path,
    file_name: &str,
    source: &str,
) -> Result<MarkdownScenarioSource> {
    let file_name = validate_local_file_name(file_name)?;
    if source.len() > LOCAL_SCENARIO_MAX_BYTES {
        bail!(
            "local Markdown scenario exceeds the {} byte limit",
            LOCAL_SCENARIO_MAX_BYTES
        );
    }
    let source_path = format!("{LOCAL_SCENARIO_DIRECTORY}/{file_name}");
    let scenario = compile_local(&source_path, source)?;
    if embedded_catalog()?
        .iter()
        .any(|existing| existing.id == scenario.id)
    {
        bail!(
            "local scenario id '{}' conflicts with an embedded scenario",
            scenario.id
        );
    }

    let directory = local_scenario_directory(data_root);
    fs::create_dir_all(&directory)
        .with_context(|| format!("create local scenario directory {}", directory.display()))?;
    let directory_metadata = fs::symlink_metadata(&directory)
        .with_context(|| format!("inspect local scenario directory {}", directory.display()))?;
    if !directory_metadata.file_type().is_dir() || directory_metadata.file_type().is_symlink() {
        bail!(
            "local scenario directory {} must be a real directory",
            directory.display()
        );
    }
    let target = directory.join(&file_name);
    let temporary = directory.join(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let write_result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("create temporary local scenario {}", temporary.display()))?;
        file.write_all(source.as_bytes())
            .with_context(|| format!("write temporary local scenario {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary local scenario {}", temporary.display()))?;
        fs::hard_link(&temporary, &target).with_context(|| {
            format!(
                "create local scenario {}; a scenario with this file name may already exist",
                target.display()
            )
        })?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    write_result?;

    let definition = MarkdownScenarioSource {
        scenario,
        source: source.to_string(),
    };
    local_catalog(data_root)?
        .into_iter()
        .find(|candidate| candidate.scenario.id == definition.scenario.id)
        .context("new local scenario was not present after persistence")
}

pub fn compile_local(path: &str, source: &str) -> Result<CompiledMarkdownScenario> {
    let mut scenario = compile(
        path,
        source,
        &BTreeSet::from([LOCAL_SCENARIO_PLAN_ID.to_string()]),
    )?;
    scenario.id = format!("local_{}", scenario.id);
    scenario.compiled_sha256 = artifact::sha256_value(&json!({
        "id": scenario.id,
        "title": scenario.title,
        "source_path": scenario.source_path,
        "version": scenario.version,
        "plans": scenario.plans,
        "before_test": scenario.before_test,
        "prompt": scenario.prompt,
        "validations": scenario.validations,
        "source_sha256": scenario.source_sha256,
        "behavior_sha256": scenario.behavior_sha256,
    }))?;
    Ok(scenario)
}

pub fn validate_local_definition(definition: &MarkdownScenarioSource) -> Result<()> {
    let compiled = compile_local(&definition.scenario.source_path, &definition.source)?;
    if compiled != definition.scenario {
        bail!(
            "local Markdown scenario '{}' differs from its frozen compiled definition",
            definition.scenario.id
        );
    }
    Ok(())
}

pub fn all_keys() -> Result<Vec<ScenarioKey>> {
    let mut keys = ScenarioId::ALL
        .into_iter()
        .map(ScenarioKey::BuiltIn)
        .collect::<Vec<_>>();
    keys.extend(
        embedded_catalog()?
            .into_iter()
            .map(|scenario| ScenarioKey::Markdown(scenario.id)),
    );
    Ok(keys)
}

pub fn all_keys_with_local(data_root: &Path) -> Result<Vec<ScenarioKey>> {
    let mut keys = all_keys()?;
    keys.extend(
        local_catalog(data_root)?
            .into_iter()
            .map(|definition| ScenarioKey::Markdown(definition.scenario.id)),
    );
    Ok(keys)
}

pub fn default_keys() -> Vec<ScenarioKey> {
    ScenarioId::ALL
        .into_iter()
        .map(ScenarioKey::BuiltIn)
        .collect()
}

pub fn selected_keys(requested: &[ScenarioKey]) -> Result<Vec<ScenarioKey>> {
    if requested.is_empty() {
        return Ok(default_keys());
    }
    Ok(requested.iter().cloned().fold(Vec::new(), |mut keys, key| {
        if !keys.contains(&key) {
            keys.push(key);
        }
        keys
    }))
}

pub const fn execution_policy() -> ExecutionPolicy {
    ExecutionPolicy {
        max_turns: 24,
        max_output_tokens: Some(8_192),
        max_total_tokens: Some(160_000),
        stuck_timeout_seconds: 300,
        max_validation_retries: Some(1),
    }
}

pub fn validate_directory(
    directory: &Path,
    campaigns: &Path,
) -> Result<Vec<CompiledMarkdownScenario>> {
    let plans = plan_ids_from_directory(campaigns)?;
    let mut paths = fs::read_dir(directory)
        .with_context(|| format!("read Markdown scenario directory {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().and_then(|extension| extension.to_str()) == Some("md"));
    paths.sort();
    let mut scenarios = paths
        .iter()
        .map(|path| {
            let source = fs::read_to_string(path)
                .with_context(|| format!("read Markdown scenario {}", path.display()))?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("Markdown scenario path must have a UTF-8 file name")?;
            compile(name, &source, &plans)
        })
        .collect::<Result<Vec<_>>>()?;
    validate_unique_ids(&scenarios)?;
    scenarios.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(scenarios)
}

pub fn validate_version_progression(
    current: &[CompiledMarkdownScenario],
    base_directory: &Path,
    campaigns: &Path,
) -> Result<()> {
    let base = validate_directory(base_directory, campaigns)?;
    validate_compiled_version_progression(current, &base)
}

fn validate_compiled_version_progression(
    current: &[CompiledMarkdownScenario],
    base: &[CompiledMarkdownScenario],
) -> Result<()> {
    let base_by_id = base
        .iter()
        .map(|scenario| (scenario.id.as_str(), scenario))
        .collect::<std::collections::HashMap<_, _>>();
    for scenario in current {
        let Some(previous) = base_by_id.get(scenario.id.as_str()) else {
            continue;
        };
        if scenario.version < previous.version {
            bail!(
                "scenario {} decreased version from {} to {}",
                scenario.id,
                previous.version,
                scenario.version
            );
        }
        if scenario.behavior_sha256 != previous.behavior_sha256
            && scenario.version <= previous.version
        {
            bail!(
                "scenario {} changed setup, prompt, validations, or weights without incrementing version {}",
                scenario.id,
                previous.version
            );
        }
    }
    Ok(())
}

pub fn compile(
    path: &str,
    source: &str,
    allowed_plans: &BTreeSet<String>,
) -> Result<CompiledMarkdownScenario> {
    if source.trim().is_empty() {
        bail!("scenario {path} is empty");
    }
    if source.starts_with("---\n") || source.starts_with("---\r\n") {
        bail!("scenario {path} cannot contain YAML frontmatter");
    }
    let id = scenario_id(path)?;
    let headings = headings(source)?;
    let title_heading = headings
        .iter()
        .find(|heading| heading.level == 1)
        .context("Markdown scenario must contain one H1 title")?;
    if headings.iter().filter(|heading| heading.level == 1).count() != 1 {
        bail!("scenario {path} must contain exactly one H1 title");
    }
    let title = title_heading.title.trim().to_string();
    if title.is_empty() {
        bail!("scenario {path} has an empty H1 title");
    }

    let h2 = headings
        .iter()
        .filter(|heading| heading.level == 2)
        .collect::<Vec<_>>();
    for required in REQUIRED_SECTIONS {
        let count = h2
            .iter()
            .filter(|heading| heading.title == required)
            .count();
        if count != 1 {
            bail!("scenario {path} must contain exactly one '## {required}' section");
        }
    }
    for heading in &h2 {
        if !REQUIRED_SECTIONS.contains(&heading.title.as_str()) {
            bail!(
                "scenario {path} contains unsupported section '## {}'",
                heading.title
            );
        }
    }
    let observed_sections = h2
        .iter()
        .map(|heading| heading.title.as_str())
        .collect::<Vec<_>>();
    if observed_sections != REQUIRED_SECTIONS {
        bail!(
            "scenario {path} sections must appear in this order: {}",
            REQUIRED_SECTIONS.join(", ")
        );
    }

    let plans_body = section_body(source, &headings, "Plans")?;
    let plans = parse_plans(path, plans_body, allowed_plans)?;
    let version = section_body(source, &headings, "Version")?
        .trim()
        .parse::<u32>()
        .with_context(|| format!("scenario {path} version must be a positive integer"))?;
    if version == 0 {
        bail!("scenario {path} version must be at least 1");
    }
    let before_test = section_body(source, &headings, "Before Test")?.to_string();
    let prompt = section_body(source, &headings, "Prompt")?.to_string();
    if before_test.trim().is_empty() || prompt.trim().is_empty() {
        bail!("scenario {path} setup and prompt sections cannot be empty");
    }
    let validations_heading = headings
        .iter()
        .find(|heading| heading.level == 2 && heading.title == "Validations")
        .expect("required validation heading was checked");
    let validations = parse_validations(path, source, &headings, validations_heading)?;
    validate_template_variables(path, &before_test)?;
    validate_template_variables(path, &prompt)?;
    for criterion in &validations {
        validate_template_variables(path, &criterion.instructions)?;
    }

    let source_sha256 = artifact::sha256_bytes(source.as_bytes());
    let behavior_sha256 = artifact::sha256_value(&json!({
        "before_test": before_test,
        "prompt": prompt,
        "validations": validations,
    }))?;
    let compiled_sha256 = artifact::sha256_value(&json!({
        "id": id,
        "title": title,
        "source_path": path,
        "version": version,
        "plans": plans,
        "before_test": before_test,
        "prompt": prompt,
        "validations": validations,
        "source_sha256": source_sha256,
        "behavior_sha256": behavior_sha256,
    }))?;
    Ok(CompiledMarkdownScenario {
        id,
        title,
        source_path: path.to_string(),
        version,
        plans,
        before_test,
        prompt,
        validations,
        source_sha256,
        behavior_sha256,
        compiled_sha256,
    })
}

fn validate_template_variables(path: &str, value: &str) -> Result<()> {
    let mut remainder = value;
    while let Some(open) = remainder.find("{{") {
        let after_open = &remainder[open + 2..];
        let close = after_open
            .find("}}")
            .with_context(|| format!("scenario {path} contains an unclosed template variable"))?;
        let variable = after_open[..close].trim();
        if !matches!(variable, "run_id" | "seed") {
            bail!("scenario {path} references unsupported template variable '{{{{{variable}}}}}'");
        }
        remainder = &after_open[close + 2..];
    }
    Ok(())
}

fn headings(source: &str) -> Result<Vec<Heading>> {
    let mut headings = Vec::new();
    let mut offset = 0usize;
    let mut fence: Option<char> = None;
    for line in source.split_inclusive('\n') {
        let without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed = without_newline.trim_start();
        let fence_marker = trimmed
            .chars()
            .next()
            .filter(|marker| matches!(marker, '`' | '~'))
            .filter(|marker| trimmed.chars().take_while(|value| value == marker).count() >= 3);
        if let Some(marker) = fence_marker {
            match fence {
                Some(active) if active == marker => fence = None,
                None => fence = Some(marker),
                _ => {}
            }
            offset += line.len();
            continue;
        }
        if fence.is_some() {
            offset += line.len();
            continue;
        }
        let hashes = without_newline
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if (1..=6).contains(&hashes)
            && without_newline.as_bytes().get(hashes).copied() == Some(b' ')
        {
            headings.push(Heading {
                level: hashes,
                title: without_newline[hashes + 1..].trim().to_string(),
                line_start: offset,
                body_start: offset + line.len(),
            });
        }
        offset += line.len();
    }
    if fence.is_some() {
        bail!("Markdown scenario contains an unclosed fenced code block");
    }
    if offset < source.len() {
        return Err(anyhow!(
            "Markdown parser did not consume the complete source"
        ));
    }
    Ok(headings)
}

fn section_body<'a>(source: &'a str, headings: &[Heading], title: &str) -> Result<&'a str> {
    let index = headings
        .iter()
        .position(|heading| heading.level == 2 && heading.title == title)
        .with_context(|| format!("missing section ## {title}"))?;
    let heading = &headings[index];
    let end = headings[index + 1..]
        .iter()
        .find(|candidate| candidate.level <= 2)
        .map_or(source.len(), |candidate| candidate.line_start);
    Ok(&source[heading.body_start..end])
}

fn parse_plans(path: &str, body: &str, allowed: &BTreeSet<String>) -> Result<Vec<String>> {
    let mut plans = Vec::new();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let plan = line
            .strip_prefix("- ")
            .with_context(|| format!("scenario {path} plans must be a Markdown bullet list"))?
            .trim();
        if !allowed.contains(plan) {
            bail!("scenario {path} references unknown plan '{plan}'");
        }
        if plans.iter().any(|existing| existing == plan) {
            bail!("scenario {path} repeats plan '{plan}'");
        }
        plans.push(plan.to_string());
    }
    if plans.is_empty() {
        bail!("scenario {path} must belong to at least one plan");
    }
    Ok(plans)
}

fn parse_validations(
    path: &str,
    source: &str,
    headings: &[Heading],
    validations_heading: &Heading,
) -> Result<Vec<MarkdownCriterion>> {
    let next_h2 = headings
        .iter()
        .find(|heading| heading.line_start > validations_heading.line_start && heading.level <= 2)
        .map_or(source.len(), |heading| heading.line_start);
    let criterion_headings = headings
        .iter()
        .filter(|heading| {
            heading.level == 3
                && heading.line_start > validations_heading.line_start
                && heading.line_start < next_h2
        })
        .collect::<Vec<_>>();
    if criterion_headings.is_empty() {
        bail!("scenario {path} must contain at least one validation criterion");
    }
    let mut validations = Vec::with_capacity(criterion_headings.len());
    for (index, heading) in criterion_headings.iter().enumerate() {
        let (title, weight) = criterion_title(path, &heading.title)?;
        let end = criterion_headings
            .get(index + 1)
            .map_or(next_h2, |next| next.line_start);
        let instructions = source[heading.body_start..end].to_string();
        if instructions.trim().is_empty() {
            bail!("scenario {path} validation '{title}' cannot be empty");
        }
        validations.push(MarkdownCriterion {
            id: format!("{:02}_{}", index + 1, slug(&title)?),
            title,
            weight,
            instructions,
        });
    }
    let unique_ids = validations
        .iter()
        .map(|criterion| criterion.id.as_str())
        .collect::<HashSet<_>>();
    if unique_ids.len() != validations.len() {
        bail!("scenario {path} contains validation titles that produce duplicate ids");
    }
    let total = validations
        .iter()
        .try_fold(0u8, |total, criterion| total.checked_add(criterion.weight))
        .context("validation weight overflow")?;
    if total != 100 {
        bail!("scenario {path} validation weights total {total}; expected exactly 100");
    }
    Ok(validations)
}

fn criterion_title(path: &str, raw: &str) -> Result<(String, u8)> {
    let open = raw
        .rfind(" (")
        .with_context(|| format!("scenario {path} validation heading must end in '(N%)'"))?;
    let percentage = raw[open + 2..]
        .strip_suffix("%)")
        .with_context(|| format!("scenario {path} validation heading must end in '(N%)'"))?
        .parse::<u8>()
        .with_context(|| format!("scenario {path} validation weight must be an integer"))?;
    if percentage == 0 {
        bail!("scenario {path} validation weights must be positive");
    }
    let title = raw[..open].trim().to_string();
    if title.is_empty() {
        bail!("scenario {path} validation title cannot be empty");
    }
    Ok((title, percentage))
}

fn scenario_id(path: &str) -> Result<String> {
    let path = Path::new(path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("Markdown scenario must have a UTF-8 file stem")?;
    slug(stem)
}

fn slug(value: &str) -> Result<String> {
    let normalized = value
        .chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' | '_' => character,
            'A'..='Z' => character.to_ascii_lowercase(),
            '-' | ' ' => '_',
            _ => '\0',
        })
        .collect::<String>();
    if normalized.is_empty()
        || normalized.starts_with('_')
        || normalized.contains('\0')
        || normalized.contains("__")
    {
        bail!("'{value}' cannot be converted to a safe scenario identifier");
    }
    Ok(normalized)
}

fn embedded_plan_ids() -> Result<BTreeSet<String>> {
    let mut plans = BTreeSet::new();
    for path in EmbeddedCampaigns::iter() {
        let bytes = EmbeddedCampaigns::get(path.as_ref())
            .with_context(|| format!("read embedded campaign {path}"))?;
        let value: serde_json::Value = serde_json::from_slice(bytes.data.as_ref())
            .with_context(|| format!("decode embedded campaign {path}"))?;
        let id = value
            .get("campaign_id")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("campaign {path} has no campaign_id"))?;
        plans.insert(id.to_string());
    }
    Ok(plans)
}

fn plan_ids_from_directory(directory: &Path) -> Result<BTreeSet<String>> {
    let mut plans = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read campaign directory {}", directory.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("decode campaign {}", path.display()))?;
        let id = value
            .get("campaign_id")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("campaign {} has no campaign_id", path.display()))?;
        plans.insert(id.to_string());
    }
    Ok(plans)
}

fn validate_unique_ids(scenarios: &[CompiledMarkdownScenario]) -> Result<()> {
    let built_ins = ScenarioId::ALL
        .into_iter()
        .map(|scenario| scenario.as_str())
        .collect::<HashSet<_>>();
    let mut ids = HashSet::new();
    for scenario in scenarios {
        if built_ins.contains(scenario.id.as_str()) || !ids.insert(scenario.id.as_str()) {
            bail!("duplicate E2E scenario id '{}'", scenario.id);
        }
    }
    Ok(())
}

fn validate_local_catalog(definitions: &[MarkdownScenarioSource]) -> Result<()> {
    let scenarios = definitions
        .iter()
        .map(|definition| definition.scenario.clone())
        .collect::<Vec<_>>();
    validate_unique_ids(&scenarios)?;
    let embedded = embedded_catalog()?
        .into_iter()
        .map(|scenario| scenario.id)
        .collect::<HashSet<_>>();
    if let Some(conflict) = definitions
        .iter()
        .find(|definition| embedded.contains(&definition.scenario.id))
    {
        bail!(
            "local scenario id '{}' conflicts with an embedded scenario",
            conflict.scenario.id
        );
    }
    Ok(())
}

fn validate_local_file_name(file_name: &str) -> Result<String> {
    if file_name.len() > 128 {
        bail!("local scenario file name exceeds 128 bytes");
    }
    let path = Path::new(file_name);
    if path.components().count() != 1
        || path.file_name().and_then(|name| name.to_str()) != Some(file_name)
        || path.extension().and_then(|extension| extension.to_str()) != Some("md")
    {
        bail!("local scenario file name must be one safe .md file name");
    }
    scenario_id(file_name)?;
    Ok(file_name.to_string())
}

fn stable_seed(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn default_scenario_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

pub fn default_campaign_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/campaigns")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plans() -> BTreeSet<String> {
        ["daily", "weekly"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn source() -> &'static str {
        "# Insert row\n\n## Plans\n\n- daily\n- weekly\n\n## Version\n\n1\n\n## Before Test\n\nPrepare the database.\n\n## Prompt\n\nInsert one row.\n\n## Validations\n\n### Row exists (80%)\n\nThe row exists.\n\n### Under ten turns (20%)\n\nFewer than ten turns were used.\n"
    }

    fn local_source() -> String {
        source().replace("- daily\n- weekly", "- local")
    }

    #[test]
    fn compiles_and_hashes_a_canonical_document() {
        let first = compile("insert-row.md", source(), &plans()).unwrap();
        let second = compile("insert-row.md", source(), &plans()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.id, "insert_row");
        assert_eq!(first.validation_weight(), 100);
        assert_eq!(first.compiled_sha256, second.compiled_sha256);
    }

    #[test]
    fn scenario_keys_preserve_legacy_json_and_accept_markdown_ids() {
        let built_in = ScenarioKey::BuiltIn(ScenarioId::ContextPressure);
        assert_eq!(
            serde_json::to_string(&built_in).unwrap(),
            "\"context_pressure\""
        );
        assert_eq!(
            serde_json::from_str::<ScenarioKey>("\"context_pressure\"").unwrap(),
            built_in
        );
        let markdown = serde_json::from_str::<ScenarioKey>("\"insert_record\"").unwrap();
        assert_eq!(markdown.as_str(), "insert_record");
        assert_eq!(
            serde_json::to_string(&markdown).unwrap(),
            "\"insert_record\""
        );
        for migrated in [
            "database_migration_recovery",
            "minimal_path",
            "sequential_pipeline",
        ] {
            let key = migrated.parse::<ScenarioKey>().unwrap();
            assert_eq!(key, ScenarioKey::Markdown(migrated.into()));
            assert!(key.built_in().is_none());
        }
    }

    #[test]
    fn plan_membership_does_not_change_behavior_hash() {
        let first = compile("insert-row.md", source(), &plans()).unwrap();
        let changed = source().replace("- weekly\n", "");
        let second = compile("insert-row.md", &changed, &plans()).unwrap();
        assert_eq!(first.behavior_sha256, second.behavior_sha256);
        assert_ne!(first.compiled_sha256, second.compiled_sha256);
    }

    #[test]
    fn renders_only_frozen_run_and_seed_variables() {
        let templated = source()
            .replace(
                "Prepare the database.",
                "Prepare scope {{run_id}} for seed {{seed}}.",
            )
            .replace("Insert one row.", "Insert one row for {{run_id}}.");
        let scenario = compile("insert-row.md", &templated, &plans()).unwrap();

        let rendered = render(&scenario, "run-123", 42);

        assert!(rendered.before_test.contains("scope run-123 for seed 42"));
        assert!(rendered.prompt.contains("row for run-123"));
        assert_eq!(rendered.run_id, "run-123");
        assert_eq!(rendered.seed, 42);
        assert!(scenario.before_test.contains("{{run_id}}"));
    }

    #[test]
    fn rejects_unknown_or_unclosed_template_variables() {
        let unknown = source().replace("Prepare the database.", "Prepare {{attempt_id}}.");
        assert!(compile("invalid.md", &unknown, &plans())
            .unwrap_err()
            .to_string()
            .contains("unsupported template variable"));

        let unclosed = source().replace("Prepare the database.", "Prepare {{run_id.");
        assert!(compile("invalid.md", &unclosed, &plans())
            .unwrap_err()
            .to_string()
            .contains("unclosed template variable"));
    }

    #[test]
    fn external_formatting_and_title_do_not_change_behavior_hash() {
        let first = compile("insert-row.md", source(), &plans()).unwrap();
        let changed = source().replacen("# Insert row", "\n# Insert one row", 1);
        let second = compile("insert-row.md", &changed, &plans()).unwrap();
        assert_eq!(first.behavior_sha256, second.behavior_sha256);
        assert_ne!(first.source_sha256, second.source_sha256);
    }

    #[test]
    fn rejects_missing_sections_unknown_plans_and_bad_weights() {
        let missing = source().replace("## Prompt", "## Missing");
        assert!(compile("invalid.md", &missing, &plans()).is_err());
        let unknown = source().replace("- weekly", "- monthly");
        assert!(compile("invalid.md", &unknown, &plans()).is_err());
        let weights = source().replace("(20%)", "(19%)");
        assert!(compile("invalid.md", &weights, &plans()).is_err());
        let repeated = source().replace("## Prompt", "## Prompt\n\nDuplicate.\n\n## Prompt");
        assert!(compile("invalid.md", &repeated, &plans()).is_err());
        let malformed = source().replace("### Row exists (80%)", "### Row exists — 80%");
        assert!(compile("invalid.md", &malformed, &plans()).is_err());
        let reordered = source().replace(
            "## Before Test\n\nPrepare the database.\n\n## Prompt\n\nInsert one row.",
            "## Prompt\n\nInsert one row.\n\n## Before Test\n\nPrepare the database.",
        );
        assert!(compile("invalid.md", &reordered, &plans()).is_err());
    }

    #[test]
    fn rejects_frontmatter_and_unsafe_file_names() {
        assert!(compile(
            "invalid.md",
            &format!("---\na: b\n---\n{}", source()),
            &plans()
        )
        .is_err());
        assert!(compile("unsafe!.md", source(), &plans()).is_err());
    }

    #[test]
    fn headings_inside_fenced_prompt_content_are_preserved_as_content() {
        let changed = source().replace(
            "Insert one row.",
            "Use this example:\n\n```md\n## Not a section\n### Not a criterion (99%)\n```",
        );
        let compiled = compile("insert-row.md", &changed, &plans()).unwrap();
        assert!(compiled.prompt.contains("## Not a section"));
        assert_eq!(compiled.validations.len(), 2);
    }

    #[test]
    fn detects_duplicate_generated_ids() {
        let first = compile("insert-row.md", source(), &plans()).unwrap();
        let second = compile("insert_row.md", source(), &plans()).unwrap();
        assert!(validate_unique_ids(&[first, second]).is_err());
    }

    #[test]
    fn behavioral_changes_require_a_version_increment_but_plan_changes_do_not() {
        let base = compile("insert-row.md", source(), &plans()).unwrap();
        let changed_prompt = source().replace("Insert one row.", "Insert exactly one row.");
        let unchanged_version = compile("insert-row.md", &changed_prompt, &plans()).unwrap();
        assert!(validate_compiled_version_progression(
            std::slice::from_ref(&unchanged_version),
            std::slice::from_ref(&base),
        )
        .is_err());

        let bumped = compile(
            "insert-row.md",
            &changed_prompt.replace("## Version\n\n1", "## Version\n\n2"),
            &plans(),
        )
        .unwrap();
        validate_compiled_version_progression(
            std::slice::from_ref(&bumped),
            std::slice::from_ref(&base),
        )
        .unwrap();

        let plan_only = compile(
            "insert-row.md",
            &source().replace("- weekly\n", ""),
            &plans(),
        )
        .unwrap();
        validate_compiled_version_progression(&[plan_only], &[base]).unwrap();
    }

    #[test]
    fn creates_and_loads_local_scenarios_outside_the_repository() {
        let root = tempfile::tempdir().unwrap();
        let definition =
            create_local_scenario(root.path(), "console-draft.md", &local_source()).unwrap();

        assert_eq!(definition.scenario.id, "local_console_draft");
        assert_eq!(definition.scenario.plans, ["local"]);
        assert_eq!(
            definition.scenario.source_path,
            "local-scenarios/console-draft.md"
        );
        assert_eq!(
            fs::read_to_string(local_scenario_directory(root.path()).join("console-draft.md"))
                .unwrap(),
            local_source()
        );
        assert_eq!(
            local_catalog(root.path()).unwrap().as_slice(),
            std::slice::from_ref(&definition)
        );
        validate_local_definition(&definition).unwrap();
        assert!(all_keys_with_local(root.path())
            .unwrap()
            .iter()
            .any(|key| key.as_str() == "local_console_draft"));
    }

    #[test]
    fn local_scenario_creation_never_overwrites_or_escapes_its_data_directory() {
        let root = tempfile::tempdir().unwrap();
        create_local_scenario(root.path(), "draft.md", &local_source()).unwrap();
        assert!(create_local_scenario(root.path(), "draft.md", &local_source()).is_err());
        assert!(create_local_scenario(root.path(), "../escape.md", &local_source()).is_err());
        assert!(create_local_scenario(root.path(), "draft.txt", &local_source()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_scenario_directory_cannot_be_redirected_through_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), local_scenario_directory(root.path())).unwrap();

        assert!(create_local_scenario(root.path(), "draft.md", &local_source()).is_err());
        assert!(!outside.path().join("draft.md").exists());
    }
}
