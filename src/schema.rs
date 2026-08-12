use schemars::gen::SchemaSettings;
use schemars::schema::{RootSchema, Schema};
use schemars::JsonSchema;

use crate::durable::{DurableArchiveManifest, HistoryRecord};
use crate::fault::{FaultEvaluation, FaultJournal, FaultPlan, FaultProfile};
use crate::report::{E2eManifestV2, E2eReport};

pub fn results_v2() -> RootSchema {
    let mut root = versioned_root_schema_for::<E2eReport>(2);
    let object = root
        .schema
        .object
        .as_mut()
        .expect("results v2 schema has an object root");
    object.required.extend(
        [
            "schema_version",
            "execution",
            "system_under_test",
            "manifest",
        ]
        .into_iter()
        .map(str::to_string),
    );
    let scenario = root
        .definitions
        .get_mut("E2eScenarioReport")
        .expect("results v2 schema declares E2eScenarioReport");
    let Schema::Object(scenario) = scenario else {
        panic!("E2eScenarioReport has an object schema")
    };
    let scenario = scenario
        .object
        .as_mut()
        .expect("E2eScenarioReport has object validation");
    scenario.required.insert("case".to_string());
    root.schema
        .metadata()
        .title
        .replace("E2eResultsV2".to_string());
    root
}

pub fn manifest_v2() -> RootSchema {
    versioned_root_schema_for::<E2eManifestV2>(2)
}

pub fn durable_archive_v1() -> RootSchema {
    versioned_root_schema_for::<DurableArchiveManifest>(1)
}

pub fn history_record_v1() -> RootSchema {
    versioned_root_schema_for::<HistoryRecord>(1)
}

pub fn fault_profile_v1() -> RootSchema {
    versioned_root_schema_for::<FaultProfile>(1)
}

pub fn fault_plan_v1() -> RootSchema {
    versioned_root_schema_for::<FaultPlan>(1)
}

pub fn fault_journal_v1() -> RootSchema {
    versioned_root_schema_for::<FaultJournal>(1)
}

pub fn fault_evaluation_v1() -> RootSchema {
    versioned_root_schema_for::<FaultEvaluation>(1)
}

fn versioned_root_schema_for<T: JsonSchema>(version: u32) -> RootSchema {
    let mut root = SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<T>();
    let properties = root
        .schema
        .object
        .as_mut()
        .expect("versioned E2E schema has an object root");
    let version_schema = properties
        .properties
        .get_mut("schema_version")
        .expect("versioned E2E schema declares schema_version");
    let Schema::Object(version_schema) = version_schema else {
        panic!("versioned E2E schema has a typed schema_version")
    };
    version_schema.metadata().default = None;
    version_schema.enum_values = Some(vec![serde_json::json!(version)]);
    root
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde::Serialize;

    use super::*;

    #[test]
    fn results_v2_schema_matches_snapshot() {
        assert_snapshot("results-v2.json", &results_v2());
    }

    #[test]
    fn manifest_v2_schema_matches_snapshot() {
        assert_snapshot("manifest-v2.json", &manifest_v2());
    }

    #[test]
    fn durable_archive_v1_schema_matches_snapshot() {
        assert_snapshot("durable-archive-v1.json", &durable_archive_v1());
    }

    #[test]
    fn history_record_v1_schema_matches_snapshot() {
        assert_snapshot("history-record-v1.json", &history_record_v1());
    }

    #[test]
    fn fault_schemas_match_snapshots() {
        for (name, schema) in [
            ("fault-profile-v1.json", fault_profile_v1()),
            ("fault-plan-v1.json", fault_plan_v1()),
            ("fault-journal-v1.json", fault_journal_v1()),
            ("fault-evaluation-v1.json", fault_evaluation_v1()),
        ] {
            assert_snapshot(name, &schema);
        }
    }

    fn assert_snapshot(name: &str, schema: &impl Serialize) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schemas")
            .join(name);
        let mut rendered = serde_json::to_string_pretty(schema).unwrap();
        rendered.push('\n');
        if std::env::var_os("HARNESS_E2E_UPDATE_SCHEMAS").is_some() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &rendered).unwrap();
        }
        let expected = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read schema snapshot {}: {error}", path.display()));
        assert_eq!(
            rendered,
            expected,
            "schema snapshot {} changed",
            path.display()
        );
    }
}
