use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact;
use crate::workflow::{
    harness_descriptor, security_scan, StepCatalog, StepTypeDescriptor, WorkflowCheckpointV1,
    WorkflowDefinitionV1,
};

const DRAFT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowDraft {
    pub schema_version: u32,
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub updated_at: String,
    pub definition_sha256: String,
    pub definition: WorkflowDefinitionV1,
    #[serde(default)]
    pub layout: Value,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowDraftWriteRequest {
    #[serde(default)]
    pub label: String,
    pub definition: WorkflowDefinitionV1,
    #[serde(default)]
    pub layout: Value,
    #[serde(default)]
    pub expected_definition_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowValidateRequest {
    pub definition: WorkflowDefinitionV1,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowExecuteRequest {
    pub expected_definition_sha256: String,
    pub url: String,
    pub model: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct WorkflowCatalogResponse {
    pub mode: String,
    pub drafts: Vec<WorkflowDraft>,
    pub official: Vec<OfficialWorkflow>,
    pub step_types: Vec<StepTypeDescriptor>,
    pub definition_schema: Value,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct OfficialWorkflow {
    pub id: String,
    pub scenario_version: u32,
    pub description: String,
    pub definition_sha256: String,
    pub definition: WorkflowDefinitionV1,
}

pub(super) fn drafts_dir(runs_dir: &Path) -> PathBuf {
    runs_dir.join("workflow-drafts")
}

pub(super) fn validation_catalog() -> Result<StepCatalog> {
    let mut catalog = StepCatalog::new();
    catalog.register_descriptor(harness_descriptor()?)?;
    for descriptor in security_scan::descriptors_only() {
        catalog.register_descriptor(descriptor)?;
    }
    Ok(catalog)
}

pub(super) fn catalog(runs_dir: &Path, view_only: bool) -> Result<WorkflowCatalogResponse> {
    let catalog = validation_catalog()?;
    Ok(WorkflowCatalogResponse {
        mode: if view_only { "observed" } else { "local" }.into(),
        drafts: if view_only {
            Vec::new()
        } else {
            list_drafts(&drafts_dir(runs_dir))?
        },
        official: list_official(&catalog)?,
        step_types: catalog.descriptors(),
        definition_schema: serde_json::to_value(crate::schema::workflow_definition())?,
    })
}

pub(super) fn validate_definition(definition: &WorkflowDefinitionV1) -> Result<String> {
    Ok(definition.validate(&validation_catalog()?)?.sha256)
}

pub(super) fn create_draft(
    runs_dir: &Path,
    request: WorkflowDraftWriteRequest,
) -> Result<WorkflowDraft> {
    let id = format!(
        "workflow-{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S"),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let now = timestamp();
    let hash = request.definition.canonical_sha256()?;
    let draft = WorkflowDraft {
        schema_version: DRAFT_SCHEMA_VERSION,
        id,
        label: validate_label(&request.label, &request.definition.id)?,
        created_at: now.clone(),
        updated_at: now,
        definition_sha256: hash,
        definition: request.definition,
        layout: request.layout,
    };
    write_draft(&drafts_dir(runs_dir), &draft)?;
    Ok(draft)
}

pub(super) fn update_draft(
    runs_dir: &Path,
    id: &str,
    request: WorkflowDraftWriteRequest,
) -> Result<WorkflowDraft> {
    validate_draft_id(id)?;
    let directory = drafts_dir(runs_dir);
    let mut draft =
        read_draft(&directory, id)?.with_context(|| format!("draft '{id}' not found"))?;
    if request
        .expected_definition_sha256
        .as_deref()
        .is_some_and(|expected| expected != draft.definition_sha256)
    {
        bail!("draft definition changed since it was loaded");
    }
    draft.label = validate_label(&request.label, &request.definition.id)?;
    draft.definition_sha256 = request.definition.canonical_sha256()?;
    draft.definition = request.definition;
    draft.layout = request.layout;
    draft.updated_at = timestamp();
    write_draft(&directory, &draft)?;
    Ok(draft)
}

pub(super) fn duplicate_draft(runs_dir: &Path, id: &str) -> Result<WorkflowDraft> {
    let original = read_draft(&drafts_dir(runs_dir), id)?
        .with_context(|| format!("draft '{id}' not found"))?;
    create_draft(
        runs_dir,
        WorkflowDraftWriteRequest {
            label: format!("{} copy", original.label),
            definition: original.definition,
            layout: original.layout,
            expected_definition_sha256: None,
        },
    )
}

pub(super) fn delete_draft(runs_dir: &Path, id: &str) -> Result<()> {
    validate_draft_id(id)?;
    let path = drafts_dir(runs_dir).join(format!("{id}.json"));
    if path.is_file() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn get_draft(runs_dir: &Path, id: &str) -> Result<WorkflowDraft> {
    read_draft(&drafts_dir(runs_dir), id)?.with_context(|| format!("draft '{id}' not found"))
}

pub(super) fn latest_checkpoint(output_dir: &Path) -> Result<Option<WorkflowCheckpointV1>> {
    let root = output_dir.join("checkpoints");
    if !root.is_dir() {
        return Ok(None);
    }
    let mut latest: Option<WorkflowCheckpointV1> = None;
    for run in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let run = run?.path();
        if !run.is_dir() {
            continue;
        }
        for attempt in fs::read_dir(&run).with_context(|| format!("read {}", run.display()))? {
            let path = attempt?.path().join("workflow-checkpoint.json");
            if !path.is_file() {
                continue;
            }
            let checkpoint: WorkflowCheckpointV1 = serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("decode {}", path.display()))?;
            if latest
                .as_ref()
                .is_none_or(|current| checkpoint.updated_at > current.updated_at)
            {
                latest = Some(checkpoint);
            }
        }
    }
    Ok(latest)
}

fn write_draft(directory: &Path, draft: &WorkflowDraft) -> Result<()> {
    fs::create_dir_all(directory)?;
    let path = directory.join(format!("{}.json", draft.id));
    let mut bytes = serde_json::to_vec_pretty(draft)?;
    bytes.push(b'\n');
    artifact::write_atomic(&path, &bytes)
}

fn read_draft(directory: &Path, id: &str) -> Result<Option<WorkflowDraft>> {
    validate_draft_id(id)?;
    let path = directory.join(format!("{id}.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let draft: WorkflowDraft = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("decode {}", path.display()))?;
    if draft.schema_version != DRAFT_SCHEMA_VERSION || draft.id != id {
        bail!("draft '{id}' identity or schema version is invalid");
    }
    let observed = draft.definition.canonical_sha256()?;
    if observed != draft.definition_sha256 {
        bail!("draft '{id}' definition hash is stale");
    }
    Ok(Some(draft))
}

fn list_drafts(directory: &Path) -> Result<Vec<WorkflowDraft>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut drafts = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|id| id.to_str())
            .unwrap_or_default();
        if let Some(draft) = read_draft(directory, id)? {
            drafts.push(draft);
        }
    }
    drafts.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(drafts)
}

fn list_official(catalog: &StepCatalog) -> Result<Vec<OfficialWorkflow>> {
    let directory = Path::new("config/workflows");
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut workflows = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let definition: WorkflowDefinitionV1 = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("decode official workflow {}", path.display()))?;
        let materialized = definition.validate(catalog)?;
        workflows.push(OfficialWorkflow {
            id: definition.id.clone(),
            scenario_version: definition.scenario_version,
            description: definition.description.clone(),
            definition_sha256: materialized.sha256,
            definition,
        });
    }
    workflows.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(workflows)
}

fn validate_draft_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("workflow draft id is invalid");
    }
    Ok(())
}

