use std::collections::{BTreeSet, HashMap};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::artifact;
use crate::context::E2eContext;
use crate::report::ObservedWorkerContract;

use super::{StepCatalog, WorkflowDefinitionV1};

pub(crate) async fn observe_worker_contracts(
    context: &E2eContext,
    catalog: &StepCatalog,
    definitions: &[WorkflowDefinitionV1],
) -> Result<Vec<ObservedWorkerContract>> {
    let used = definitions
        .iter()
        .flat_map(|definition| &definition.nodes)
        .map(|node| (node.step_type.as_str(), node.step_version))
        .collect::<BTreeSet<_>>();
    let mut expected_contracts = HashMap::new();
    for (step_type, version) in used {
        let registered = catalog
            .get(step_type, version)
            .context("validated step type disappeared from catalog")?;
        for required in &registered.descriptor.required_functions {
            let expectation = (
                required.request_schema_sha256.clone(),
                required.response_schema_sha256.clone(),
            );
            if let Some(previous) =
                expected_contracts.insert(required.function_id.clone(), expectation.clone())
            {
                if previous != expectation {
                    bail!(
                        "step catalog declares conflicting contract hashes for '{}'",
                        required.function_id
                    );
                }
            }
        }
    }
    let harness = crate::wire::control_plane_function_ids().collect::<BTreeSet<_>>();
    let function_ids = expected_contracts
        .keys()
        .filter(|function_id| !harness.contains(function_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if function_ids.is_empty() {
        return Ok(Vec::new());
    }
    let info = context
        .trigger_value(
            "engine::functions::info",
            json!({"function_ids": function_ids}),
        )
        .await?;
    let details = info
        .get("functions")
        .and_then(Value::as_array)
        .context("functions::info response is missing functions[]")?;
    let by_id = details
        .iter()
        .filter_map(|detail| Some((detail.get("function_id")?.as_str()?, detail)))
        .collect::<HashMap<_, _>>();
    function_ids
        .into_iter()
        .map(|function_id| {
            let detail = by_id
                .get(function_id.as_str())
                .with_context(|| format!("functions::info omitted '{function_id}'"))?;
            let request_schema = detail
                .get("request_schema")
                .filter(|schema| schema.is_object())
                .with_context(|| format!("'{function_id}' has no request schema"))?;
            let response_schema = detail
                .get("response_schema")
                .filter(|schema| schema.is_object())
                .with_context(|| format!("'{function_id}' has no response schema"))?;
            let request_schema_sha256 = artifact::sha256_value(request_schema)?;
            let response_schema_sha256 = artifact::sha256_value(response_schema)?;
            let expected = &expected_contracts[&function_id];
            if expected
                .0
                .as_ref()
                .is_some_and(|hash| hash != &request_schema_sha256)
                || expected
                    .1
                    .as_ref()
                    .is_some_and(|hash| hash != &response_schema_sha256)
            {
                bail!("observed contract for '{function_id}' differs from the registered exact contract");
            }
            Ok(ObservedWorkerContract {
                function_id,
                request_schema_sha256,
                response_schema_sha256,
            })
        })
        .collect()
}
