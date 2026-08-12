use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::wire::ControlPlaneEvidence;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionIdentity {
    pub execution_id: String,
    pub lane: String,
    pub started_at: String,
    pub completed_at: String,
}

impl ExecutionIdentity {
    pub fn validate(&self) -> Result<()> {
        required_value(self.execution_id.clone(), "execution id")?;
        required_value(self.lane.clone(), "execution lane")?;
        let started_at = DateTime::parse_from_rfc3339(&self.started_at)
            .context("execution started_at must be RFC 3339")?;
        let completed_at = DateTime::parse_from_rfc3339(&self.completed_at)
            .context("execution completed_at must be RFC 3339")?;
        if completed_at < started_at {
            bail!("execution completed_at precedes started_at");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum StackIdentity {
    Source {
        workers_repository: String,
        workers_revision: String,
    },
    Registry {
        stack_versions: BTreeMap<String, String>,
        stack_lock_digest: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SystemUnderTestIdentity {
    pub stack: StackIdentity,
    pub engine_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_revision: Option<String>,
    pub harness_version: String,
    pub e2e_repository: String,
    pub e2e_revision: String,
    pub contract_hashes: BTreeMap<String, String>,
}

impl SystemUnderTestIdentity {
    pub fn from_environment(
        engine_version: String,
        harness_version: String,
        contracts: &ControlPlaneEvidence,
    ) -> Result<Self> {
        let stack_mode = nonempty_env("HARNESS_E2E_STACK_MODE").unwrap_or_else(|| "source".into());
        let stack = match stack_mode.as_str() {
            "source" => StackIdentity::Source {
                workers_repository: identity_value(
                    "HARNESS_E2E_WORKERS_REPOSITORY",
                    env!("HARNESS_E2E_BUILD_REPOSITORY"),
                )?,
                workers_revision: identity_value(
                    "HARNESS_E2E_WORKERS_REVISION",
                    env!("HARNESS_E2E_BUILD_REVISION"),
                )?,
            },
            "registry" => {
                let raw = nonempty_env("HARNESS_E2E_STACK_VERSIONS")
                    .context("registry identity requires HARNESS_E2E_STACK_VERSIONS")?;
                let stack_versions: BTreeMap<String, String> = serde_json::from_str(&raw)
                    .context("HARNESS_E2E_STACK_VERSIONS must be a JSON object")?;
                if stack_versions.is_empty() {
                    bail!("registry identity requires at least one exact stack version");
                }
                let stack_lock_digest = nonempty_env("HARNESS_E2E_STACK_DIGEST")
                    .context("registry identity requires HARNESS_E2E_STACK_DIGEST")?;
                validate_sha256(&stack_lock_digest, "HARNESS_E2E_STACK_DIGEST")?;
                StackIdentity::Registry {
                    stack_versions,
                    stack_lock_digest,
                }
            }
            other => bail!("HARNESS_E2E_STACK_MODE must be source or registry, got {other}"),
        };
        let contract_hashes = contracts
            .functions
            .iter()
            .map(|contract| (contract.function_id.clone(), contract.sha256.clone()))
            .collect();
        let identity = Self {
            stack,
            engine_version: required_value(engine_version, "engine version")?,
            engine_revision: nonempty_env("HARNESS_E2E_ENGINE_REVISION"),
            harness_version: required_value(harness_version, "Harness version")?,
            e2e_repository: identity_value(
                "HARNESS_E2E_REPOSITORY",
                env!("HARNESS_E2E_BUILD_REPOSITORY"),
            )?,
            e2e_revision: identity_value(
                "HARNESS_E2E_REVISION",
                env!("HARNESS_E2E_BUILD_REVISION"),
            )?,
            contract_hashes,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<()> {
        required_value(self.engine_version.clone(), "engine version")?;
        required_value(self.harness_version.clone(), "Harness version")?;
        if let Some(revision) = &self.engine_revision {
            required_value(revision.clone(), "engine revision")?;
        }
        required_value(self.e2e_repository.clone(), "E2E repository")?;
        validate_git_revision(&self.e2e_revision, "E2E revision")?;
        if self.contract_hashes.is_empty() {
            bail!("system identity has no control-plane contract hashes");
        }
        for (function_id, digest) in &self.contract_hashes {
            required_value(function_id.clone(), "contract function id")?;
            validate_sha256(digest, function_id)?;
        }
        match &self.stack {
            StackIdentity::Source {
                workers_repository,
                workers_revision,
            } => {
                required_value(workers_repository.clone(), "workers repository")?;
                validate_git_revision(workers_revision, "workers revision")?;
            }
            StackIdentity::Registry {
                stack_versions,
                stack_lock_digest,
            } => {
                if stack_versions.is_empty() {
                    bail!("registry identity has no stack versions");
                }
                for (worker, version) in stack_versions {
                    required_value(worker.clone(), "registry worker")?;
                    required_value(version.clone(), "registry worker version")?;
                }
                validate_sha256(stack_lock_digest, "stack lock")?;
            }
        }
        Ok(())
    }
}

fn validate_git_revision(value: &str, label: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} must be a full immutable Git SHA")
    }
    Ok(())
}

pub fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn identity_value(name: &str, fallback: &str) -> Result<String> {
    required_value(
        nonempty_env(name).unwrap_or_else(|| fallback.to_string()),
        name,
    )
}

fn required_value(value: String, label: &str) -> Result<String> {
    if value.trim().is_empty() || value == "unknown" {
        bail!("{label} is unavailable; provide an explicit immutable identity")
    }
    Ok(value)
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} is not a SHA-256 digest")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn build_identity_is_immutable_in_a_git_checkout() {
        assert_ne!(env!("HARNESS_E2E_BUILD_REPOSITORY"), "unknown");
        let revision = env!("HARNESS_E2E_BUILD_REVISION");
        assert_eq!(revision.len(), 40);
        assert!(revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
