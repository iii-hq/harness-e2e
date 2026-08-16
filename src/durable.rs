use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::artifact;
use crate::identity::StackIdentity;
use crate::redaction::{RedactionPolicy, RedactionReport};
use crate::report::E2eReport;

pub const ARCHIVE_ID: &str = "e2e::archive";
pub const ARCHIVE_HEAD_ID: &str = "e2e::archive-head";
pub const ARCHIVE_RESTORE_ID: &str = "e2e::archive-restore";
pub const RETENTION_SWEEP_ID: &str = "e2e::retention-sweep";
pub const HISTORY_LIST_ID: &str = "e2e::history-list";
const STORAGE_PUT: &str = "storage::putObject";
const STORAGE_GET: &str = "storage::getObject";
const STORAGE_HEAD: &str = "storage::headObject";
const STORAGE_DELETE: &str = "storage::deleteObject";
const DATABASE_EXECUTE: &str = "database::execute";
const DATABASE_QUERY: &str = "database::query";
const CHUNK_BYTES: usize = 6 * 1024 * 1024;
const HISTORY_TABLE: &str = "harness_e2e_history";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    Temporary,
    PullRequest,
    Longitudinal,
    Canonical,
}

impl RetentionClass {
    fn environment_suffix(self) -> &'static str {
        match self {
            Self::Temporary => "TEMPORARY",
            Self::PullRequest => "PULL_REQUEST",
            Self::Longitudinal => "LONGITUDINAL",
            Self::Canonical => "CANONICAL",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Temporary => "temporary",
            Self::PullRequest => "pull-request",
            Self::Longitudinal => "longitudinal",
            Self::Canonical => "canonical",
        }
    }

    fn lifetime(self) -> Option<Duration> {
        match self {
            Self::Temporary => Some(Duration::days(1)),
            Self::PullRequest => Some(Duration::days(14)),
            Self::Longitudinal => Some(Duration::days(400)),
            Self::Canonical => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DurableConfig {
    pub database: String,
    pub backup_bucket: String,
    pub timeout_ms: u64,
}

impl DurableConfig {
    pub fn from_environment() -> Self {
        Self {
            database: environment("HARNESS_E2E_HISTORY_DATABASE", "primary"),
            backup_bucket: environment("HARNESS_E2E_STORAGE_BACKUP_BUCKET", "e2e-canonical"),
            timeout_ms: std::env::var("HARNESS_E2E_DURABLE_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|timeout| *timeout >= 1_000)
                .unwrap_or(120_000),
        }
    }

    fn bucket(&self, retention: RetentionClass) -> String {
        environment(
            &format!(
                "HARNESS_E2E_STORAGE_{}_BUCKET",
                retention.environment_suffix()
            ),
            &format!("e2e-{}", retention.key()),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StorageObjectReference {
    pub uri: String,
    pub bucket: String,
    pub key: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DurableChunk {
    pub index: u32,
    pub object: StorageObjectReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DurableObject {
    pub id: String,
    pub relative_path: String,
    pub media_type: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub chunks: Vec<DurableChunk>,
    pub redaction: RedactionReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DurableArchiveManifest {
    pub archive_id: String,
    pub execution_id: String,
    pub identity_sha256: String,
    pub retention_class: RetentionClass,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub objects: Vec<DurableObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DurableArchiveReference {
    pub archive_id: String,
    pub execution_id: String,
    pub identity_sha256: String,
    pub retention_class: RetentionClass,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub manifest: StorageObjectReference,
    pub manifest_backup: StorageObjectReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryRecord {
    pub ingestion_id: String,
    pub identity_sha256: String,
    pub execution_id: String,
    pub lane: String,
    pub occurred_at: String,
    pub subject_provider: String,
    pub subject_model: String,
    pub passed: bool,
    pub case_count: u32,
    pub stack_mode: String,
    pub subject_revision: String,
    pub e2e_repository: String,
    pub e2e_revision: String,
    pub archive: DurableArchiveReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveResponse {
    pub archive: DurableArchiveReference,
    pub history: HistoryRecord,
    pub duplicate_ingestion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveAvailability {
    Available,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveHeadResponse {
    pub archive: DurableArchiveReference,
    pub availability: ArchiveAvailability,
    pub verified_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveRestoreResponse {
    pub archive: DurableArchiveReference,
    pub availability: ArchiveAvailability,
    pub restored_files: u32,
    pub restored_root: Option<String>,
    pub restored_from_backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoryListRequest {
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default = "default_history_limit")]
    pub limit: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoryListResponse {
    pub records: Vec<HistoryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetentionSweepRequest {
    pub before: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_sweep_limit")]
    pub limit: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetentionSweepResponse {
    pub inspected: u32,
    pub deleted_archives: Vec<String>,
    pub dry_run: bool,
}

#[async_trait]
pub trait FunctionCaller: Send + Sync {
    async fn call(&self, function_id: &str, payload: Value) -> Result<Value>;
}

#[derive(Clone)]
struct IiiCaller {
    client: IIIClient,
    timeout_ms: u64,
}

#[async_trait]
impl FunctionCaller for IiiCaller {
    async fn call(&self, function_id: &str, payload: Value) -> Result<Value> {
        self.client
            .trigger(TriggerRequest {
                function_id: function_id.into(),
                payload,
                action: None,
                timeout_ms: Some(self.timeout_ms),
            })
            .await
            .map_err(|error| anyhow::anyhow!("{function_id}: {error}"))
    }
}

#[derive(Clone)]
pub struct DurableHistory {
    caller: Arc<dyn FunctionCaller>,
    config: DurableConfig,
    redaction: RedactionPolicy,
}

impl DurableHistory {
    pub fn from_client(client: IIIClient) -> Self {
        let config = DurableConfig::from_environment();
        Self {
            caller: Arc::new(IiiCaller {
                client,
                timeout_ms: config.timeout_ms,
            }),
            config,
            redaction: RedactionPolicy::from_environment(),
        }
    }

    #[cfg(test)]
    fn with_caller(caller: Arc<dyn FunctionCaller>, config: DurableConfig) -> Self {
        Self {
            caller,
            config,
            redaction: RedactionPolicy::default(),
        }
    }

    pub async fn archive(
        &self,
        output: &Path,
        report: &E2eReport,
        retention_class: RetentionClass,
    ) -> Result<ArchiveResponse> {
        let execution = &report.execution;
        let system = &report.system_under_test;
        system.validate()?;
        let identity_sha256 = artifact::sha256_value(system)?;
        let archive_id = archive_id(report, retention_class)?;
        let created_at = execution.completed_at.clone();
        let completed_at = DateTime::parse_from_rfc3339(&created_at)
            .context("execution completed_at must be RFC 3339")?
            .with_timezone(&Utc);
        let expires_at = retention_class
            .lifetime()
            .map(|duration| (completed_at + duration).to_rfc3339_opts(SecondsFormat::Millis, true));
        let bucket = self.config.bucket(retention_class);
        let prefix = format!(
            "e2e/{}/{}/{}",
            retention_class.key(),
            digest_component(&identity_sha256),
            archive_id
        );
        let mut objects = Vec::new();
        for relative_path in list_files(output)? {
            let source = output.join(&relative_path);
            let bytes = std::fs::read(&source)
                .with_context(|| format!("read archive input {}", source.display()))?;
            let media_type = mime_guess::from_path(&relative_path)
                .first_raw()
                .unwrap_or("application/octet-stream")
                .to_string();
            let (sanitized, redaction) = self.redaction.sanitize_bytes(&media_type, &bytes)?;
            if redaction.changed() {
                bail!(
                    "artifact {} still required redaction after evidence hashes were finalized",
                    relative_path.display()
                );
            }
            if sanitized != bytes {
                bail!(
                    "artifact sanitizer changed clean bytes for {}",
                    relative_path.display()
                );
            }
            let sha256 = sha256_bytes(&bytes);
            let id = digest_component(&sha256);
            let mut chunks = Vec::new();
            let byte_chunks = if bytes.is_empty() {
                vec![bytes.as_slice()]
            } else {
                bytes.chunks(CHUNK_BYTES).collect()
            };
            for (index, chunk) in byte_chunks.into_iter().enumerate() {
                let key = format!(
                    "{prefix}/objects/{id}/chunk-{index:06}-{}",
                    digest_component(&sha256_bytes(chunk))
                );
                let object = self
                    .put_object(
                        &bucket,
                        &key,
                        chunk,
                        "application/octet-stream",
                        json!({
                            "archive_id": archive_id,
                            "artifact_sha256": sha256,
                            "chunk_index": index.to_string(),
                            "retention_class": retention_class.key(),
                        }),
                    )
                    .await?;
                chunks.push(DurableChunk {
                    index: u32::try_from(index).context("too many artifact chunks")?,
                    object,
                });
            }
            objects.push(DurableObject {
                id,
                relative_path: relative_path.to_string_lossy().replace('\\', "/"),
                media_type,
                sha256,
                size_bytes: bytes.len().try_into().unwrap_or(u64::MAX),
                chunks,
                redaction,
            });
        }
        let manifest = DurableArchiveManifest {
            archive_id: archive_id.clone(),
            execution_id: execution.execution_id.clone(),
            identity_sha256: identity_sha256.clone(),
            retention_class,
            created_at: created_at.clone(),
            expires_at: expires_at.clone(),
            objects,
        };
        validate_manifest(&manifest)?;
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        manifest_bytes.push(b'\n');
        self.redaction.assert_clean(&manifest_bytes)?;
        let manifest_sha = sha256_bytes(&manifest_bytes);
        let manifest_key = format!("{prefix}/manifest-{}.json", digest_component(&manifest_sha));
        let manifest_object = self
            .put_object(
                &bucket,
                &manifest_key,
                &manifest_bytes,
                "application/json",
                json!({
                    "archive_id": archive_id,
                    "identity_sha256": identity_sha256,
                    "retention_class": retention_class.key(),
                }),
            )
            .await?;
        let backup_key = format!(
            "e2e-manifest-backups/{}/manifest-{}.json",
            archive_id,
            digest_component(&manifest_sha)
        );
        let manifest_backup = self
            .put_object(
                &self.config.backup_bucket,
                &backup_key,
                &manifest_bytes,
                "application/json",
                json!({"archive_id": archive_id, "primary_bucket": bucket}),
            )
            .await?;
        let archive = DurableArchiveReference {
            archive_id,
            execution_id: execution.execution_id.clone(),
            identity_sha256,
            retention_class,
            created_at,
            expires_at,
            manifest: manifest_object,
            manifest_backup,
        };
        let history = history_record(report, archive.clone())?;
        let duplicate_ingestion = self.ingest(&history).await?;
        Ok(ArchiveResponse {
            archive,
            history,
            duplicate_ingestion,
        })
    }

    pub async fn head(&self, archive: DurableArchiveReference) -> Result<ArchiveHeadResponse> {
        validate_archive_reference(&archive)?;
        if expired(&archive, Utc::now())? {
            return Ok(ArchiveHeadResponse {
                archive,
                availability: ArchiveAvailability::Expired,
                verified_at: now(),
            });
        }
        let value = self
            .caller
            .call(
                STORAGE_HEAD,
                json!({"bucket": archive.manifest.bucket, "key": archive.manifest.key}),
            )
            .await
            .context("head durable archive manifest")?;
        let size = value
            .get("size")
            .and_then(Value::as_u64)
            .context("storage head response has no size")?;
        if size != archive.manifest.size_bytes {
            bail!("durable archive manifest size differs from its immutable reference");
        }
        Ok(ArchiveHeadResponse {
            archive,
            availability: ArchiveAvailability::Available,
            verified_at: now(),
        })
    }

    pub async fn restore(
        &self,
        archive: DurableArchiveReference,
        restore_root: &Path,
    ) -> Result<ArchiveRestoreResponse> {
        validate_archive_reference(&archive)?;
        if expired(&archive, Utc::now())? {
            return Ok(ArchiveRestoreResponse {
                archive,
                availability: ArchiveAvailability::Expired,
                restored_files: 0,
                restored_root: None,
                restored_from_backup: false,
            });
        }
        let (manifest_bytes, restored_from_backup) = match self.get_object(&archive.manifest).await
        {
            Ok(bytes) => (bytes, false),
            Err(primary_error) => match self.get_object(&archive.manifest_backup).await {
                Ok(bytes) => (bytes, true),
                Err(backup_error) => {
                    return Err(primary_error
                        .context(format!("manifest backup also failed: {backup_error:#}")))
                }
            },
        };
        let manifest: DurableArchiveManifest =
            serde_json::from_slice(&manifest_bytes).context("decode durable archive manifest")?;
        validate_manifest(&manifest)?;
        if manifest.archive_id != archive.archive_id
            || manifest.identity_sha256 != archive.identity_sha256
            || manifest.execution_id != archive.execution_id
        {
            bail!("restored manifest identity does not match archive reference");
        }
        let destination = restore_root.join(&archive.archive_id);
        if destination.exists() {
            E2eReport::read_from(&destination)
                .context("validate previously restored E2E result and evidence")?;
            return Ok(ArchiveRestoreResponse {
                archive,
                availability: ArchiveAvailability::Available,
                restored_files: u32::try_from(manifest.objects.len()).unwrap_or(u32::MAX),
                restored_root: Some(destination.to_string_lossy().to_string()),
                restored_from_backup,
            });
        }
        std::fs::create_dir_all(&destination)?;
        let restore_result = async {
            for object in &manifest.objects {
                let relative = safe_path(&object.relative_path)?;
                let mut bytes = Vec::with_capacity(
                    usize::try_from(object.size_bytes)
                        .context("artifact is too large to restore")?,
                );
                for chunk in &object.chunks {
                    bytes.extend(self.get_object(&chunk.object).await?);
                }
                if bytes.len() as u64 != object.size_bytes || sha256_bytes(&bytes) != object.sha256
                {
                    bail!(
                        "restored artifact {} failed hash verification",
                        object.relative_path
                    );
                }
                self.redaction.assert_clean(&bytes)?;
                let path = destination.join(relative);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                artifact::write_atomic(&path, &bytes)?;
            }
            E2eReport::read_from(&destination)
                .context("validate reconstructed E2E result and evidence")?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(error) = restore_result {
            let _ = std::fs::remove_dir_all(&destination);
            return Err(error);
        }
        Ok(ArchiveRestoreResponse {
            archive,
            availability: ArchiveAvailability::Available,
            restored_files: u32::try_from(manifest.objects.len()).unwrap_or(u32::MAX),
            restored_root: Some(destination.to_string_lossy().to_string()),
            restored_from_backup,
        })
    }

    pub async fn history_list(&self, request: HistoryListRequest) -> Result<HistoryListResponse> {
        if request.limit == 0 || request.limit > 500 {
            bail!("history list limit must be between 1 and 500");
        }
        self.ensure_history_table().await?;
        let value = self
            .caller
            .call(
                DATABASE_QUERY,
                json!({
                    "db": self.config.database,
                    "sql": format!("SELECT record_json, record_sha256 FROM {HISTORY_TABLE} WHERE deleted_at IS NULL ORDER BY occurred_at DESC LIMIT ?"),
                    "params": [request.limit],
                }),
            )
            .await?;
        let rows = value
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut records = Vec::new();
        for row in rows {
            let encoded = row
                .get("record_json")
                .and_then(Value::as_str)
                .context("history row is missing record_json")?;
            let expected = row
                .get("record_sha256")
                .and_then(Value::as_str)
                .context("history row is missing record_sha256")?;
            let record: HistoryRecord = serde_json::from_str(encoded)?;
            validate_history_record(&record)?;
            if sha256_bytes(encoded.as_bytes()) != expected {
                bail!(
                    "history record {} failed hash verification",
                    record.ingestion_id
                );
            }
            if request
                .lane
                .as_ref()
                .is_none_or(|lane| lane == &record.lane)
            {
                records.push(record);
            }
        }
        Ok(HistoryListResponse { records })
    }

    pub async fn retention_sweep(
        &self,
        request: RetentionSweepRequest,
    ) -> Result<RetentionSweepResponse> {
        if request.limit == 0 || request.limit > 500 {
            bail!("retention sweep limit must be between 1 and 500");
        }
        let before = DateTime::parse_from_rfc3339(&request.before)
            .context("retention sweep before must be RFC 3339")?
            .with_timezone(&Utc);
        self.ensure_history_table().await?;
        let value = self
            .caller
            .call(
                DATABASE_QUERY,
                json!({
                    "db": self.config.database,
                    "sql": format!("SELECT record_json, record_sha256 FROM {HISTORY_TABLE} WHERE deleted_at IS NULL AND expires_at IS NOT NULL AND expires_at <= ? ORDER BY expires_at LIMIT ?"),
                    "params": [before.to_rfc3339(), request.limit],
                }),
            )
            .await?;
        let rows = value
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut deleted_archives = Vec::new();
        for row in &rows {
            let encoded = row
                .get("record_json")
                .and_then(Value::as_str)
                .context("retention row is missing record_json")?;
            let expected = row
                .get("record_sha256")
                .and_then(Value::as_str)
                .context("retention row is missing record_sha256")?;
            if sha256_bytes(encoded.as_bytes()) != expected {
                bail!("retention row failed hash verification");
            }
            let record: HistoryRecord = serde_json::from_str(encoded)?;
            validate_history_record(&record)?;
            if !request.dry_run {
                self.delete_archive(&record.archive).await?;
                self.caller
                    .call(
                        DATABASE_EXECUTE,
                        json!({
                            "db": self.config.database,
                            "sql": format!("UPDATE {HISTORY_TABLE} SET deleted_at = ? WHERE ingestion_id = ? AND deleted_at IS NULL"),
                            "params": [now(), record.ingestion_id],
                        }),
                    )
                    .await?;
            }
            deleted_archives.push(record.archive.archive_id);
        }
        Ok(RetentionSweepResponse {
            inspected: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            deleted_archives,
            dry_run: request.dry_run,
        })
    }

    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        bytes: &[u8],
        media_type: &str,
        metadata: Value,
    ) -> Result<StorageObjectReference> {
        if bytes.len() > CHUNK_BYTES {
            bail!("storage inline chunk exceeds the {CHUNK_BYTES} byte limit");
        }
        let sha256 = sha256_bytes(bytes);
        let metadata = metadata
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let value = self
            .caller
            .call(
                STORAGE_PUT,
                json!({
                    "bucket": bucket,
                    "key": key,
                    "body_base64": BASE64.encode(bytes),
                    "content_type": media_type,
                    "cache_control": "private, no-store",
                    "metadata": metadata,
                }),
            )
            .await
            .with_context(|| format!("put immutable storage object {bucket}/{key}"))?;
        let size = value
            .get("size")
            .and_then(Value::as_u64)
            .context("storage put response has no size")?;
        if size != bytes.len() as u64 {
            bail!("storage put response size differs from uploaded bytes");
        }
        let head = self
            .caller
            .call(STORAGE_HEAD, json!({"bucket": bucket, "key": key}))
            .await?;
        if head.get("size").and_then(Value::as_u64) != Some(size) {
            bail!("storage head did not verify the uploaded object size");
        }
        let reference = StorageObjectReference {
            uri: immutable_uri(bucket, key, &sha256),
            bucket: bucket.into(),
            key: key.into(),
            sha256,
            size_bytes: size,
            media_type: media_type.into(),
        };
        if self.get_object(&reference).await? != bytes {
            bail!("storage read-after-write differs from uploaded bytes");
        }
        Ok(reference)
    }

    async fn get_object(&self, reference: &StorageObjectReference) -> Result<Vec<u8>> {
        validate_storage_reference(reference)?;
        let value = self
            .caller
            .call(
                STORAGE_GET,
                json!({"bucket": reference.bucket, "key": reference.key}),
            )
            .await
            .with_context(|| format!("get immutable object {}", reference.uri))?;
        let body = value
            .get("body_base64")
            .and_then(Value::as_str)
            .context("storage get response has no body_base64")?;
        let bytes = BASE64.decode(body).context("decode storage object body")?;
        if bytes.len() as u64 != reference.size_bytes || sha256_bytes(&bytes) != reference.sha256 {
            bail!(
                "storage object {} failed immutable verification",
                reference.uri
            );
        }
        Ok(bytes)
    }

    async fn delete_archive(&self, archive: &DurableArchiveReference) -> Result<()> {
        let manifest_bytes = match self.get_object(&archive.manifest).await {
            Ok(bytes) => bytes,
            Err(_) => self.get_object(&archive.manifest_backup).await?,
        };
        let manifest: DurableArchiveManifest = serde_json::from_slice(&manifest_bytes)?;
        validate_manifest(&manifest)?;
        for object in manifest.objects {
            for chunk in object.chunks {
                self.delete_object(&chunk.object).await?;
            }
        }
        self.delete_object(&archive.manifest).await?;
        self.delete_object(&archive.manifest_backup).await?;
        Ok(())
    }

    async fn delete_object(&self, reference: &StorageObjectReference) -> Result<()> {
        self.caller
            .call(
                STORAGE_DELETE,
                json!({"bucket": reference.bucket, "key": reference.key}),
            )
            .await?;
        Ok(())
    }

    async fn ingest(&self, record: &HistoryRecord) -> Result<bool> {
        validate_history_record(record)?;
        self.ensure_history_table().await?;
        if let Some(existing) = self.history_get(&record.ingestion_id).await? {
            if serde_json::to_value(&existing)? != serde_json::to_value(record)? {
                bail!(
                    "ingestion id {} already has different content",
                    record.ingestion_id
                );
            }
            return Ok(true);
        }
        let encoded = serde_json::to_string(record)?;
        let digest = sha256_bytes(encoded.as_bytes());
        let insert = self
            .caller
            .call(
                DATABASE_EXECUTE,
                json!({
                    "db": self.config.database,
                    "sql": format!("INSERT INTO {HISTORY_TABLE} (ingestion_id, identity_sha256, execution_id, lane, occurred_at, expires_at, record_json, record_sha256, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)"),
                    "params": [
                        record.ingestion_id,
                        record.identity_sha256,
                        record.execution_id,
                        record.lane,
                        record.occurred_at,
                        record.archive.expires_at,
                        encoded,
                        digest,
                    ],
                }),
            )
            .await;
        if let Err(error) = insert {
            if let Some(existing) = self.history_get(&record.ingestion_id).await? {
                if serde_json::to_value(&existing)? == serde_json::to_value(record)? {
                    return Ok(true);
                }
            }
            return Err(error).context("insert immutable E2E history record");
        }
        Ok(false)
    }

    async fn history_get(&self, ingestion_id: &str) -> Result<Option<HistoryRecord>> {
        let value = self
            .caller
            .call(
                DATABASE_QUERY,
                json!({
                    "db": self.config.database,
                    "sql": format!("SELECT record_json, record_sha256 FROM {HISTORY_TABLE} WHERE ingestion_id = ?"),
                    "params": [ingestion_id],
                }),
            )
            .await?;
        let Some(row) = value
            .get("rows")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
        else {
            return Ok(None);
        };
        let encoded = row
            .get("record_json")
            .and_then(Value::as_str)
            .context("history row has no record_json")?;
        let expected = row
            .get("record_sha256")
            .and_then(Value::as_str)
            .context("history row has no record_sha256")?;
        if sha256_bytes(encoded.as_bytes()) != expected {
            bail!("history record {ingestion_id} failed hash verification");
        }
        let record: HistoryRecord = serde_json::from_str(encoded)?;
        validate_history_record(&record)?;
        Ok(Some(record))
    }

    async fn ensure_history_table(&self) -> Result<()> {
        self.caller
            .call(
                DATABASE_EXECUTE,
                json!({
                    "db": self.config.database,
                    "sql": format!("CREATE TABLE IF NOT EXISTS {HISTORY_TABLE} (ingestion_id TEXT PRIMARY KEY, identity_sha256 TEXT NOT NULL, execution_id TEXT NOT NULL, lane TEXT NOT NULL, occurred_at TEXT NOT NULL, expires_at TEXT NULL, record_json TEXT NOT NULL, record_sha256 TEXT NOT NULL, deleted_at TEXT NULL)"),
                    "params": [],
                }),
            )
            .await
            .context("initialize E2E history table")?;
        Ok(())
    }
}

fn archive_id(report: &E2eReport, retention: RetentionClass) -> Result<String> {
    let execution = &report.execution;
    let system = &report.system_under_test;
    let digest = artifact::sha256_value(&json!({
        "execution": execution,
        "system": system,
        "retention": retention,
    }))?;
    Ok(digest_component(&digest)[..32].to_string())
}

fn history_record(report: &E2eReport, archive: DurableArchiveReference) -> Result<HistoryRecord> {
    let execution = &report.execution;
    let system = &report.system_under_test;
    let (stack_mode, subject_revision) = match &system.stack {
        StackIdentity::Source {
            workers_revision, ..
        } => ("source".to_string(), workers_revision.clone()),
        StackIdentity::Registry {
            stack_lock_digest, ..
        } => ("registry".to_string(), stack_lock_digest.clone()),
    };
    let record = HistoryRecord {
        ingestion_id: archive.archive_id.clone(),
        identity_sha256: archive.identity_sha256.clone(),
        execution_id: execution.execution_id.clone(),
        lane: execution.lane.clone(),
        occurred_at: execution.completed_at.clone(),
        subject_provider: report.subject.provider.clone(),
        subject_model: report.subject.model.clone(),
        passed: report.passed,
        case_count: u32::try_from(report.scenarios.len()).unwrap_or(u32::MAX),
        stack_mode,
        subject_revision,
        e2e_repository: system.e2e_repository.clone(),
        e2e_revision: system.e2e_revision.clone(),
        archive,
    };
    validate_history_record(&record)?;
    Ok(record)
}

fn validate_history_record(record: &HistoryRecord) -> Result<()> {
    if record.ingestion_id != record.archive.archive_id
        || record.execution_id != record.archive.execution_id
        || record.identity_sha256 != record.archive.identity_sha256
    {
        bail!("history identity differs from its archive reference");
    }
    DateTime::parse_from_rfc3339(&record.occurred_at)
        .context("history occurred_at must be RFC 3339")?;
    validate_archive_reference(&record.archive)
}

fn validate_archive_reference(reference: &DurableArchiveReference) -> Result<()> {
    if reference.archive_id.len() != 32
        || !reference
            .archive_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("invalid durable archive reference");
    }
    validate_storage_reference(&reference.manifest)?;
    validate_storage_reference(&reference.manifest_backup)?;
    DateTime::parse_from_rfc3339(&reference.created_at)
        .context("archive created_at must be RFC 3339")?;
    if let Some(expires_at) = &reference.expires_at {
        DateTime::parse_from_rfc3339(expires_at).context("archive expires_at must be RFC 3339")?;
    } else if reference.retention_class != RetentionClass::Canonical {
        bail!("non-canonical archive must declare expires_at");
    }
    Ok(())
}

fn validate_manifest(manifest: &DurableArchiveManifest) -> Result<()> {
    if manifest.objects.is_empty() {
        bail!("empty durable archive manifest");
    }
    for object in &manifest.objects {
        safe_path(&object.relative_path)?;
        if object.chunks.is_empty()
            || object
                .chunks
                .iter()
                .enumerate()
                .any(|(index, chunk)| chunk.index != index as u32)
        {
            bail!(
                "artifact {} has an invalid chunk sequence",
                object.relative_path
            );
        }
        for chunk in &object.chunks {
            validate_storage_reference(&chunk.object)?;
        }
    }
    Ok(())
}

fn validate_storage_reference(reference: &StorageObjectReference) -> Result<()> {
    if reference.bucket.trim().is_empty()
        || reference.key.trim().is_empty()
        || sha256_digest(&reference.sha256).is_none()
        || reference.uri != immutable_uri(&reference.bucket, &reference.key, &reference.sha256)
    {
        bail!("invalid immutable storage reference");
    }
    Ok(())
}

fn expired(reference: &DurableArchiveReference, now: DateTime<Utc>) -> Result<bool> {
    reference
        .expires_at
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .context("archive expires_at must be RFC 3339")
        .map(|expires| expires.is_some_and(|expires| expires.with_timezone(&Utc) <= now))
}

fn list_files(root: &Path) -> Result<Vec<PathBuf>> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(directory)
            .with_context(|| format!("read archive directory {}", directory.display()))?
        {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                bail!(
                    "archive input cannot contain symlinks: {}",
                    entry.path().display()
                );
            }
            if file_type.is_dir() {
                visit(root, &entry.path(), files)?;
            } else if file_type.is_file() {
                files.push(entry.path().strip_prefix(root)?.to_path_buf());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    if files.is_empty() {
        bail!("archive source is empty");
    }
    Ok(files)
}

fn safe_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe archive path: {value}");
    }
    Ok(path.to_path_buf())
}

fn immutable_uri(bucket: &str, key: &str, sha256: &str) -> String {
    format!(
        "iii-storage://{bucket}/{key}?sha256={}",
        digest_component(sha256)
    )
}

fn digest_component(value: &str) -> String {
    value
        .strip_prefix("sha256:")
        .unwrap_or(value)
        .to_ascii_lowercase()
}

fn sha256_digest(value: &str) -> Option<&str> {
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(digest)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn environment(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

const fn default_history_limit() -> u16 {
    100
}

const fn default_sweep_limit() -> u16 {
    100
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeCaller {
        calls: Mutex<Vec<(String, Value)>>,
    }

    #[async_trait]
    impl FunctionCaller for FakeCaller {
        async fn call(&self, function_id: &str, payload: Value) -> Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((function_id.into(), payload.clone()));
            Ok(match function_id {
                STORAGE_PUT => json!({
                    "etag": "etag",
                    "size": BASE64.decode(payload["body_base64"].as_str().unwrap()).unwrap().len(),
                    "version_id": null,
                }),
                STORAGE_HEAD => {
                    json!({"size": 3, "etag": "etag", "content_type": "text/plain", "last_modified": now()})
                }
                STORAGE_GET => json!({"body_base64": BASE64.encode(b"abc"), "size": 3}),
                DATABASE_EXECUTE => json!({"affected_rows": 1}),
                DATABASE_QUERY => json!({"rows": []}),
                _ => bail!("unexpected fake function {function_id}"),
            })
        }
    }

    fn config() -> DurableConfig {
        DurableConfig {
            database: "analytics".into(),
            backup_bucket: "backup".into(),
            timeout_ms: 1_000,
        }
    }

    fn object(bucket: &str, key: &str, byte: char) -> StorageObjectReference {
        let sha256 = format!("sha256:{}", byte.to_string().repeat(64));
        StorageObjectReference {
            uri: immutable_uri(bucket, key, &sha256),
            bucket: bucket.into(),
            key: key.into(),
            sha256,
            size_bytes: 1,
            media_type: "application/json".into(),
        }
    }

    #[test]
    fn retention_policy_is_explicit_and_canonical_never_expires() {
        assert_eq!(
            RetentionClass::Temporary.lifetime(),
            Some(Duration::days(1))
        );
        assert_eq!(
            RetentionClass::PullRequest.lifetime(),
            Some(Duration::days(14))
        );
        assert_eq!(
            RetentionClass::Longitudinal.lifetime(),
            Some(Duration::days(400))
        );
        assert_eq!(RetentionClass::Canonical.lifetime(), None);
    }

    #[test]
    fn immutable_uris_include_the_content_hash() {
        let digest = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            immutable_uri("private", "runs/object", &digest),
            format!(
                "iii-storage://private/runs/object?sha256={}",
                "a".repeat(64)
            )
        );
    }

    #[test]
    fn archive_paths_cannot_escape_the_restore_root() {
        assert!(safe_path("runs/evidence.json").is_ok());
        assert!(safe_path("../secret").is_err());
        assert!(safe_path("/absolute").is_err());
    }

    #[tokio::test]
    async fn put_uses_only_storage_functions_and_checks_head() {
        let caller = Arc::new(FakeCaller::default());
        let history = DurableHistory::with_caller(caller.clone(), config());
        let reference = history
            .put_object(
                "private",
                "object",
                b"abc",
                "text/plain",
                json!({"retention": "temporary"}),
            )
            .await
            .unwrap();
        assert_eq!(reference.size_bytes, 3);
        let calls = caller.calls.lock().unwrap();
        assert_eq!(calls[0].0, STORAGE_PUT);
        assert_eq!(calls[1].0, STORAGE_HEAD);
        assert_eq!(calls[2].0, STORAGE_GET);
        assert!(calls.iter().all(|(id, _)| id.starts_with("storage::")));
    }

    #[tokio::test]
    async fn expired_archives_are_not_reported_as_corrupt_or_fetched() {
        let caller = Arc::new(FakeCaller::default());
        let history = DurableHistory::with_caller(caller.clone(), config());
        let archive = DurableArchiveReference {
            archive_id: "a".repeat(32),
            execution_id: "execution".into(),
            identity_sha256: format!("sha256:{}", "b".repeat(64)),
            retention_class: RetentionClass::Temporary,
            created_at: "2020-01-01T00:00:00Z".into(),
            expires_at: Some("2020-01-02T00:00:00Z".into()),
            manifest: object("temporary", "manifest", 'c'),
            manifest_backup: object("backup", "manifest", 'c'),
        };

        let response = history.head(archive).await.unwrap();
        assert!(matches!(
            response.availability,
            ArchiveAvailability::Expired
        ));
        assert!(caller.calls.lock().unwrap().is_empty());
    }
}
