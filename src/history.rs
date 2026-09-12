//! Historical observations are retained data, never local execution receipts.
use std::collections::BTreeSet;

use anyhow::{ensure, Context, Result};
use chrono::DateTime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact;

pub(crate) mod evidence;
pub(crate) mod projection;
pub(crate) mod store;

pub const SCHEMA: &str = "harness-e2e-history/v1";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryImport {
    pub json: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub schema: String,
    pub source: Source,
    pub captured_at: String,
    pub scope: Scope,
    pub counts: Counts,
    pub plan: Plan,
    pub campaigns: Vec<Value>,
    pub executions: Vec<Execution>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub instance_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub plan_key: String,
    pub complete: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub campaigns: usize,
    pub executions: usize,
    pub reports: usize,
    pub runs: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub key: String,
    pub configuration: Option<Value>,
    pub active: bool,
    pub source_updated_at: String,
    pub content_sha256: String,
    pub limitation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub record: Value,
    pub source_updated_at: String,
    pub content_sha256: String,
    pub materialization: Materialization,
    pub reports: Vec<Value>,
    pub runs: Vec<Value>,
    pub bundles: Vec<GithubBundle>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Materialization {
    pub availability: MaterializationAvailability,
    pub snapshot: Option<Value>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationAvailability {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GithubBundle {
    Github {
        repository: String,
        run_id: u64,
        run_attempt: u32,
        artifact_id: Option<u64>,
        artifact_name: Option<String>,
        sha256: Option<String>,
        size_bytes: Option<u64>,
    },
}

impl HistoryImport {
    pub fn decode(&self) -> Result<History> {
        ensure!(
            artifact::sha256_bytes(self.json.as_bytes()) == self.sha256,
            "History transport checksum does not match its UTF-8 content"
        );
        let history: History = serde_json::from_str(&self.json).context("decode history export")?;
        history.validate()?;
        Ok(history)
    }
}

impl History {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == SCHEMA, "Unsupported history schema");
        identity(&self.source.instance_id)?;
        identity(&self.plan.key)?;
        timestamp(&self.captured_at)?;
        timestamp(&self.plan.source_updated_at)?;
        digest(&self.plan.content_sha256)?;
        ensure!(
            self.scope.complete && self.scope.plan_key == self.plan.key,
            "Expected a complete export of one identified plan"
        );
        ensure!(
            self.plan
                .configuration
                .as_ref()
                .is_some_and(Value::is_object)
                || (!self.plan.active
                    && self
                        .plan
                        .limitation
                        .as_ref()
                        .is_some_and(|v| !v.trim().is_empty())),
            "Missing plan configuration must have an explicit historical limitation"
        );
        ensure!(
            self.counts.campaigns == self.campaigns.len()
                && self.counts.executions == self.executions.len()
                && self.counts.reports
                    == self
                        .executions
                        .iter()
                        .map(|e| e.reports.len())
                        .sum::<usize>()
                && self.counts.runs == self.executions.iter().map(|e| e.runs.len()).sum::<usize>(),
            "History counts do not match the exported records"
        );
        let mut campaigns = BTreeSet::new();
        for campaign in &self.campaigns {
            let id = field(campaign, "id")?;
            ensure!(campaigns.insert(id), "Duplicate campaign identity {id}");
            ensure!(
                field(campaign, "planKey")? == self.plan.key,
                "Campaign belongs to another plan"
            );
        }
        let mut executions = BTreeSet::new();
        let mut reports = BTreeSet::new();
        let mut runs = BTreeSet::new();
        for execution in &self.executions {
            let id = field(&execution.record, "id")?;
            ensure!(executions.insert(id), "Duplicate execution identity {id}");
            ensure!(
                field(&execution.record, "planKey")? == self.plan.key,
                "Execution belongs to another plan"
            );
            ensure!(
                campaigns.contains(field(&execution.record, "campaignId")?),
                "Execution references a missing campaign"
            );
            ensure!(
                execution.record["attempt"].as_u64().is_some_and(|v| v > 0),
                "Execution relaunch attempt must be positive"
            );
            ensure!(
                execution.record["terminal"].is_boolean(),
                "Execution terminal state is missing"
            );
            field(&execution.record, "phase")?;
            timestamp(field(&execution.record, "requestedAt")?)?;
            timestamp(&execution.source_updated_at)?;
            digest(&execution.content_sha256)?;
            match execution.materialization.availability {
                MaterializationAvailability::Complete => {
                    let snapshot: crate::test_plan::ProfileSnapshot = serde_json::from_value(
                        execution
                            .materialization
                            .snapshot
                            .clone()
                            .context("Complete materialization has no snapshot")?,
                    )
                    .context("decode complete profile snapshot")?;
                    ensure!(
                        snapshot.schema == "harness-e2e-profile-snapshot/v1",
                        "Unsupported profile snapshot schema"
                    );
                }
                MaterializationAvailability::Partial | MaterializationAvailability::Unavailable => {
                    ensure!(
                        execution
                            .materialization
                            .reason
                            .as_ref()
                            .is_some_and(|r| !r.trim().is_empty()),
                        "Incomplete materialization must explain the retained gap"
                    );
                    if execution.materialization.availability
                        == MaterializationAvailability::Unavailable
                    {
                        ensure!(
                            execution.materialization.snapshot.is_none(),
                            "Unavailable materialization cannot contain a snapshot"
                        );
                    }
                }
            }
            let mut execution_reports = BTreeSet::new();
            for report in &execution.reports {
                let report_id = field(report, "id")?;
                ensure!(
                    reports.insert(report_id),
                    "Duplicate report identity {report_id}"
                );
                execution_reports.insert(report_id);
                ensure!(
                    field(report, "executionId")? == id,
                    "Report belongs to another execution"
                );
                ensure!(
                    report["runAttempt"].as_u64().is_some_and(|v| v > 0),
                    "Report GitHub attempt must be positive"
                );
                ensure!(
                    matches!(field(report, "kind")?, "materialized" | "shard" | "summary"),
                    "Unknown report kind"
                );
                ensure!(
                    report["payload"].is_object(),
                    "Report payload must be retained"
                );
                digest(field(report, "payloadSha256")?)?;
                timestamp(field(report, "receivedAt")?)?;
            }
            for run in &execution.runs {
                let run_id = field(run, "id")?;
                ensure!(
                    runs.insert(run_id),
                    "Duplicate selected run identity {run_id}"
                );
                ensure!(
                    field(run, "executionId")? == id,
                    "Run belongs to another execution"
                );
                ensure!(
                    field(run, "campaignId")? == field(&execution.record, "campaignId")?,
                    "Run belongs to another campaign"
                );
                ensure!(
                    execution_reports.contains(field(run, "reportId")?),
                    "Run references a missing report"
                );
                field(run, "scenarioId")?;
                timestamp(field(run, "capturedAt")?)?;
                ensure!(
                    run["record"].is_object(),
                    "Original run record must be retained"
                );
                digest(field(run, "recordSha256")?)?;
            }
            for bundle in &execution.bundles {
                let GithubBundle::Github {
                    repository,
                    run_id,
                    run_attempt,
                    artifact_id,
                    artifact_name,
                    sha256,
                    ..
                } = bundle;
                let parts: Vec<_> = repository.split('/').collect();
                ensure!(
                    parts.len() == 2
                        && parts.iter().all(|p| !p.is_empty()
                            && *p != "."
                            && *p != ".."
                            && p.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))),
                    "Invalid GitHub repository"
                );
                ensure!(
                    *run_id > 0 && *run_attempt > 0 && artifact_id.is_none_or(|id| id > 0),
                    "Invalid GitHub bundle identity"
                );
                if let Some(name) = artifact_name {
                    identity(name)?;
                }
                if let Some(hash) = sha256 {
                    digest(hash.strip_prefix("sha256:").unwrap_or(hash))?;
                }
            }
        }
        Ok(())
    }

    pub fn local_plan_id(&self) -> String {
        local_id("plan", &self.source.instance_id, &self.plan.key)
    }
}

