use std::fs;
use std::path::{Component, Path};

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactReference {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
}

impl ArtifactReference {
    pub fn verify(&self, output: &Path) -> Result<()> {
        if self.id.trim().is_empty()
            || self.kind.trim().is_empty()
            || self.media_type.trim().is_empty()
        {
            bail!("artifact reference metadata must be non-empty");
        }
        let relative_path = Path::new(&self.path);
        validate_relative_path(relative_path)?;
        let path = output.join(relative_path);
        let bytes = fs::read(&path).with_context(|| format!("read artifact {}", path.display()))?;
        if bytes.len() as u64 != self.size_bytes {
            bail!("artifact {} size does not match its reference", self.path);
        }
        if sha256_bytes(&bytes) != self.sha256 {
            bail!("artifact {} hash does not match its reference", self.path);
        }
        Ok(())
    }
}

pub fn write_json<T>(
    output: &Path,
    relative_path: &Path,
    id: impl Into<String>,
    kind: impl Into<String>,
    value: &T,
) -> Result<ArtifactReference>
where
    T: Serialize,
{
    validate_relative_path(relative_path)?;
    let path = output.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)
        .with_context(|| format!("serialize {}", relative_path.display()))?;
    bytes.push(b'\n');
    write_atomic(&path, &bytes)?;
    Ok(ArtifactReference {
        id: id.into(),
        kind: kind.into(),
        path: relative_path.to_string_lossy().replace('\\', "/"),
        sha256: sha256_bytes(&bytes),
        size_bytes: bytes.len().try_into().unwrap_or(u64::MAX),
        media_type: "application/json".to_string(),
    })
}

pub fn write_bytes(
    output: &Path,
    relative_path: &Path,
    id: impl Into<String>,
    kind: impl Into<String>,
    media_type: impl Into<String>,
    bytes: &[u8],
) -> Result<ArtifactReference> {
    validate_relative_path(relative_path)?;
    let path = output.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if path.exists() {
        let existing = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if existing != bytes {
            bail!(
                "immutable artifact {} already exists with different content",
                relative_path.display()
            );
        }
    } else {
        write_atomic(&path, bytes)?;
    }
    Ok(ArtifactReference {
        id: id.into(),
        kind: kind.into(),
        path: relative_path.to_string_lossy().replace('\\', "/"),
        sha256: sha256_bytes(bytes),
        size_bytes: bytes.len().try_into().unwrap_or(u64::MAX),
        media_type: media_type.into(),
    })
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("artifact path must have a UTF-8 file name")?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    fs::write(&temporary, bytes).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

pub fn sha256_value<T>(value: &T) -> Result<String>
where
    T: Serialize,
{
    let value = serde_json::to_value(value).context("serialize value for SHA-256")?;
    let canonical = canonical_json(value);
    let bytes = serde_json::to_vec(&canonical).context("encode canonical JSON")?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_json(value)))
                    .collect(),
            )
        }
        other => other,
    }
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("artifact path must be a non-empty relative path");
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("artifact path cannot contain parent, root, or current-directory components");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hash_is_independent_of_object_key_order() {
        let left = serde_json::json!({"b": 2, "a": {"d": 4, "c": 3}});
        let right = serde_json::json!({"a": {"c": 3, "d": 4}, "b": 2});
        assert_eq!(sha256_value(&left).unwrap(), sha256_value(&right).unwrap());
    }

    #[test]
    fn artifact_paths_cannot_escape_the_output_directory() {
        let output = tempfile::tempdir().unwrap();
        let error = write_json(
            output.path(),
            Path::new("../outside.json"),
            "evidence",
            "test",
            &serde_json::json!({}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot contain"));
    }

    #[test]
    fn artifact_verification_detects_content_changes() {
        let output = tempfile::tempdir().unwrap();
        let reference = write_json(
            output.path(),
            Path::new("evidence.json"),
            "evidence",
            "test",
            &serde_json::json!({"value": 1}),
        )
        .unwrap();
        reference.verify(output.path()).unwrap();
        fs::write(output.path().join("evidence.json"), b"{}\n").unwrap();

        let error = reference.verify(output.path()).unwrap_err();
        assert!(error.to_string().contains("size does not match"));
    }

    #[test]
    fn immutable_bytes_are_idempotent_but_cannot_be_replaced() {
        let output = tempfile::tempdir().unwrap();
        let path = Path::new("comparison/summary.md");
        let first = write_bytes(
            output.path(),
            path,
            "summary",
            "test",
            "text/markdown",
            b"v1",
        )
        .unwrap();
        let replay = write_bytes(
            output.path(),
            path,
            "summary",
            "test",
            "text/markdown",
            b"v1",
        )
        .unwrap();
        assert_eq!(first, replay);

        let error = write_bytes(
            output.path(),
            path,
            "summary",
            "test",
            "text/markdown",
            b"v2",
        )
        .unwrap_err();
        assert!(error.to_string().contains("immutable artifact"));
    }
}
