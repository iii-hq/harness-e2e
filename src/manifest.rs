use serde::Serialize;

use crate::worker::WorkerConfig;

pub const WORKER_NAME: &str = "harness-e2e";
pub const DESCRIPTION: &str =
    "Measure Harness capability, retain reproducible evidence, and expose the E2E dashboard.";

#[derive(Debug, Serialize)]
pub struct ModuleManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub default_config: serde_json::Value,
    pub supported_targets: Vec<String>,
}

pub fn build_manifest() -> ModuleManifest {
    ModuleManifest {
        name: WORKER_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: DESCRIPTION.into(),
        default_config: serde_json::to_value(WorkerConfig::default())
            .expect("worker defaults serialize"),
        supported_targets: vec![env!("TARGET").into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_manifest_has_the_public_worker_identity() {
        let manifest = build_manifest();
        assert_eq!(manifest.name, "harness-e2e");
        assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
        assert!(!manifest.description.is_empty());
        assert_eq!(
            manifest.default_config,
            serde_json::json!({ "data_dir": "~/.iii/data/harness-e2e" })
        );
        assert_eq!(manifest.supported_targets, [env!("TARGET")]);
    }
}