pub fn local_id(kind: &str, instance: &str, id: &str) -> String {
    let hash = artifact::sha256_bytes(serde_json::json!([instance, id]).to_string().as_bytes());
    format!("remote-{kind}-{}", &hash[7..])
}

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let value = value[key]
        .as_str()
        .with_context(|| format!("History record lacks {key}"))?;
    identity(value)?;
    Ok(value)
}

fn identity(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control),
        "Invalid history identity"
    );
    Ok(())
}

pub(crate) fn timestamp(value: &str) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(value).context("Invalid history timestamp")
}

fn digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid retained SHA-256 digest"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires isolated HARNESS_E2E_TEST_DATABASE_URL and HISTORY_EXPORT_ARTIFACT produced by the RC PostgreSQL integration test"]
    async fn real_database_imports_rc_postgres_export() {
        let transport: HistoryImport = serde_json::from_slice(
            &std::fs::read(std::env::var("HISTORY_EXPORT_ARTIFACT").unwrap()).unwrap(),
        )
        .unwrap();
        let history = transport.decode().unwrap();
        assert!(history.counts.executions > 50);
        let url = std::env::var("HARNESS_E2E_TEST_DATABASE_URL").unwrap();
        let client = iii_sdk::register_worker(&url, iii_sdk::InitOptions::default());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let db = crate::persistence::Persistence::new(
            client.clone(),
            "harness_e2e".into(),
            "default".into(),
        );
        db.initialize().await.unwrap();
        db.import_history(transport.clone()).await.unwrap();
        assert_eq!(
            db.import_history(transport).await.unwrap()["unchanged"],
            history.executions.len() + 1
        );
        for execution in &history.executions {
            let id = local_id(
                "execution",
                &history.source.instance_id,
                field(&execution.record, "id").unwrap(),
            );
            let (_, retained) = db.imported_execution(&id).await.unwrap().unwrap();
            assert_eq!(retained.record, execution.record);
            assert_eq!(retained.reports.len(), execution.reports.len());
            assert_eq!(retained.runs, execution.runs);
            assert_eq!(
                serde_json::to_value(retained.materialization).unwrap(),
                serde_json::to_value(&execution.materialization).unwrap()
            );
        }
        let summaries = db.imported_execution_summaries().await.unwrap();
        assert_eq!(
            summaries
                .iter()
                .filter(|e| e["plan_id"] == history.local_plan_id())
                .count(),
            51
        );
        assert_eq!(history.counts.runs, 1);
        client.shutdown_async().await;
    }

    fn fixture() -> HistoryImport {
        serde_json::from_str(include_str!(
            "../tests/fixtures/history/retained-history.json"
        ))
        .unwrap()
    }

    #[test]
    fn retained_history_keeps_incomplete_materialization_and_all_attempts() {
        let history = fixture().decode().unwrap();
        assert!(history.executions.len() > 50);
        assert_eq!(history.executions[0].reports.len(), 3);
        assert_eq!(
            history.executions[0].materialization.availability,
            MaterializationAvailability::Partial
        );
        assert_eq!(
            history.executions[0].runs[0]["record"]["objective_score"],
            0.125
        );
        assert_eq!(
            history.executions[0].runs[0]["record"]["efficiency"],
            Value::Null
        );
        assert_ne!(
            history.local_plan_id(),
            local_id("plan", "another-rc", &history.plan.key)
        );
    }

    #[test]
    fn malformed_or_truncated_history_never_passes_import_validation() {
        let mut input = fixture();
        input.json.push(' ');
        assert!(input.decode().unwrap_err().to_string().contains("checksum"));
        let original = fixture().decode().unwrap();
        let mut history = original.clone();
        history.executions.pop();
        assert!(history
            .validate()
            .unwrap_err()
            .to_string()
            .contains("counts"));
        let mut history = original.clone();
        history.executions[0].runs[0]["reportId"] = serde_json::json!("missing");
        assert!(history
            .validate()
            .unwrap_err()
            .to_string()
            .contains("missing report"));
        let mut history = original.clone();
        history.executions[0].record["planKey"] = serde_json::json!("other");
        assert!(history
            .validate()
            .unwrap_err()
            .to_string()
            .contains("another plan"));
        let mut history = original;
        history.executions[0].materialization.availability = MaterializationAvailability::Complete;
        assert!(history.validate().is_err());
    }

    #[test]
    fn published_schema_matches_the_worker_contract() {
        let snapshot_schema =
            serde_json::to_value(schemars::schema_for!(crate::test_plan::ProfileSnapshot)).unwrap();
        let snapshot_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schemas/e2e-profile-snapshot-v1.json");
        if std::env::var_os("UPDATE_HISTORY_SCHEMA").is_some() {
            std::fs::write(
                &snapshot_path,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&snapshot_schema).unwrap()
                ),
            )
            .unwrap();
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(snapshot_path).unwrap()).unwrap(),
            snapshot_schema
        );
        let schema = serde_json::to_value(schemars::schema_for!(History)).unwrap();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/e2e-history-v1.json");
        if std::env::var_os("UPDATE_HISTORY_SCHEMA").is_some() {
            std::fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&schema).unwrap()),
            )
            .unwrap();
        }
        let published: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(published, schema);
        let validator = jsonschema::JSONSchema::compile(&published).unwrap();
        assert!(validator.is_valid(&serde_json::from_str::<Value>(&fixture().json).unwrap()));
    }
}
