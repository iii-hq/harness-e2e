use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod adaptive_runtime;

pub use adaptive_runtime::{
    adaptive_policy, build_adaptive_runtime, reference_adaptive_plans, CrossRepoAdaptiveRuntime,
    CrossRepoRuntimeState,
};

pub const SCENARIO_ID: &str = "cross_repo_contract_migration";
pub const SCENARIO_VERSION: u32 = 1;
pub const CANARY_EVIDENCE_ID: &str = "canary.consumer_b_missing_alias";
const MARKER_FILE: &str = ".harness-e2e-cross-repo-fixture.json";
const FIXED_GIT_NAME: &str = "Harness E2E Fixture";
const FIXED_GIT_EMAIL: &str = "harness-e2e-fixture@example.invalid";
const INITIAL_GIT_DATE: &str = "2001-01-01T00:00:00Z";
const PLAN_A_GIT_DATE: &str = "2001-01-02T00:00:00Z";
const PLAN_B_GIT_DATE: &str = "2001-01-03T00:00:00Z";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TemplatePhase {
    TrustedAnchor,
    AgentSelected,
    DeterministicGate,
    WorkspaceMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplaySafety {
    Idempotent,
    Compensable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateDescriptor {
    pub id: String,
    pub revision: u8,
    pub phase: TemplatePhase,
    pub replay_safety: ReplaySafety,
    pub allowed_roots: Vec<String>,
    pub network_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanRevisionRequest {
    pub revision: u8,
    #[serde(default)]
    pub selected_templates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_sha256: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveNode {
    pub id: String,
    pub template_id: String,
    pub depends_on: Vec<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterializedAdaptiveDag {
    pub scenario_id: String,
    pub scenario_version: u32,
    pub revision: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_sha256: Option<String>,
    pub evidence_ids: Vec<String>,
    pub nodes: Vec<AdaptiveNode>,
    pub sha256: String,
}

pub fn template_catalog() -> Vec<TemplateDescriptor> {
    [
        (
            "materialize_visible_repositories",
            1,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Compensable,
        ),
        (
            "inspect_producer_contract",
            1,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
        ),
        (
            "inspect_consumer_a",
            1,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
        ),
        (
            "inspect_git_provenance",
            1,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
        ),
        (
            "migrate_visible_contract",
            1,
            TemplatePhase::WorkspaceMutation,
            ReplaySafety::Compensable,
        ),
        (
            "validate_visible_matrix",
            1,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
        ),
        (
            "reveal_consumer_b_canary",
            1,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Compensable,
        ),
        (
            "resume_from_canary",
            2,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Idempotent,
        ),
        (
            "inspect_consumer_b",
            2,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
        ),
        (
            "inspect_legacy_alias_history",
            2,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
        ),
        (
            "inspect_compatibility_matrix",
            2,
            TemplatePhase::AgentSelected,
            ReplaySafety::Idempotent,
        ),
        (
            "add_legacy_alias",
            2,
            TemplatePhase::WorkspaceMutation,
            ReplaySafety::Compensable,
        ),
        (
            "validate_full_matrix",
            2,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
        ),
        (
            "validate_workspace_boundaries",
            2,
            TemplatePhase::DeterministicGate,
            ReplaySafety::Idempotent,
        ),
        (
            "reconcile_repositories",
            2,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Idempotent,
        ),
        (
            "cleanup_workspace",
            2,
            TemplatePhase::TrustedAnchor,
            ReplaySafety::Compensable,
        ),
    ]
    .into_iter()
    .map(|(id, revision, phase, replay_safety)| TemplateDescriptor {
        id: id.into(),
        revision,
        phase,
        replay_safety,
        allowed_roots: vec!["producer".into(), "consumer-a".into(), "consumer-b".into()],
        network_allowed: false,
    })
    .collect()
}

pub fn materialize_plan(request: &PlanRevisionRequest) -> Result<MaterializedAdaptiveDag> {
    if !matches!(request.revision, 1 | 2) {
        bail!("cross-repository migration supports exactly two plan revisions");
    }
    if request.revision == 1 {
        if request.supersedes_sha256.is_some() || !request.evidence_ids.is_empty() {
            bail!("revision 1 cannot supersede a plan or cite canary evidence");
        }
    } else {
        let supersedes = request
            .supersedes_sha256
            .as_deref()
            .filter(|value| is_sha256(value))
            .context("revision 2 requires a valid supersedes_sha256")?;
        if supersedes.chars().all(|value| value == '0') {
            bail!("revision 2 cannot supersede an all-zero hash");
        }
        if !request
            .evidence_ids
            .iter()
            .any(|value| value == CANARY_EVIDENCE_ID)
        {
            bail!("revision 2 must cite '{CANARY_EVIDENCE_ID}'");
        }
    }

    let catalog = template_catalog()
        .into_iter()
        .map(|descriptor| (descriptor.id.clone(), descriptor))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();
    for template_id in &request.selected_templates {
        let descriptor = catalog
            .get(template_id)
            .with_context(|| format!("unknown cross-repository template '{template_id}'"))?;
        if descriptor.revision != request.revision
            || descriptor.phase != TemplatePhase::AgentSelected
        {
            bail!(
                "template '{template_id}' is not agent-selectable in revision {}",
                request.revision
            );
        }
        if !selected.insert(template_id.clone()) {
            bail!("template '{template_id}' was selected more than once");
        }
    }
    if selected.is_empty() || selected.len() > 3 {
        bail!("each migration revision must select between one and three analysis templates");
    }

    let nodes = if request.revision == 1 {
        first_revision_nodes(selected.into_iter().collect())
    } else {
        second_revision_nodes(selected.into_iter().collect())
    };
    let sha256 = canonical_sha256(&(
        SCENARIO_ID,
        SCENARIO_VERSION,
        request.revision,
        &request.supersedes_sha256,
        &request.evidence_ids,
        &nodes,
    ))?;
    Ok(MaterializedAdaptiveDag {
        scenario_id: SCENARIO_ID.into(),
        scenario_version: SCENARIO_VERSION,
        revision: request.revision,
        supersedes_sha256: request.supersedes_sha256.clone(),
        evidence_ids: request.evidence_ids.clone(),
        nodes,
        sha256,
    })
}

fn first_revision_nodes(selected: Vec<String>) -> Vec<AdaptiveNode> {
    let mut nodes = vec![node("materialize", "materialize_visible_repositories", &[])];
    let mut analyses = Vec::new();
    for (index, template) in selected.into_iter().enumerate() {
        let id = format!("analysis_{}", index + 1);
        analyses.push(id.clone());
        nodes.push(AdaptiveNode {
            id,
            template_id: template,
            depends_on: vec!["materialize".into()],
            required: true,
        });
    }
    nodes.push(AdaptiveNode {
        id: "migrate_visible".into(),
        template_id: "migrate_visible_contract".into(),
        depends_on: analyses,
        required: true,
    });
    nodes.push(node(
        "visible_matrix",
        "validate_visible_matrix",
        &["migrate_visible"],
    ));
    nodes.push(node(
        "trusted_canary",
        "reveal_consumer_b_canary",
        &["visible_matrix"],
    ));
    nodes
}

fn second_revision_nodes(selected: Vec<String>) -> Vec<AdaptiveNode> {
    let mut nodes = vec![node("resume", "resume_from_canary", &[])];
    let mut analyses = Vec::new();
    for (index, template) in selected.into_iter().enumerate() {
        let id = format!("replan_analysis_{}", index + 1);
        analyses.push(id.clone());
        nodes.push(AdaptiveNode {
            id,
            template_id: template,
            depends_on: vec!["resume".into()],
            required: true,
        });
    }
    nodes.push(AdaptiveNode {
        id: "legacy_alias".into(),
        template_id: "add_legacy_alias".into(),
        depends_on: analyses,
        required: true,
    });
    nodes.push(node(
        "full_matrix",
        "validate_full_matrix",
        &["legacy_alias"],
    ));
    nodes.push(node(
        "boundaries",
        "validate_workspace_boundaries",
        &["full_matrix"],
    ));
    nodes.push(node("reconcile", "reconcile_repositories", &["boundaries"]));
    nodes.push(node("cleanup", "cleanup_workspace", &["reconcile"]));
    nodes
}

fn node(id: &str, template_id: &str, depends_on: &[&str]) -> AdaptiveNode {
    AdaptiveNode {
        id: id.into(),
        template_id: template_id.into(),
        depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
        required: true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteContract {
    pub path: String,
    pub response_fields: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProducerContract {
    pub current_contract_version: u32,
    pub supported_contract_versions: BTreeSet<u32>,
    pub routes: Vec<RouteContract>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerExpectation {
    pub consumer_id: String,
    pub required_contract_version: u32,
    pub route: String,
    pub required_fields: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityResult {
    pub consumer_id: String,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanaryOutcome {
    pub passed: bool,
    pub evidence_id: String,
    pub materialized_repository: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceMarker {
    scenario_id: String,
    fixture_sha256: String,
}

#[derive(Debug)]
pub struct CrossRepoSimulator {
    fixture_root: PathBuf,
    workspace_root: PathBuf,
    initial_commits: BTreeMap<String, String>,
    consumer_b_revealed: bool,
    cleanup_complete: bool,
}

impl CrossRepoSimulator {
    pub fn materialize(fixture_root: &Path, workspace_root: &Path) -> Result<Self> {
        validate_source_fixture(fixture_root)?;
        if workspace_root.exists() {
            if workspace_root.read_dir()?.next().is_some() {
                bail!("cross-repository workspace must be absent or empty");
            }
        } else {
            fs::create_dir_all(workspace_root)?;
        }
        for repository in ["producer", "consumer-a"] {
            copy_source_tree(
                &fixture_root.join(repository),
                &workspace_root.join(repository),
            )?;
        }
        let marker = WorkspaceMarker {
            scenario_id: SCENARIO_ID.into(),
            fixture_sha256: fixture_sha256(fixture_root)?,
        };
        fs::write(
            workspace_root.join(MARKER_FILE),
            serde_json::to_vec_pretty(&marker)?,
        )?;
        let mut initial_commits = BTreeMap::new();
        for repository in ["producer", "consumer-a"] {
            let root = workspace_root.join(repository);
            git_init_and_commit(&root, "fixture: initial state", INITIAL_GIT_DATE)?;
            initial_commits.insert(
                repository.into(),
                git_output(&root, &["rev-parse", "HEAD"])?,
            );
        }
        Ok(Self {
            fixture_root: fixture_root.to_path_buf(),
            workspace_root: workspace_root.to_path_buf(),
            initial_commits,
            consumer_b_revealed: false,
            cleanup_complete: false,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn visible_repositories(&self) -> Vec<String> {
        let mut repositories = vec!["producer".into(), "consumer-a".into()];
        if self.consumer_b_revealed {
            repositories.push("consumer-b".into());
        }
        repositories
    }

    pub fn initial_commit(&self, repository: &str) -> Option<&str> {
        self.initial_commits.get(repository).map(String::as_str)
    }

    pub fn apply_reference_plan_a(&mut self) -> Result<()> {
        self.ensure_active()?;
        write_json(
            &self.workspace_root,
            "producer/contract.json",
            &serde_json::json!({
                "current_contract_version": 2,
                "supported_contract_versions": [1, 2],
                "routes": [
                    {"path": "/v1/profile", "response_fields": ["name"]},
                    {"path": "/v2/profile", "response_fields": ["display_name", "id"]}
                ]
            }),
        )?;
        write_text(
            &self.workspace_root,
            "producer/src/profile_api.py",
            PRODUCER_PLAN_A_SOURCE,
        )?;
        let migration_target: ConsumerExpectation =
            read_json(&self.workspace_root.join("consumer-a/migration_target.json"))?;
        write_json(
            &self.workspace_root,
            "consumer-a/expectation.json",
            &migration_target,
        )?;
        write_text(
            &self.workspace_root,
            "consumer-a/src/client.py",
            CONSUMER_A_PLAN_A_SOURCE,
        )?;
        git_commit_changes(
            &self.workspace_root.join("producer"),
            "feat: add versioned profile contract",
            PLAN_A_GIT_DATE,
        )?;
        git_commit_changes(
            &self.workspace_root.join("consumer-a"),
            "feat: consume profile contract v2",
            PLAN_A_GIT_DATE,
        )?;
        Ok(())
    }

    pub fn validate_visible_matrix(&self) -> Result<Vec<CompatibilityResult>> {
        self.ensure_active()?;
        self.validate_consumers(&["consumer-a"])
    }

    pub fn run_trusted_canary(&mut self) -> Result<CanaryOutcome> {
        self.ensure_active()?;
        if self.consumer_b_revealed {
            bail!("consumer-b canary can be revealed exactly once");
        }
        let target = self.workspace_root.join("consumer-b");
        copy_source_tree(&self.fixture_root.join("hidden/consumer-b"), &target)?;
        git_init_and_commit(&target, "fixture: initial state", INITIAL_GIT_DATE)?;
        self.initial_commits.insert(
            "consumer-b".into(),
            git_output(&target, &["rev-parse", "HEAD"])?,
        );
        self.consumer_b_revealed = true;
        let result = self
            .validate_consumers(&["consumer-b"])?
            .into_iter()
            .next()
            .context("consumer-b canary produced no result")?;
        if result.passed {
            bail!("trusted canary fixture no longer forces a material replanning event");
        }
        Ok(CanaryOutcome {
            passed: false,
            evidence_id: CANARY_EVIDENCE_ID.into(),
            materialized_repository: "consumer-b".into(),
            reason: result.reason,
        })
    }

    pub fn apply_reference_plan_b(&mut self) -> Result<()> {
        self.ensure_active()?;
        if !self.consumer_b_revealed {
            bail!("plan B cannot run before the trusted canary reveals consumer-b");
        }
        write_json(
            &self.workspace_root,
            "producer/contract.json",
            &serde_json::json!({
                "current_contract_version": 2,
                "supported_contract_versions": [1, 2],
                "routes": [
                    {"path": "/profile", "response_fields": ["name"]},
                    {"path": "/v1/profile", "response_fields": ["name"]},
                    {"path": "/v2/profile", "response_fields": ["display_name", "id"]}
                ]
            }),
        )?;
        write_text(
            &self.workspace_root,
            "producer/src/profile_api.py",
            PRODUCER_PLAN_B_SOURCE,
        )?;
        git_commit_changes(
            &self.workspace_root.join("producer"),
            "fix: preserve legacy profile alias",
            PLAN_B_GIT_DATE,
        )?;
        Ok(())
    }

    pub fn validate_full_matrix(&self) -> Result<Vec<CompatibilityResult>> {
        self.ensure_active()?;
        if !self.consumer_b_revealed {
            bail!("full compatibility matrix requires the revealed consumer-b");
        }
        self.validate_consumers(&["consumer-a", "consumer-b"])
    }

    pub fn validate_boundaries(&self) -> Result<WorkspaceBoundaryGates> {
        self.ensure_active()?;
        let allowed = BTreeSet::from([
            "producer/contract.json".to_string(),
            "producer/src/profile_api.py".to_string(),
            "consumer-a/expectation.json".to_string(),
            "consumer-a/src/client.py".to_string(),
        ]);
        let mut changed = BTreeSet::new();
        let mut git_clean = true;
        for repository in self.visible_repositories() {
            let root = self.workspace_root.join(&repository);
            git_clean &= git_output(&root, &["status", "--porcelain"])?.is_empty();
            let initial = self
                .initial_commits
                .get(&repository)
                .context("visible repository has no initial commit provenance")?;
            for path in git_output(&root, &["diff", "--name-only", initial, "HEAD"])?.lines() {
                if !path.trim().is_empty() {
                    changed.insert(format!("{repository}/{}", path.trim()));
                }
            }
        }
        Ok(WorkspaceBoundaryGates {
            allowed_paths_only: changed.iter().all(|path| allowed.contains(path)),
            git_clean,
            deterministic_provenance: self.validate_provenance()?,
            network_disabled: template_catalog()
                .iter()
                .all(|template| !template.network_allowed),
            changed_paths: changed,
        })
    }

    pub fn reject_network_access(&self, target: &str) -> Result<()> {
        self.ensure_active()?;
        bail!("network access is disabled for cross-repository fixture target '{target}'")
    }

    pub fn cleanup(&mut self) -> Result<()> {
        self.ensure_active()?;
        let marker_path = self.workspace_root.join(MARKER_FILE);
        let marker: WorkspaceMarker = read_json(&marker_path)
            .context("refusing cleanup without the owned workspace marker")?;
        if marker.scenario_id != SCENARIO_ID
            || marker.fixture_sha256 != fixture_sha256(&self.fixture_root)?
        {
            bail!("refusing cleanup of a workspace with a mismatched ownership marker");
        }
        fs::remove_dir_all(&self.workspace_root).with_context(|| {
            format!(
                "remove owned fixture workspace {}",
                self.workspace_root.display()
            )
        })?;
        self.cleanup_complete = true;
        Ok(())
    }

    pub fn cleanup_complete(&self) -> bool {
        self.cleanup_complete && !self.workspace_root.exists()
    }

    fn validate_consumers(&self, consumers: &[&str]) -> Result<Vec<CompatibilityResult>> {
        let producer: ProducerContract =
            read_json(&self.workspace_root.join("producer/contract.json"))?;
        consumers
            .iter()
            .map(|consumer| {
                let expectation: ConsumerExpectation =
                    read_json(&self.workspace_root.join(consumer).join("expectation.json"))?;
                Ok(check_compatibility(&producer, &expectation))
            })
            .collect()
    }

    fn validate_provenance(&self) -> Result<bool> {
        for repository in self.visible_repositories() {
            let root = self.workspace_root.join(repository);
            let author = git_output(&root, &["log", "--format=%an <%ae>"])?;
            if author
                .lines()
                .any(|line| line != format!("{FIXED_GIT_NAME} <{FIXED_GIT_EMAIL}>"))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn ensure_active(&self) -> Result<()> {
        if self.cleanup_complete || !self.workspace_root.exists() {
            bail!("cross-repository fixture workspace has already been cleaned up");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBoundaryGates {
    pub allowed_paths_only: bool,
    pub git_clean: bool,
    pub deterministic_provenance: bool,
    pub network_disabled: bool,
    pub changed_paths: BTreeSet<String>,
}

impl WorkspaceBoundaryGates {
    pub fn passed(&self) -> bool {
        self.allowed_paths_only
            && self.git_clean
            && self.deterministic_provenance
            && self.network_disabled
    }
}

pub fn check_compatibility(
    producer: &ProducerContract,
    consumer: &ConsumerExpectation,
) -> CompatibilityResult {
    if !producer
        .supported_contract_versions
        .contains(&consumer.required_contract_version)
    {
        return compatibility_failure(
            consumer,
            format!(
                "producer does not support contract v{}",
                consumer.required_contract_version
            ),
        );
    }
    let Some(route) = producer
        .routes
        .iter()
        .find(|route| route.path == consumer.route)
    else {
        return compatibility_failure(
            consumer,
            format!("producer is missing required route '{}'", consumer.route),
        );
    };
    let missing = consumer
        .required_fields
        .difference(&route.response_fields)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return compatibility_failure(
            consumer,
            format!(
                "route '{}' is missing fields: {}",
                consumer.route,
                missing.join(", ")
            ),
        );
    }
    CompatibilityResult {
        consumer_id: consumer.consumer_id.clone(),
        passed: true,
        reason: "version, route, and response fields are compatible".into(),
    }
}

fn compatibility_failure(consumer: &ConsumerExpectation, reason: String) -> CompatibilityResult {
    CompatibilityResult {
        consumer_id: consumer.consumer_id.clone(),
        passed: false,
        reason,
    }
}

fn validate_source_fixture(root: &Path) -> Result<()> {
    for relative in [
        "producer/contract.json",
        "producer/src/profile_api.py",
        "consumer-a/expectation.json",
        "consumer-a/migration_target.json",
        "consumer-a/src/client.py",
        "hidden/consumer-b/expectation.json",
        "hidden/consumer-b/src/client.py",
    ] {
        if !root.join(relative).is_file() {
            bail!("cross-repository source fixture is missing '{relative}'");
        }
    }
    if contains_git_metadata(root)? {
        bail!("cross-repository fixtures must be source-only and cannot contain .git metadata");
    }
    Ok(())
}

fn contains_git_metadata(root: &Path) -> Result<bool> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            return Ok(true);
        }
        if entry.file_type()?.is_dir() && contains_git_metadata(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn copy_source_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("read source fixture directory {}", source.display()))?
    {
        let entry = entry?;
        if entry.file_name() == ".git" {
            bail!("source fixture unexpectedly contains .git metadata");
        }
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_source_tree(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            bail!("source fixture cannot contain symlinks or special files");
        }
    }
    Ok(())
}

fn write_json(root: &Path, relative: &str, value: &impl Serialize) -> Result<()> {
    validate_relative_path(relative)?;
    fs::write(root.join(relative), serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn write_text(root: &Path, relative: &str, value: &str) -> Result<()> {
    validate_relative_path(relative)?;
    fs::write(root.join(relative), value)?;
    Ok(())
}

fn validate_relative_path(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("fixture mutation path '{relative}' is outside the owned workspace");
    }
    let allowed = [
        "producer/contract.json",
        "producer/src/profile_api.py",
        "consumer-a/expectation.json",
        "consumer-a/src/client.py",
    ];
    if !allowed.contains(&relative) {
        bail!("fixture mutation path '{relative}' is not allowlisted");
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read fixture JSON {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse fixture JSON {}", path.display()))
}

fn git_init_and_commit(root: &Path, message: &str, date: &str) -> Result<()> {
    git_run(root, &["init", "-q", "--initial-branch=main"], None)?;
    git_run(root, &["add", "."], None)?;
    git_run(root, &["commit", "-q", "-m", message], Some(date))?;
    Ok(())
}

fn git_commit_changes(root: &Path, message: &str, date: &str) -> Result<()> {
    git_run(root, &["add", "."], None)?;
    git_run(root, &["commit", "-q", "-m", message], Some(date))?;
    Ok(())
}

fn git_run(root: &Path, args: &[&str], date: Option<&str>) -> Result<()> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", FIXED_GIT_NAME)
        .env("GIT_AUTHOR_EMAIL", FIXED_GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", FIXED_GIT_NAME)
        .env("GIT_COMMITTER_EMAIL", FIXED_GIT_EMAIL);
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command
        .output()
        .with_context(|| format!("execute git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .with_context(|| format!("execute git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn fixture_sha256(root: &Path) -> Result<String> {
    let mut paths = Vec::new();
    collect_source_files(root, root, &mut paths)?;
    paths.sort();
    let mut hasher = Sha256::new();
    for relative in paths {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(root.join(&relative))?);
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_source_files(root: &Path, current: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_source_files(root, &entry.path(), paths)?;
        } else if entry.file_type()?.is_file() {
            paths.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn canonical_sha256(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

const PRODUCER_PLAN_A_SOURCE: &str = r#"def get_profile_v1(user_id: str) -> dict:
    return {"name": f"user-{user_id}"}


def get_profile_v2(user_id: str) -> dict:
    return {"id": user_id, "display_name": f"user-{user_id}"}
"#;

const PRODUCER_PLAN_B_SOURCE: &str = r#"def get_profile_legacy_alias(user_id: str) -> dict:
    return {"name": f"user-{user_id}"}


def get_profile_v1(user_id: str) -> dict:
    return {"name": f"user-{user_id}"}


def get_profile_v2(user_id: str) -> dict:
    return {"id": user_id, "display_name": f"user-{user_id}"}
"#;

const CONSUMER_A_PLAN_A_SOURCE: &str = r#"PROFILE_ROUTE = "/v2/profile"
REQUIRED_FIELDS = {"id", "display_name"}
"#;
