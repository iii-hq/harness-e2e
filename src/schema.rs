use schemars::gen::SchemaSettings;
use schemars::schema::{RootSchema, Schema};
use schemars::JsonSchema;

use crate::assessment::{AnalysisBundle, AnalysisResponse};
use crate::asset::AssetCaptureManifest;
use crate::control::ScenariosListResponse;
use crate::durable::{DurableArchiveManifest, HistoryRecord};
use crate::fault::{FaultEvaluation, FaultJournal, FaultPlan, FaultProfile};
use crate::report::{E2eManifest, E2eObservationEnvelope, E2eReport};
use crate::workflow::WorkflowCheckpointV1;

pub fn results() -> RootSchema {
    let mut root = root_schema_for::<E2eReport>();
    let object = root
        .schema
        .object
        .as_mut()
        .expect("results schema has an object root");
    object.required.insert("manifest".to_string());
    let scenario = root
        .definitions
        .get_mut("E2eScenarioReport")
        .expect("results schema declares E2eScenarioReport");
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
        .replace("E2eResults".to_string());
    root
}

pub fn analysis_bundle() -> RootSchema {
    root_schema_for::<AnalysisBundle>()
}

pub fn analysis_response() -> RootSchema {
    root_schema_for::<AnalysisResponse>()
}

pub fn asset_capture() -> RootSchema {
    root_schema_for::<AssetCaptureManifest>()
}

pub fn manifest() -> RootSchema {
    root_schema_for::<E2eManifest>()
}

pub fn observation() -> RootSchema {
    root_schema_for::<E2eObservationEnvelope>()
}

pub fn scenario_catalog() -> RootSchema {
    root_schema_for::<ScenariosListResponse>()
}

pub fn workflow_checkpoint() -> RootSchema {
    root_schema_for::<WorkflowCheckpointV1>()
}

pub fn durable_archive() -> RootSchema {
    root_schema_for::<DurableArchiveManifest>()
}

pub fn history_record() -> RootSchema {
    root_schema_for::<HistoryRecord>()
}

pub fn fault_profile() -> RootSchema {
    root_schema_for::<FaultProfile>()
}

pub fn fault_plan() -> RootSchema {
    root_schema_for::<FaultPlan>()
}

pub fn fault_journal() -> RootSchema {
    root_schema_for::<FaultJournal>()
}

pub fn fault_evaluation() -> RootSchema {
    root_schema_for::<FaultEvaluation>()
}

fn root_schema_for<T: JsonSchema>() -> RootSchema {
    SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<T>()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde::Serialize;

    use super::*;

    #[test]
    fn results_schema_matches_snapshot() {
        assert_snapshot("results.json", &results());
    }

    #[test]
    fn analysis_schemas_match_snapshots() {
        assert_snapshot("analysis-bundle.json", &analysis_bundle());
        assert_snapshot("analysis-response.json", &analysis_response());
    }

    #[test]
    fn asset_capture_schema_matches_snapshot() {
        assert_snapshot("asset-capture.json", &asset_capture());
    }

    #[test]
    fn manifest_schema_matches_snapshot() {
        assert_snapshot("manifest.json", &manifest());
    }

    #[test]
    fn observation_contract_schemas_match_snapshots() {
        assert_snapshot("e2e-observation-v1.json", &observation());
        assert_snapshot("e2e-scenario-catalog-v3.json", &scenario_catalog());
    }

    #[test]
    fn workflow_schemas_match_snapshots() {
        assert_snapshot("workflow-checkpoint-v1.json", &workflow_checkpoint());
    }

    #[test]
    fn durable_archive_schema_matches_snapshot() {
        assert_snapshot("durable-archive.json", &durable_archive());
    }

    #[test]
    fn history_record_schema_matches_snapshot() {
        assert_snapshot("history-record.json", &history_record());
    }

    #[test]
    fn fault_schemas_match_snapshots() {
        for (name, schema) in [
            ("fault-profile.json", fault_profile()),
            ("fault-plan.json", fault_plan()),
            ("fault-journal.json", fault_journal()),
            ("fault-evaluation.json", fault_evaluation()),
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
