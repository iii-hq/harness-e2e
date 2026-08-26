use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::artifact::{self, ArtifactReference};

use super::{ImprovementLoopRecord, ImprovementLoopSpecV1};

const RECORD_FILE: &str = "loop.json";
const SPEC_FILE: &str = "spec.json";

#[derive(Debug, Clone)]
pub struct ImprovementStore {
    root: PathBuf,
}

impl ImprovementStore {
    pub fn new(runs_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: runs_dir.into().join("improvement-loops"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn loop_dir(&self, id: &str) -> Result<PathBuf> {
        validate_loop_id(id)?;
        Ok(self.root.join(id))
    }

    pub fn create(&self, spec: ImprovementLoopSpecV1) -> Result<ImprovementLoopRecord> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        let id = format!(
            "improve-{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let loop_dir = self.loop_dir(&id)?;
        fs::create_dir(&loop_dir)
            .with_context(|| format!("create improvement loop {}", loop_dir.display()))?;
        let record = ImprovementLoopRecord::new(id, spec)?;
        self.write_json_file(&loop_dir.join(SPEC_FILE), &record.spec)?;
        self.write(&record)?;
        Ok(record)
    }

    pub fn write(&self, record: &ImprovementLoopRecord) -> Result<()> {
        if record.id.trim().is_empty() {
            bail!("improvement loop record id is empty");
        }
        record.validate_integrity()?;
        let loop_dir = self.loop_dir(&record.id)?;
        fs::create_dir_all(&loop_dir).with_context(|| format!("create {}", loop_dir.display()))?;
        self.write_json_file(&loop_dir.join(RECORD_FILE), record)?;
        self.write_json_file(&loop_dir.join("journal.json"), &record.transitions)
    }

    pub fn read(&self, id: &str) -> Result<Option<ImprovementLoopRecord>> {
        let path = self.loop_dir(id)?.join(RECORD_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let record: ImprovementLoopRecord = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("decode {}", path.display()))?;
        if record.id != id {
            bail!("improvement loop record id differs from its directory");
        }
        record.validate_integrity()?;
        let spec_path = self.loop_dir(id)?.join(SPEC_FILE);
        let immutable_spec: ImprovementLoopSpecV1 = serde_json::from_slice(
            &fs::read(&spec_path).with_context(|| format!("read {}", spec_path.display()))?,
        )
        .with_context(|| format!("decode {}", spec_path.display()))?;
        if immutable_spec != record.spec {
            bail!("improvement loop spec differs from immutable spec.json");
        }
        Ok(Some(record))
    }

    pub fn get(&self, id: &str) -> Result<ImprovementLoopRecord> {
        self.read(id)?
            .with_context(|| format!("improvement loop '{id}' not found"))
    }

    pub fn list(&self) -> Result<Vec<ImprovementLoopRecord>> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in
            fs::read_dir(&self.root).with_context(|| format!("read {}", self.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(record) = self.read(&id)? {
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(records)
    }

    pub fn write_artifact<T: Serialize>(
        &self,
        loop_id: &str,
        relative_path: &Path,
        artifact_id: impl Into<String>,
        kind: impl Into<String>,
        value: &T,
    ) -> Result<ArtifactReference> {
        let loop_dir = self.loop_dir(loop_id)?;
        artifact::write_json(&loop_dir, relative_path, artifact_id, kind, value)
    }

    pub fn write_text_artifact(
        &self,
        loop_id: &str,
        relative_path: &Path,
        artifact_id: impl Into<String>,
        kind: impl Into<String>,
        text: &str,
    ) -> Result<ArtifactReference> {
        let loop_dir = self.loop_dir(loop_id)?;
        artifact::write_bytes(
            &loop_dir,
            relative_path,
            artifact_id,
            kind,
            "text/plain; charset=utf-8",
            text.as_bytes(),
        )
    }

    pub fn read_artifact<T: DeserializeOwned>(
        &self,
        loop_id: &str,
        reference: &ArtifactReference,
    ) -> Result<T> {
        let loop_dir = self.loop_dir(loop_id)?;
        reference.verify(&loop_dir)?;
        let path = loop_dir.join(&reference.path);
        serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("decode {}", path.display()))
    }

    pub fn artifact_path(&self, loop_id: &str, reference: &ArtifactReference) -> Result<PathBuf> {
        let loop_dir = self.loop_dir(loop_id)?;
        reference.verify(&loop_dir)?;
        Ok(loop_dir.join(&reference.path))
    }

    fn write_json_file<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(value)
            .with_context(|| format!("serialize {}", path.display()))?;
        bytes.push(b'\n');
        artifact::write_atomic(path, &bytes)
    }
}

fn validate_loop_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("improvement loop id is invalid");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::improvement::tests::valid_spec;

    #[test]
    fn records_are_written_atomically_and_sorted_by_update_time() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImprovementStore::new(temp.path());
        let first = store.create(valid_spec(temp.path())).unwrap();
        let second = store.create(valid_spec(temp.path())).unwrap();
        assert_eq!(store.get(&first.id).unwrap().id, first.id);
        let records = store.list().unwrap();
        assert_eq!(records.len(), 2);
        let ids = records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains(first.id.as_str()));
        assert!(ids.contains(second.id.as_str()));
    }

    #[test]
    fn a_directory_cannot_substitute_a_different_record_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImprovementStore::new(temp.path());
        let mut record = store.create(valid_spec(temp.path())).unwrap();
        let id = record.id.clone();
        record.id = "different".into();
        store
            .write_json_file(&store.loop_dir(&id).unwrap().join(RECORD_FILE), &record)
            .unwrap();
        assert!(store.read(&id).is_err());
    }

    #[test]
    fn immutable_spec_and_record_hashes_reject_local_drift() {
        let temp = tempfile::tempdir().unwrap();
        let store = ImprovementStore::new(temp.path());
        let mut record = store.create(valid_spec(temp.path())).unwrap();
        let id = record.id.clone();
        record.spec.label = "tampered".into();
        store
            .write_json_file(&store.loop_dir(&id).unwrap().join(RECORD_FILE), &record)
            .unwrap();
        assert!(store.read(&id).is_err());
    }
}
