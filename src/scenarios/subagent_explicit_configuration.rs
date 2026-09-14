use std::collections::BTreeSet;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "subagent_explicit_configuration";
const ARTIFACT: &str = "subagent_configuration_evidence";
const ROLES: [&str; 2] = ["analyst", "auditor"];
const CHILD_TURNS: u32 = 8;
const CHILD_TOKENS: u64 = 4096;
const TOOL_ALLOW: [&str; 4] = [
    "state::get",
    "engine::functions::list",
    "engine::functions::info",
    "directory::skills::get",
];
const LEAF_DENY: [&str; 5] = [
    "harness::spawn",
    "harness::send",
    "engine::register_trigger",
    "engine::unregister_trigger",
    "engine::registered-triggers::*",
];
const SPAWNS: AssessmentSpec = AssessmentSpec::scored(
    "explicit_spawns", 20,
    "Exactly two successful spawns carry every requested field and create the named direct children.",
);
const CONFIG: AssessmentSpec = AssessmentSpec::scored(
    "effective_configuration", 25,
    "Both child turn records preserve the requested route, budgets, output contract, filesystem scope and leaf policy.",
);
const PROFILES: AssessmentSpec = AssessmentSpec::scored(
    "profiles_and_skills", 20,
    "Each child resolves its own profile and exact skill filter, with the fixture instructions preloaded and followed.",
);
const RESULTS: AssessmentSpec = AssessmentSpec::scored_in(
    "child_results",
    20,
    "Both children read the order fixture themselves and return the correct independent results.",
    EvaluationDimension::Deliverable,
);
const CONSOLIDATION: AssessmentSpec = AssessmentSpec::scored_in(
    "parent_consolidation", 15,
    "The parent observes both completed child results and consolidates them exactly after completion.",
    EvaluationDimension::Deliverable,
);
const ASSESSMENTS: &[AssessmentSpec] = &[SPAWNS, CONFIG, PROFILES, RESULTS, CONSOLIDATION];

fn name(run_id: &str, role: &str) -> String {
    let digest = crate::artifact::sha256_value(&json!(run_id)).expect("attempt id serializes");
    format!("e2e-subagent-{}-{role}", &digest[7..39])
}

fn child_id(run_id: &str, role: &str) -> String {
    format!("e2e_{run_id}-{role}")
}

fn skill_id(run_id: &str, role: &str) -> String {
    format!("harness/{}", name(run_id, role))
}

fn orders() -> Value {
    json!([
        {"id":"a", "category":"hardware", "amount":120},
        {"id":"b", "category":"software", "amount":50},
        {"id":"c", "category":"hardware", "amount":30},
        {"id":"b", "category":"software", "amount":50},
        {"id":"d", "category":"hardware", "amount":-5}
    ])
}

fn skill_body(run_id: &str, role: &str) -> String {
    let rule = if role == "analyst" {
        "Keep the first occurrence of each id, then exclude non-positive amounts. Return result as an object mapping category to the sum of its remaining amounts."
    } else {
        "Return result as an object with duplicate_ids (ids occurring more than once) and invalid_ids (ids with non-positive amounts). Each array contains unique ids sorted alphabetically."
    };
    format!(
        "{rule}\nInclude skill_marker exactly \"{}\" in your final JSON.",
        skill_id(run_id, role)
    )
}

fn profile_body(run_id: &str, role: &str) -> String {
    format!(
        "You are the {role}. Read the orders using state::get yourself, apply your preloaded skill, and return only the requested JSON. Include profile_marker exactly \"{}\". Never delegate or modify data.",
        name(run_id, role)
    )
}

fn resources(run_id: &str) -> Vec<(&'static str, String, String)> {
    ROLES.into_iter().flat_map(|role| {
        let skill = skill_id(run_id, role);
        [
            ("skills", skill.clone(), format!("---\ntitle: E2E {role}\n---\n{}", skill_body(run_id, role))),
            ("agents", name(run_id, role), format!("---\nname: E2E {role}\nskills: [{skill}]\nfunctions: [\"state::get\"]\n---\n{}", profile_body(run_id, role))),
        ]
    }).collect()
}

