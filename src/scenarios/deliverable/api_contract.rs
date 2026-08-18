//! An OpenAPI document for a specified service. Routes, methods, status
//! codes, and the resource schema are compared against the specification.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.api_contract";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "api_contract_artifact";
const CONTRACT_FILE: &str = "api/openapi.json";
const OPENAPI_VERSION: &str = "3.1.0";
const OPERATIONS: [(&str, &str, &[&str]); 4] = [
    ("/tasks", "get", &["200"]),
    ("/tasks", "post", &["201", "400"]),
    ("/tasks/{id}", "get", &["200", "404"]),
    ("/tasks/{id}", "delete", &["204", "404"]),
];
const TASK_FIELDS: [&str; 3] = ["done", "id", "title"];

const DOCUMENT_PARSES: AssessmentSpec = AssessmentSpec::hard_gated(
    "document_parses",
    15,
    "The file is JSON declaring the requested OpenAPI version.",
);
const OPERATION_SET_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "operation_set_exact",
    35,
    "The document declares exactly the specified path and method pairs.",
);
const STATUS_CODES_EXACT: AssessmentSpec = AssessmentSpec::hard_gated(
    "status_codes_exact",
    30,
    "Every operation documents exactly the specified responses.",
);
const RESOURCE_SCHEMA: AssessmentSpec = AssessmentSpec::hard_gated(
    "resource_schema",
    20,
    "The Task schema exists and requires exactly the specified fields.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    DOCUMENT_PARSES,
    OPERATION_SET_EXACT,
    STATUS_CODES_EXACT,
    RESOURCE_SCHEMA,
];

fn specification() -> String {
    OPERATIONS
        .iter()
        .map(|(path, method, statuses)| {
            format!(
                "- `{} {path}` responds with {}",
                method.to_uppercase(),
                statuses.join(" and ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expected_operations() -> Vec<Value> {
    OPERATIONS
        .iter()
        .map(|(path, method, statuses)| {
            json!({ "path": path, "method": method, "responses": statuses })
        })
        .collect()
}

fn observed_operations(document: &Value) -> Vec<Value> {
    let mut operations = Vec::new();
    if let Some(paths) = document.get("paths").and_then(Value::as_object) {
        for (path, methods) in paths {
            let Some(methods) = methods.as_object() else {
                continue;
            };
            for (method, operation) in methods {
                let mut responses: Vec<String> = operation
                    .get("responses")
                    .and_then(Value::as_object)
                    .map(|responses| responses.keys().cloned().collect())
                    .unwrap_or_default();
                responses.sort();
                operations.push(json!({
                    "path": path,
                    "method": method.to_lowercase(),
                    "responses": responses,
                }));
            }
        }
    }
    operations.sort_by_key(|operation| {
        (
            operation["path"].as_str().unwrap_or_default().to_string(),
            operation["method"].as_str().unwrap_or_default().to_string(),
        )
    });
    operations
}

fn sorted_expectation() -> Vec<Value> {
    let mut expected = expected_operations();
    expected.sort_by_key(|operation| {
        (
            operation["path"].as_str().unwrap_or_default().to_string(),
            operation["method"].as_str().unwrap_or_default().to_string(),
        )
    });
    expected
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Write the OpenAPI contract for a small task service in this workspace.\n\n\
             The service has exactly these operations:\n\n{}\n\n\
             1. Write `{CONTRACT_FILE}` as JSON with `\"openapi\": \"{OPENAPI_VERSION}\"`, an \
             `info` object, and a `paths` object holding exactly those path and method pairs. \
             Document exactly the response codes listed, no more and no fewer.\n\
             2. Declare `components.schemas.Task` as an object whose `required` array is exactly \
             {fields:?}.\n\
             3. Reply with exactly one line: `OPERATIONS:{count}`.",
            specification(),
            fields = TASK_FIELDS,
            count = OPERATIONS.len(),
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(18, 240_000, 360),
        assessments: ASSESSMENTS,
        setup: None,
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "contract_file": CONTRACT_FILE,
            "openapi": OPENAPI_VERSION,
            "operations": expected_operations(),
            "task_required": TASK_FIELDS,
        }),
        super::build_profile(1, 3),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["operations", "task_required", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

fn document(run_id: &str) -> Value {
    workspace::read_json(&workspace::root(ID, run_id), CONTRACT_FILE).unwrap_or(Value::Null)
}

fn task_required(document: &Value) -> Vec<String> {
    let mut fields: Vec<String> = document
        .pointer("/components/schemas/Task/required")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    fields.sort();
    fields
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let document = document(run_id);
        let operations = observed_operations(&document);
        let expected = sorted_expectation();
        let paths_and_methods: Vec<Value> = operations
            .iter()
            .map(|operation| json!({ "path": operation["path"], "method": operation["method"] }))
            .collect();
        let expected_paths: Vec<Value> = expected
            .iter()
            .map(|operation| json!({ "path": operation["path"], "method": operation["method"] }))
            .collect();
        let fields = task_required(&document);
        let expected_fields: Vec<String> = TASK_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect();

        Ok(assessment::build_evaluation([
            DOCUMENT_PARSES.full_or_zero(
                document.get("openapi").and_then(Value::as_str) == Some(OPENAPI_VERSION),
                format!("observed openapi field {:?}", document.get("openapi")),
            ),
            OPERATION_SET_EXACT.full_or_zero(
                paths_and_methods == expected_paths,
                format!("observed operations {paths_and_methods:?}"),
            ),
            STATUS_CODES_EXACT.full_or_zero(
                operations == expected,
                format!("observed responses {operations:?}"),
            ),
            RESOURCE_SCHEMA.full_or_zero(
                fields == expected_fields
                    && observation
                        .response
                        .contains(&format!("OPERATIONS:{}", OPERATIONS.len())),
                format!("observed Task required {fields:?}, expected {expected_fields:?}"),
            ),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let document = document(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "operations": observed_operations(&document),
                "task_required": task_required(&document),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_api_contract_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_document_satisfies_the_operation_comparison() {
        let document = json!({
            "openapi": OPENAPI_VERSION,
            "paths": {
                "/tasks": {
                    "get": { "responses": { "200": {} } },
                    "post": { "responses": { "201": {}, "400": {} } }
                },
                "/tasks/{id}": {
                    "get": { "responses": { "200": {}, "404": {} } },
                    "delete": { "responses": { "204": {}, "404": {} } }
                }
            }
        });
        assert_eq!(observed_operations(&document), sorted_expectation());
    }

    #[test]
    fn an_undocumented_response_breaks_the_comparison() {
        let document = json!({
            "paths": { "/tasks": { "get": { "responses": { "200": {}, "500": {} } } } }
        });
        assert_ne!(observed_operations(&document), sorted_expectation());
    }
}