fn validate_label(label: &str, fallback: &str) -> Result<String> {
    let label = if label.trim().is_empty() {
        fallback.trim()
    } else {
        label.trim()
    };
    if label.is_empty() || label.len() > 120 || label.chars().any(char::is_control) {
        bail!("workflow draft label is invalid");
    }
    Ok(label.to_string())
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_does_not_change_definition_hash_and_drafts_are_atomic() {
        let output = tempfile::tempdir().unwrap();
        let definition: WorkflowDefinitionV1 = serde_json::from_str(include_str!(
            "../../config/workflows/security-scan.full.json"
        ))
        .unwrap();
        let first = create_draft(
            output.path(),
            WorkflowDraftWriteRequest {
                label: "security".into(),
                definition: definition.clone(),
                layout: serde_json::json!({"preflight": {"x": 0, "y": 0}}),
                expected_definition_sha256: None,
            },
        )
        .unwrap();
        let updated = update_draft(
            output.path(),
            &first.id,
            WorkflowDraftWriteRequest {
                label: "security".into(),
                definition,
                layout: serde_json::json!({"preflight": {"x": 200, "y": 80}}),
                expected_definition_sha256: Some(first.definition_sha256.clone()),
            },
        )
        .unwrap();
        assert_eq!(first.definition_sha256, updated.definition_sha256);
        assert!(!drafts_dir(output.path())
            .join(format!("{}.json.tmp", first.id))
            .exists());
    }

    #[test]
    fn latest_checkpoint_exposes_live_node_state() {
        let output = tempfile::tempdir().unwrap();
        let checkpoint = WorkflowCheckpointV1 {
            schema_version: 1,
            workflow_id: "workflow.test".into(),
            workflow_sha256: format!("sha256:{}", "a".repeat(64)),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            updated_at: "2026-08-17T12:00:00.000Z".into(),
            terminal_nodes: Vec::new(),
            active_nodes: vec!["active".into()],
            steps: Vec::new(),
        };
        crate::workflow::CheckpointStore::new(output.path(), "run", "attempt")
            .persist(&checkpoint)
            .unwrap();

        let observed = latest_checkpoint(output.path()).unwrap().unwrap();
        assert_eq!(observed.active_nodes, vec!["active"]);
        assert_eq!(observed.workflow_id, "workflow.test");
    }
}