async fn installed_ids(context: &E2eContext, kind: &str, run_id: &str) -> Result<BTreeSet<String>> {
    let args = if kind == "skills" {
        json!({"prefix":format!("harness/{}", name(run_id,""))})
    } else {
        json!({})
    };
    let listed = context
        .trigger_value(&format!("directory::{kind}::list"), args)
        .await?;
    listed[kind]
        .as_array()
        .context("Directory listing has no entries array")?
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .map(str::to_owned)
                .context("Directory entry has no id")
        })
        .collect()
}

fn output_contract(role: &str) -> Value {
    let result = if role == "analyst" {
        json!({"type":"object", "additionalProperties":{"type":"integer"}})
    } else {
        json!({"type":"object", "required":["duplicate_ids","invalid_ids"],
        "additionalProperties":false, "properties":{
            "duplicate_ids":{"type":"array","items":{"type":"string"}},
            "invalid_ids":{"type":"array","items":{"type":"string"}}
        }})
    };
    json!({"type":"json", "schema":{
        "type":"object", "required":["profile_marker","skill_marker","result"],
        "additionalProperties":false, "properties":{
            "profile_marker":{"type":"string"}, "skill_marker":{"type":"string"},
            "result":result
        }
    }})
}

fn spawn_request(run_id: &str, role: &str, model: &Value, provider: &Value) -> Value {
    json!({
        "session_id":child_id(run_id, role), "agent":name(run_id, role),
        "model":model, "provider":provider,
        "display":{"name":role,"icon":if role == "analyst" {"database"} else {"review"},
            "color":if role == "analyst" {"blue"} else {"teal"}},
        "task":format!("Read state::get with scope '{}' and key 'orders'. Apply your profile and skill to those orders. Return the required JSON.", name(run_id, "data")),
        "options":{
            "max_turns":CHILD_TURNS, "max_output_tokens":CHILD_TOKENS,
            "thinking_level":"low", "max_validation_retries":1,
            "functions":{"allow":["state::get"]},
            "skills":[skill_id(run_id, role)], "orchestrator":false, "max_children":0,
            "filesystem_root":format!("/tmp/{}/{role}", name(run_id, "workspace")),
            "output":output_contract(role)
        }
    })
}

fn expected_result(run_id: &str, role: &str) -> Value {
    json!({"profile_marker":name(run_id, role), "skill_marker":skill_id(run_id, role),
        "result":if role == "analyst" {json!({"hardware":150,"software":50})}
            else {json!({"duplicate_ids":["b"],"invalid_ids":["d"]})}
    })
}

pub struct SubagentExplicitConfiguration;

#[async_trait]
impl Scenario for SubagentExplicitConfiguration {
    fn id(&self) -> &'static str {
        ID
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Delegate two small order-analysis tasks with explicit model, profile, skill, limits and permissions; verify the resolved children and their consolidated results.")
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        let mut contract = super::validation_loop::validation_contract(
            ARTIFACT,
            "json",
            json!({
                "type":"object", "required":["root_options","children","tree","response"],
                "additionalProperties":true
            }),
        );
        contract.artifacts[0].max_size_bytes = 1_048_576;
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "orders":orders(), "roles":ROLES, "model_policy":"explicit_parent_route",
                "child_max_turns":CHILD_TURNS, "child_max_output_tokens":CHILD_TOKENS,
                "thinking_level":"low", "max_validation_retries":1,
                "fixture_resources":resources(super::CONTRACT_NAMESPACE),
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eSubagents,
                Capability::IiiFunctions,
                Capability::IiiState,
                Capability::IiiTriggers,
            ],
            contract,
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let requests: Vec<_> = ROLES
            .iter()
            .map(|role| {
                spawn_request(
                    run_id,
                    role,
                    &json!("COPY_ROOT_MODEL"),
                    &json!("COPY_ROOT_PROVIDER"),
                )
            })
            .collect();
        ScenarioSpec {
            id: ID,
            prompt: format!(
                "Create exactly two subagents using the explicit configuration below.\n\
                 First read state::get with scope 'harness_turn' and key 'e2e_{run_id}'. \
                 Copy options.model and options.provider into the model/provider fields of BOTH \
                 requests, replacing COPY_ROOT_MODEL and COPY_ROOT_PROVIDER with the exact values. \
                 Do not omit those fields or rely on inheritance. The prepared profiles deliberately \
                 do not set a model.\n\
                 Call harness::spawn with each request:\n{}\n\
                 Keep all fields exact; task wording may vary if it preserves the instructions. \
                 The filesystem roots specify the child execution scope; this task needs no file operations. \
                 Both children must read the orders themselves and apply their own profile and skill. \
                 Do not perform their analysis or alter the fixture.\n\
                 After spawning BOTH children, read harness::status with session_id and verbose true \
                 for each child. If either is still running, register a one-shot timer wake with \
                 engine::register_trigger, trigger_type 'timer', config {{\"in_ms\":1000}}, once true \
                 and NO function_id, then END YOUR TURN. On the timer wake, check both statuses again. \
                 Repeat this timed wait only while a child is still running. \
                 Only when both completed, return exactly one JSON object with keys analyst and auditor, \
                 each containing that child's complete result object. Never claim success for a failed child.",
                serde_json::to_string_pretty(&requests).expect("spawn requests serialize")
            ),
            filesystem_root: None,
            execution: ExecutionPolicy { max_turns: Some(32), max_output_tokens: Some(8192),
                max_total_tokens: Some(200_000), stuck_timeout_seconds: Some(300),
                max_validation_retries: None },
            denied_functions: &[], criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn allowed_functions(&self, _run_id: &str) -> Option<Vec<String>> {
        Some(
            TOOL_ALLOW
                .into_iter()
                .chain([
                    "harness::spawn",
                    "harness::status",
                    "engine::register_trigger",
                    "engine::unregister_trigger",
                ])
                .map(str::to_owned)
                .collect(),
        )
    }

    fn required_functions(&self, _run_id: &str) -> Vec<String> {
        [
            "harness::spawn",
            "harness::status",
            "session::get",
            "session::messages",
            "state::get",
            "state::set",
            "state::delete",
            "directory::agents::create",
            "directory::agents::list",
            "directory::agents::get",
            "directory::agents::delete",
            "directory::skills::create",
            "directory::skills::list",
            "directory::skills::get",
            "directory::skills::delete",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        context
            .observe_function_contracts(&self.required_functions(run_id))
            .await?;
        let scope = name(run_id, "data");
        let ledger = common::state_value(
            context
                .trigger_value("state::get", json!({"scope":scope,"key":"fixture"}))
                .await?,
        );
        ensure!(
            ledger.is_null(),
            "an earlier fixture still needs cleanup: {scope}"
        );
        let resources = resources(run_id);
        for kind in ["skills", "agents"] {
            let installed = installed_ids(context, kind, run_id).await?;
            ensure!(
                resources
                    .iter()
                    .filter(|(k, _, _)| *k == kind)
                    .all(|(_, id, _)| !installed.contains(id)),
                "fixture id already exists in Directory {kind}"
            );
        }
        // Persist intent before any create, including ones whose response might be lost.
        context
            .trigger_value(
                "state::set",
                json!({"scope":scope,"key":"fixture","value":resources}),
            )
            .await?;
        for (kind, id, content) in &resources {
            let created = context
                .trigger_value(
                    &format!("directory::{kind}::create"),
                    json!({"id":id,"content":content}),
                )
                .await?;
            ensure!(
                created["id"] == *id,
                "Directory create returned a different id: {created}"
            );
        }
        for role in ROLES {
            let skill = skill_id(run_id, role);
            let profile = name(run_id, role);
            let loaded = context
                .trigger_value("directory::skills::get", json!({"id":skill}))
                .await?;
            ensure!(
                loaded["id"] == skill
                    && loaded["body"]
                        .as_str()
                        .is_some_and(|s| s.trim() == skill_body(run_id, role)),
                "prepared skill is not readable with its exact body: {skill}"
            );
            let loaded = context
                .trigger_value("directory::agents::get", json!({"id":profile}))
                .await?;
            ensure!(
                loaded["skills"] == json!([skill]) && loaded["model"].is_null(),
                "prepared profile differs from its configuration: {profile}"
            );
        }
        context
            .trigger_value(
                "state::set",
                json!({"scope":name(run_id,"data"), "key":"orders", "value":orders()}),
            )
            .await?;
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let root = common::state_value(
            context
                .trigger_value(
                    "state::get",
                    json!({"scope":"harness_turn", "key":observation.metrics.root_session_id}),
                )
                .await?,
        );
        ensure!(
            root["options"]["model"].is_string() && root["options"]["provider"].is_string(),
            "Harness turn evidence does not expose the root route"
        );
        let mut children = serde_json::Map::new();
        for role in ROLES {
            let id = child_id(run_id, role);
            let record = common::state_value(
                context
                    .trigger_value("state::get", json!({"scope":"harness_turn", "key":id}))
                    .await?,
            );
            let child = if record.is_null() {
                Value::Null
            } else {
                ensure!(
                    record["options"].is_object(),
                    "child turn record has no options: {id}"
                );
                json!({"record":{
                    "turn_id":record["turn_id"],"parent":record["parent"],"options":record["options"],
                    "status":record["status"],"result":record["result"],
                    "stop_reason":record["stop_reason"],"result_error":record["result_error"]
                }, "session":context.trigger_value("session::get", json!({"session_id":id})).await?,
                    "transcript":context.transcript(&id).await?})
            };
            children.insert(role.to_owned(), child);
        }
        Ok(vec![CapturedDeliverable {
            id: ARTIFACT.into(),
            kind: "json".into(),
            content: json!({"root_options":{
                "model":root["options"]["model"], "provider":root["options"]["provider"],
                "max_total_tokens":root["options"]["max_total_tokens"],
                "functions":root["options"]["functions"]
            }, "children":children, "tree":observation.metrics.by_session,
                "transcript":observation.transcript, "response":observation.response})
            .into(),
            invariants: Vec::new(),
            provenance: vec![ProvenanceEvidence {
                kind: "session".into(),
                source_id: observation.metrics.root_session_id.clone(),
                relation: "observed".into(),
            }],
        }])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        if !observation.metrics.complete {
            return Ok(assessment::prerequisite_failure(
                ASSESSMENTS,
                "complete_tree_metrics",
                "The session tree metrics are incomplete; exact child count cannot be verified.",
            ));
        }
        let evidence = observation
            .deliverables
            .iter()
            .find(|d| d.id == ARTIFACT)
            .and_then(|d| d.content.as_json())
            .context("missing captured subagent configuration evidence")?;
        Ok(evaluate_evidence(evidence, run_id))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let scope = name(run_id, "data");
        let ledger = common::state_value(
            context
                .trigger_value("state::get", json!({"scope":scope,"key":"fixture"}))
                .await?,
        );
        if ledger.is_null() {
            return Ok(());
        }
        let resources = resources(run_id);
        ensure!(
            ledger == serde_json::to_value(&resources)?,
            "fixture ownership record differs: {scope}"
        );
        let mut errors = Vec::new();
        for kind in ["agents", "skills"] {
            let installed = installed_ids(context, kind, run_id).await?;
            for (_, id, content) in resources
                .iter()
                .filter(|(k, id, _)| *k == kind && installed.contains(id))
            {
                let result: Result<()> = async {
                    let current = context
                        .trigger_value(
                            &format!("directory::{kind}::get"),
                            json!({"id":id,"raw":true}),
                        )
                        .await?;
                    ensure!(
                        current["raw"] == *content,
                        "refusing to delete changed fixture {id}"
                    );
                    context
                        .trigger_value(&format!("directory::{kind}::delete"), json!({"id":id}))
                        .await?;
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    errors.push(format!("{kind}/{id}: {error:#}"));
                }
            }
        }
        ensure!(
            errors.is_empty(),
            "fixture cleanup failed: {}",
            errors.join("; ")
        );
        for key in ["orders", "fixture"] {
            context
                .trigger_value("state::delete", json!({"scope":scope,"key":key}))
                .await?;
        }
        Ok(())
    }
}

fn string_set(value: &Value) -> BTreeSet<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn evaluate_evidence(evidence: &Value, run_id: &str) -> ObjectiveEvaluation {
    let calls = common::function_outcomes(&evidence["transcript"]);
    let spawns: Vec<_> = calls
        .iter()
        .filter(|c| c.function_id == "harness::spawn")
        .collect();
    let root = &evidence["root_options"];
    let tree = evidence["tree"].as_array();
    let root_id = format!("e2e_{run_id}");
    let exact_tree = tree.is_some_and(|rows| {
        rows.len() == 3
            && ROLES.iter().all(|role| {
                rows.iter().any(|row| {
                    row["session_id"] == child_id(run_id, role)
                        && row["parent_session_id"] == root_id
                        && row["depth"] == 1
                })
            })
    });
    let mut explicit = spawns.len() == 2 && exact_tree;
    let mut config = true;
    let mut profiles = true;
    let mut results = true;
    let mut observed = true;
    let mut finished = true;
    let mut summary = serde_json::Map::new();
    for role in ROLES {
        let id = child_id(run_id, role);
        let child = &evidence["children"][role];
        let record = &child["record"];
        let options = &record["options"];
        let session = &child["session"]["meta"];
        let expected = spawn_request(run_id, role, &root["model"], &root["provider"]);
        let matches: Vec<_> = spawns
            .iter()
            .filter(|c| c.arguments["session_id"] == id)
            .collect();
        explicit &= matches.len() == 1
            && matches.iter().all(|c| {
                c.is_error == Some(false)
                    && c.details.as_ref().is_some_and(|d| {
                        d["child_session_id"] == id
                            && d["child_turn_id"] == record["turn_id"]
                            && d["reused"] == false
                    })
                    && [
                        "model",
                        "provider",
                        "agent",
                        "display",
                        "session_id",
                        "options",
                    ]
                    .iter()
                    .all(|key| c.arguments[key] == expected[key])
                    && record["parent"]["session_id"] == root_id
                    && c.call_id.as_deref() == record["parent"]["function_call_id"].as_str()
            });
        let allowed = string_set(&options["functions"]["allow"]);
        let denied = string_set(&options["functions"]["deny"]);
        let expected_denied: BTreeSet<_> = string_set(&root["functions"]["deny"])
            .into_iter()
            .chain(LEAF_DENY)
            .collect();
        config &= !child.is_null()
            && options["model"] == root["model"]
            && options["provider"] == root["provider"]
            && options["max_turns"] == CHILD_TURNS
            && options["max_output_tokens"] == CHILD_TOKENS
            && options["max_total_tokens"] == root["max_total_tokens"]
            && options["thinking_level"] == "low"
            && options["max_validation_retries"] == 1
            && options["output"] == expected["options"]["output"]
            && options["metadata"]["fs_scope"]["root"] == expected["options"]["filesystem_root"]
            && session["title"] == role
            && session["metadata"]["subagent_display"] == expected["display"]
            && allowed == TOOL_ALLOW.into_iter().collect()
            && denied == expected_denied;
        let result = &record["result"];
        profiles &= options["agent"]["id"] == name(run_id, role)
            && session["metadata"]["agent_profile"]["id"] == name(run_id, role)
            && session["metadata"]["agent_profile"]["skills"] == json!([skill_id(run_id, role)])
            && options["skill_context"]["filter"] == json!([skill_id(run_id, role)])
            && options["system_prompt"].as_str().is_some_and(|s| {
                s.contains(&profile_body(run_id, role)) && s.contains(&skill_body(run_id, role))
            })
            && result["profile_marker"] == name(run_id, role)
            && result["skill_marker"] == skill_id(run_id, role);
        let complete = record["status"] == "completed"
            && record["stop_reason"].is_null()
            && record["result_error"].is_null();
        finished &= complete;
        let read_input = common::function_outcomes(&child["transcript"])
            .iter()
            .any(|c| {
                c.function_id == "state::get"
                    && c.is_error == Some(false)
                    && c.arguments == json!({"scope":name(run_id,"data"), "key":"orders"})
                    && c.details.clone().map(common::state_value).as_ref() == Some(&orders())
            });
        results &= complete && read_input && result == &expected_result(run_id, role);
        observed &= calls.iter().any(|c| {
            c.function_id == "harness::status"
                && c.arguments["session_id"] == id
                && c.is_error == Some(false)
                && c.details.as_ref().is_some_and(|d| {
                    d["session_id"] == id
                        && d["turn_id"] == record["turn_id"]
                        && d["status"] == "completed"
                        && d["result"] == *result
                })
        });
        summary.insert(role.to_owned(), result.clone());
    }
    let response = evidence["response"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let consolidated = finished && observed && response == Some(Value::Object(summary));
    assessment::build_evaluation(
        if finished && consolidated { CompletionState::Completed } else { CompletionState::TaskIncomplete },
        [SPAWNS.full_or_zero(explicit, format!("spawn_calls={}, exact_child_tree={exact_tree}, explicit_configuration={explicit}",spawns.len())),
            CONFIG.full_or_zero(config, format!("resolved_configuration_matches={config}")),
            PROFILES.full_or_zero(profiles, format!("own_profile_and_skill_applied={profiles}")),
            RESULTS.full_or_zero(results, format!("independent_correct_results={results}")),
            CONSOLIDATION.full_or_zero(consolidated, format!("both_finished={finished}, completed_results_observed={observed}, consolidated={consolidated}"))]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(id: &str, function: &str, arguments: Value, details: Value) -> Vec<Value> {
        vec![
            json!({"message":{"role":"assistant","content":[{
                "type":"function_call","id":id,"function_id":function,"arguments":arguments
            }]}}),
            json!({"message":{"role":"function_result","function_call_id":id,
                "function_id":function,"is_error":false,"details":details}}),
        ]
    }

    fn fixture() -> Value {
        let run_id = "test";
        let root = json!({"model":"test-model","provider":"test-provider",
            "max_total_tokens":200000,"functions":{"deny":["e2e::*"]}});
        let mut messages = Vec::new();
        let mut children = serde_json::Map::new();
        let mut tree = vec![json!({"session_id":"e2e_test","depth":0})];
        for role in ROLES {
            let id = child_id(run_id, role);
            let request = spawn_request(run_id, role, &root["model"], &root["provider"]);
            let result = expected_result(run_id, role);
            let turn = format!("turn-{role}");
            let call = format!("spawn-{role}");
            let options = json!({
                "model":"test-model","provider":"test-provider","max_turns":8,
                "max_output_tokens":4096,"max_total_tokens":200000,
                "thinking_level":"low","max_validation_retries":1,
                "output":output_contract(role),
                "metadata":{"fs_scope":{"root":request["options"]["filesystem_root"]}},
                "functions":{"allow":TOOL_ALLOW,
                    "deny":["e2e::*","harness::spawn","harness::send","engine::register_trigger",
                        "engine::unregister_trigger","engine::registered-triggers::*"]},
                "agent":{"id":name(run_id,role)},
                "skill_context":{"filter":[skill_id(run_id,role)]},
                "system_prompt":format!("{}\n<preloaded_skills>\n{}\n</preloaded_skills>",
                    profile_body(run_id,role),skill_body(run_id,role))
            });
            messages.extend(invocation(
                &call,
                "harness::spawn",
                request.clone(),
                json!({
                    "child_session_id":id,"child_turn_id":turn,"reused":false
                }),
            ));
            messages.extend(invocation(
                &format!("status-{role}"),
                "harness::status",
                json!({"session_id":id,"verbose":true}),
                json!({
                    "session_id":id,"turn_id":turn,"status":"completed","result":result
                }),
            ));
            let transcript = json!({"messages":invocation("read","state::get",
                json!({"scope":name(run_id,"data"),"key":"orders"}),orders())});
            children.insert(
                role.into(),
                json!({"record":{
                "turn_id":turn,"status":"completed","options":options,"result":result,
                "parent":{"session_id":"e2e_test","turn_id":"parent-turn","function_call_id":call}
            },"session":{"meta":{"title":role,"metadata":{
                "subagent_display":request["display"],
                "agent_profile":{"id":name(run_id,role),"skills":[skill_id(run_id,role)]}
            }}},"transcript":transcript}),
            );
            tree.push(json!({"session_id":id,"parent_session_id":"e2e_test","depth":1}));
        }
        json!({"root_options":root,"children":children,"tree":tree,
            "transcript":{"messages":messages},"response":json!({
                "analyst":expected_result(run_id,"analyst"),"auditor":expected_result(run_id,"auditor")
            }).to_string()})
    }

    fn points(evidence: &Value, criterion: &str) -> u8 {
        evaluate_evidence(evidence, "test")
            .awards
            .into_iter()
            .find(|a| a.id == criterion)
            .unwrap()
            .awarded
            .unwrap()
    }

    #[test]
    fn fixture_ids_do_not_collide_for_attempts_with_the_same_prefix() {
        let first = "abcd0000000000000000000000000000";
        let second = "abcd1111111111111111111111111111";
        assert_ne!(name(first, "data"), name(second, "data"));
        for (_, id, _) in resources(first) {
            assert!(id.split('/').all(|segment| segment.len() <= 64));
            assert!(!resources(second).iter().any(|(_, other, _)| other == &id));
        }
    }

    #[test]
    fn complete_correlated_evidence_earns_every_point() {
        let evidence = fixture();
        let evaluation = evaluate_evidence(&evidence, "test");
        assert_eq!(evaluation.completion, CompletionState::Completed);
        assert_eq!(
            evaluation
                .awards
                .iter()
                .map(|a| u16::from(a.awarded.unwrap()))
                .sum::<u16>(),
            100
        );
    }

    #[test]
    fn emitted_spawns_or_reused_sessions_do_not_prove_two_children() {
        let mut evidence = fixture();
        evidence["transcript"]["messages"][1]["message"]["is_error"] = json!(true);
        assert_eq!(points(&evidence, "explicit_spawns"), 0);
        let mut evidence = fixture();
        evidence["transcript"]["messages"][1]["message"]["details"]["reused"] = json!(true);
        assert_eq!(points(&evidence, "explicit_spawns"), 0);
        let mut evidence = fixture();
        evidence["children"]["analyst"]["record"]["parent"]["function_call_id"] =
            json!("wrong-call");
        assert_eq!(points(&evidence, "explicit_spawns"), 0);
        let mut evidence = fixture();
        evidence["tree"]
            .as_array_mut()
            .unwrap()
            .push(json!({"session_id":"extra","depth":2}));
        assert_eq!(points(&evidence, "explicit_spawns"), 0);
    }

    #[test]
    fn resolved_configuration_must_match_even_when_spawn_arguments_are_correct() {
        for (path, value) in [
            ("/model", json!("other-model")),
            ("/provider", json!("other-provider")),
            ("/max_turns", json!(99)),
            ("/max_output_tokens", json!(8192)),
            ("/max_total_tokens", json!(999999)),
            ("/thinking_level", json!("high")),
            ("/max_validation_retries", json!(10)),
            ("/output/type", json!("text")),
            ("/metadata/fs_scope/root", json!("/")),
            ("/functions/allow", json!(["*"])),
            ("/functions/deny", json!([])),
        ] {
            let mut evidence = fixture();
            *evidence["children"]["analyst"]["record"]["options"]
                .pointer_mut(path)
                .unwrap() = value;
            assert_eq!(points(&evidence, "effective_configuration"), 0, "{path}");
            assert_eq!(points(&evidence, "explicit_spawns"), 20, "{path}");
        }
    }

    #[test]
    fn a_profile_id_alone_does_not_prove_skill_loading_or_use() {
        for path in ["/skill_context/filter", "/system_prompt", "/agent/id"] {
            let mut evidence = fixture();
            *evidence["children"]["auditor"]["record"]["options"]
                .pointer_mut(path)
                .unwrap() = Value::Null;
            assert_eq!(points(&evidence, "profiles_and_skills"), 0, "{path}");
        }
        let mut evidence = fixture();
        evidence["children"]["auditor"]["record"]["result"]["skill_marker"] = json!("guessed");
        assert_eq!(points(&evidence, "profiles_and_skills"), 0);
    }

    #[test]
    fn results_require_real_child_reads_and_correct_work() {
        let mut evidence = fixture();
        evidence["children"]["analyst"]["transcript"]["messages"][1]["message"]
            ["function_call_id"] = json!("unrelated");
        assert_eq!(points(&evidence, "child_results"), 0);
        let mut evidence = fixture();
        evidence["children"]["analyst"]["record"]["result"]["result"]["software"] = json!(100);
        assert_eq!(points(&evidence, "child_results"), 0);
        let mut evidence = fixture();
        evidence["children"]["analyst"]["record"]["stop_reason"] = json!("max_turns");
        assert_eq!(points(&evidence, "child_results"), 0);
    }

    #[test]
    fn consolidation_cannot_be_fabricated_before_child_completion() {
        let mut evidence = fixture();
        evidence["transcript"]["messages"][3]["message"]["details"]["status"] = json!("running");
        assert_eq!(points(&evidence, "parent_consolidation"), 0);
        let mut evidence = fixture();
        evidence["children"]["analyst"] = Value::Null;
        assert_eq!(points(&evidence, "child_results"), 0);
        assert_eq!(points(&evidence, "parent_consolidation"), 0);
        assert_eq!(
            evaluate_evidence(&evidence, "test").completion,
            CompletionState::TaskIncomplete
        );
    }
}
