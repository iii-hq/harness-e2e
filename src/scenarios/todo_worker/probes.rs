use super::contracts::bounded_text;
use super::evidence::{bounded_value, outcome};
use super::*;
pub(super) async fn validate_compose(
    client: &IIIClient,
    contract: &TodoTaskContract,
) -> Result<(Value, ProbeOutcome)> {
    let path = Path::new(&contract.workspace_root).join("worker-compose.yaml");
    let local = fs::read_to_string(&path).ok();
    if local.is_none() {
        return Ok((
            json!({
                "compose_present": false,
                "name_exact": false,
                "runtime_exec": false,
                "stack_present": false
            }),
            ProbeOutcome::Failed,
        ));
    }
    let response = trigger_value(
        client,
        "compose::validate",
        json!({"path": path, "stack": contract.worker_name}),
    )
    .await?;
    let yaml = local
        .as_deref()
        .and_then(|text| serde_yaml::from_str::<Value>(text).ok());
    let worker = yaml
        .as_ref()
        .and_then(|value| value.get("workers"))
        .and_then(|value| value.get(&contract.worker_name));
    let name_ok = worker.is_some();
    let source_ok = worker
        .and_then(|value| value.pointer("/source/path"))
        .and_then(Value::as_str)
        == Some(".");
    let runtime_exec = worker
        .and_then(|value| value.pointer("/runtime/exec"))
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty() && values.iter().all(Value::is_string));
    let expected_ref = format!("catalog://{}", contract.worker_name);
    let stack_present = yaml
        .as_ref()
        .and_then(|value| value.get("stacks"))
        .and_then(|value| value.get(&contract.worker_name))
        .and_then(|value| value.get("containers"))
        .and_then(Value::as_object)
        .is_some_and(|containers| {
            containers.values().any(|container| {
                container.get("worker").and_then(Value::as_str) == Some(expected_ref.as_str())
            })
        });
    let remotely_valid = response.get("namespace").and_then(Value::as_str).is_some()
        && response
            .get("start_order")
            .and_then(Value::as_array)
            .is_some_and(|order| !order.is_empty());
    let valid = remotely_valid && name_ok && source_ok && runtime_exec && stack_present;
    Ok((
        json!({
            "compose_validate": bounded_value(&response),
            "compose_present": local.is_some(),
            "name_exact": name_ok,
            "source_path": source_ok,
            "runtime_exec": runtime_exec,
            "stack_present": stack_present
        }),
        outcome(valid),
    ))
}

pub(super) async fn install_and_wait(
    client: &IIIClient,
    contract: &TodoTaskContract,
) -> Result<(Value, bool)> {
    let initial = trigger_value(
        client,
        "worker::status",
        json!({"name": contract.worker_name}),
    )
    .await;
    if let Ok(status) = &initial {
        if status.get("installed").and_then(Value::as_bool) == Some(true)
            && status.get("running").and_then(Value::as_bool) == Some(true)
        {
            return Ok((
                json!({"already_live": true, "status": bounded_value(status)}),
                true,
            ));
        }
        if worker_mechanism_unavailable_in_status(status) {
            bail!(
                "Todo worker validation mechanism is unavailable: {}",
                bounded_value(status)
            );
        }
    } else if initial
        .as_ref()
        .err()
        .is_some_and(|error| !is_remote_invocation_failure(error))
    {
        return Err(initial.expect_err("checked as error"));
    }

    let add = trigger_value(
        client,
        "compose::up",
        json!({
            "path": Path::new(&contract.workspace_root).join("worker-compose.yaml"),
            "stack": contract.worker_name,
            "wait": false
        }),
    )
    .await;
    let add = match add {
        Ok(value) => value,
        Err(error) => {
            if worker_mechanism_unavailable_in_error(&error) {
                return Err(error.context("Todo worker validation mechanism is unavailable"));
            }
            if !is_remote_invocation_failure(&error) {
                return Err(error);
            }
            return Ok((
                json!({
                    "initial_status": initial.ok().map(|value| bounded_value(&value)),
                    "add_error": bounded_text(&error.to_string(), 1024)
                }),
                false,
            ));
        }
    };

    let mut last_status = Value::Null;
    for _ in 0..120 {
        match trigger_value(
            client,
            "worker::status",
            json!({"name": contract.worker_name}),
        )
        .await
        {
            Ok(status) => {
                let running = status.get("installed").and_then(Value::as_bool) == Some(true)
                    && status.get("running").and_then(Value::as_bool) == Some(true);
                last_status = status;
                if running {
                    return Ok((
                        json!({"worker_add": bounded_value(&add), "status": bounded_value(&last_status)}),
                        true,
                    ));
                }
                if worker_mechanism_unavailable_in_status(&last_status) {
                    bail!(
                        "Todo worker validation mechanism is unavailable: {}",
                        bounded_value(&last_status)
                    );
                }
            }
            Err(error) => {
                if worker_mechanism_unavailable_in_error(&error) {
                    return Err(error.context("Todo worker validation mechanism is unavailable"));
                }
                if !is_remote_invocation_failure(&error) {
                    return Err(error);
                }
                last_status = json!({"status_error": bounded_text(&error.to_string(), 1024)});
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok((
        json!({"worker_add": bounded_value(&add), "last_status": bounded_value(&last_status), "polls": 120}),
        false,
    ))
}

pub(crate) fn worker_mechanism_unavailable_in_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| worker_mechanism_unavailable_text(&cause.to_string()))
}

pub(crate) fn worker_mechanism_unavailable_in_status(status: &Value) -> bool {
    worker_mechanism_unavailable_text(&status.to_string())
}

pub(crate) fn worker_mechanism_unavailable_text(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("kvm not accessible")
        || normalized.contains("/dev/kvm") && normalized.contains("permission")
        || normalized.contains("project lock busy")
        || normalized.contains("worker operation is active")
}

pub(super) async fn validate_function_surface(
    client: &IIIClient,
    contract: &TodoTaskContract,
) -> Result<(bool, Value, BTreeMap<String, String>)> {
    let ids = contract.function_ids.values().cloned().collect::<Vec<_>>();
    let info = trigger_value(
        client,
        "engine::functions::info",
        json!({"function_ids": ids}),
    )
    .await?;
    let functions = info
        .get("functions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut observations = Vec::new();
    let mut hashes = BTreeMap::new();
    let mut all_ok = true;
    for (operation, function_id) in &contract.function_ids {
        let expected = &contract.request_response_schemas[operation];
        let observed = functions.iter().find(|function| {
            function.get("function_id").and_then(Value::as_str) == Some(function_id.as_str())
        });
        let (present, description, request_hash, response_hash) = if let Some(function) = observed {
            let description = function
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            let request_hash = function
                .get("request_schema")
                .filter(|value| value.is_object())
                .and_then(|value| crate::artifact::sha256_value(value).ok());
            let response_hash = function
                .get("response_schema")
                .filter(|value| value.is_object())
                .and_then(|value| crate::artifact::sha256_value(value).ok());
            (true, description, request_hash, response_hash)
        } else {
            (false, false, None, None)
        };
        let expected_request = crate::artifact::sha256_value(&expected.request)?;
        let expected_response = crate::artifact::sha256_value(&expected.response)?;
        if let Some(hash) = &request_hash {
            hashes.insert(format!("{operation}.request"), hash.clone());
        }
        if let Some(hash) = &response_hash {
            hashes.insert(format!("{operation}.response"), hash.clone());
        }
        let ok = present
            && description
            && request_hash.as_deref() == Some(expected_request.as_str())
            && response_hash.as_deref() == Some(expected_response.as_str());
        all_ok &= ok;
        observations.push(json!({
            "operation": operation,
            "function_id": function_id,
            "present": present,
            "description": description,
            "request_sha256": request_hash,
            "response_sha256": response_hash,
            "expected_request_sha256": expected_request,
            "expected_response_sha256": expected_response,
            "passed": ok
        }));
    }
    Ok((all_ok, json!({"functions": observations}), hashes))
}

pub(super) async fn run_crud_cycle(
    client: &IIIClient,
    contract: &TodoTaskContract,
    repetition: u8,
) -> Result<(bool, Value)> {
    let create = contract.function_id("create")?;
    let list = contract.function_id("list")?;
    let update = contract.function_id("update")?;
    let delete = contract.function_id("delete")?;
    let baseline = trigger_value(client, list, json!({})).await?;
    let baseline_items = todo_map(&baseline);
    let title_a = format!("e2e-{}-{repetition}-a", contract.worker_name);
    let title_b = format!("e2e-{}-{repetition}-b", contract.worker_name);
    let mut created_ids = Vec::new();
    let result = async {
        let first = trigger_value(client, create, json!({"title": title_a})).await?;
        let second = trigger_value(client, create, json!({"title": title_b})).await?;
        let first_todo = first
            .get("todo")
            .cloned()
            .context("create A response omits todo")?;
        let second_todo = second
            .get("todo")
            .cloned()
            .context("create B response omits todo")?;
        let first_id = todo_id(&first_todo)?.to_string();
        let second_id = todo_id(&second_todo)?.to_string();
        created_ids.extend([first_id.clone(), second_id.clone()]);
        let created_ok = first_id != second_id
            && todo_title(&first_todo) == Some(title_a.as_str())
            && todo_title(&second_todo) == Some(title_b.as_str())
            && first_todo.get("completed").and_then(Value::as_bool) == Some(false)
            && second_todo.get("completed").and_then(Value::as_bool) == Some(false);
        let after_create = trigger_value(client, list, json!({})).await?;
        let after_create_map = todo_map(&after_create);
        let listed =
            after_create_map.contains_key(&first_id) && after_create_map.contains_key(&second_id);
        let updated =
            trigger_value(client, update, json!({"id": first_id, "completed": true})).await?;
        let updated_todo = updated
            .get("todo")
            .cloned()
            .context("update response omits todo")?;
        let update_ok = todo_id(&updated_todo)? == first_id
            && updated_todo.get("completed").and_then(Value::as_bool) == Some(true);
        let after_update = trigger_value(client, list, json!({})).await?;
        let after_update_map = todo_map(&after_update);
        let second_unchanged = after_update_map
            .get(&second_id)
            .and_then(|todo| todo.get("completed"))
            .and_then(Value::as_bool)
            == Some(false);
        let deleted_first = trigger_value(client, delete, json!({"id": first_id})).await?;
        let after_delete = trigger_value(client, list, json!({})).await?;
        let after_delete_map = todo_map(&after_delete);
        let delete_ok = deleted_first.get("deleted").and_then(Value::as_bool) == Some(true)
            && !after_delete_map.contains_key(&first_id)
            && after_delete_map.contains_key(&second_id);
        let deleted_second = trigger_value(client, delete, json!({"id": second_id})).await?;
        let final_list = trigger_value(client, list, json!({})).await?;
        let final_map = todo_map(&final_list);
        let final_clean = deleted_second.get("deleted").and_then(Value::as_bool) == Some(true)
            && !final_map.contains_key(&second_id);
        let baseline_preserved = baseline_items
            .iter()
            .all(|(id, todo)| final_map.get(id) == Some(todo));
        let ok = created_ok
            && listed
            && update_ok
            && second_unchanged
            && delete_ok
            && final_clean
            && baseline_preserved;
        Ok::<_, anyhow::Error>((
            ok,
            json!({
                "created_ids": created_ids,
                "created_ok": created_ok,
                "listed_after_create": listed,
                "updated_same_id": update_ok,
                "unrelated_item_unchanged": second_unchanged,
                "delete_removed_only_target": delete_ok,
                "probe_items_removed": final_clean,
                "baseline_items_preserved": baseline_preserved
            }),
        ))
    }
    .await;
    for id in &created_ids {
        let _ = trigger_value(client, delete, json!({"id": id})).await;
    }
    result
}

pub(super) async fn run_concurrent_create(
    client: &IIIClient,
    contract: &TodoTaskContract,
    concurrency: u8,
) -> Result<(bool, Value)> {
    if concurrency == 0 || concurrency > 5 {
        bail!("Todo concurrent-create width must be between one and five");
    }
    let create = contract.function_id("create")?.to_string();
    let list = contract.function_id("list")?.to_string();
    let delete = contract.function_id("delete")?.to_string();
    let baseline = trigger_value(client, &list, json!({})).await;
    let baseline_items = baseline.as_ref().map(todo_map).unwrap_or_default();
    let titles = (0..concurrency)
        .map(|index| format!("e2e-{}-concurrent-{index}", contract.worker_name))
        .collect::<Vec<_>>();
    let responses = join_all(titles.iter().cloned().map(|title| {
        let create = create.clone();
        async move {
            let result = trigger_value(client, &create, json!({"title": title})).await;
            (title, result)
        }
    }))
    .await;

    let mut ids = Vec::new();
    let mut response_facts = Vec::new();
    let mut all_shapes_valid = true;
    for (title, response) in responses {
        match response {
            Ok(value) => {
                let todo = value.get("todo");
                let id = todo.and_then(|todo| todo.get("id")).and_then(Value::as_str);
                let valid = id.is_some_and(|id| !id.trim().is_empty())
                    && todo
                        .and_then(|todo| todo.get("title"))
                        .and_then(Value::as_str)
                        == Some(title.as_str())
                    && todo
                        .and_then(|todo| todo.get("completed"))
                        .and_then(Value::as_bool)
                        == Some(false);
                if let Some(id) = id {
                    ids.push(id.to_string());
                }
                all_shapes_valid &= valid;
                response_facts.push(json!({"title": title, "id": id, "valid": valid}));
            }
            Err(error) => {
                all_shapes_valid = false;
                response_facts.push(json!({
                    "title": title,
                    "error": bounded_text(&error.to_string(), 512)
                }));
            }
        }
    }
    let unique = ids.iter().collect::<BTreeSet<_>>().len() == usize::from(concurrency);
    let after_create = trigger_value(client, &list, json!({})).await;
    let all_listed = after_create.as_ref().is_ok_and(|value| {
        let items = todo_map(value);
        ids.iter().all(|id| items.contains_key(id))
    });
    for id in &ids {
        let _ = trigger_value(client, &delete, json!({"id": id})).await;
    }
    let final_list = trigger_value(client, &list, json!({})).await;
    let cleanup_complete = final_list.as_ref().is_ok_and(|value| {
        let items = todo_map(value);
        ids.iter().all(|id| !items.contains_key(id))
            && baseline_items
                .iter()
                .all(|(id, todo)| items.get(id) == Some(todo))
    });
    let passed = all_shapes_valid
        && ids.len() == usize::from(concurrency)
        && unique
        && all_listed
        && cleanup_complete;
    Ok((
        passed,
        json!({
            "responses": response_facts,
            "requested": concurrency,
            "returned_ids": ids.len(),
            "unique_ids": unique,
            "all_listed": all_listed,
            "probe_items_removed": cleanup_complete,
            "baseline_readable": baseline.is_ok(),
            "final_list_readable": final_list.is_ok()
        }),
    ))
}

pub(super) async fn validate_invalid_inputs(
    client: &IIIClient,
    contract: &TodoTaskContract,
) -> Result<(bool, Value, bool)> {
    let empty = rejection_observation(
        client,
        contract.function_id("create")?,
        json!({"title": ""}),
    )
    .await?;
    let unknown = format!("missing-{}", contract.worker_name);
    let update = rejection_observation(
        client,
        contract.function_id("update")?,
        json!({"id": unknown, "completed": true}),
    )
    .await?;
    let delete = rejection_observation(
        client,
        contract.function_id("delete")?,
        json!({"id": unknown}),
    )
    .await?;
    let empty_rejected = empty.0;
    let update_rejected = update.0;
    let delete_rejected = delete.0;
    Ok((
        empty_rejected && update_rejected && delete_rejected,
        json!({
            "empty_title_rejected": empty_rejected,
            "unknown_update_rejected": update_rejected,
            "unknown_delete_rejected": delete_rejected,
            "observations": {
                "empty_title": empty.1,
                "unknown_update": update.1,
                "unknown_delete": delete.1
            }
        }),
        false,
    ))
}

async fn rejection_observation(
    client: &IIIClient,
    function_id: &str,
    payload: Value,
) -> Result<(bool, Value)> {
    match trigger_value(client, function_id, payload).await {
        Ok(value) => {
            let rejected = value.get("error").is_some()
                || value.get("success").and_then(Value::as_bool) == Some(false)
                || value.get("deleted").and_then(Value::as_bool) == Some(false);
            Ok((rejected, bounded_value(&value)))
        }
        Err(error) if is_remote_invocation_failure(&error) => Ok((
            true,
            json!({"remote_rejection": bounded_text(&error.to_string(), 512)}),
        )),
        Err(error) => Err(error),
    }
}

async fn trigger_value(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    client
        .trigger(TriggerRequest {
            function_id: function_id.into(),
            payload,
            action: None,
            timeout_ms: Some(60_000),
        })
        .await
        .map_err(anyhow::Error::new)
        .with_context(|| format!("invoke {function_id}"))
}

pub(crate) fn is_remote_invocation_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Remote { .. })
    )
}

fn todo_map(response: &Value) -> BTreeMap<String, Value> {
    response
        .get("todos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|todo| Some((todo.get("id")?.as_str()?.to_string(), todo.clone())))
        .collect()
}

fn todo_id(todo: &Value) -> Result<&str> {
    todo.get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("Todo object has no non-empty id")
}

fn todo_title(todo: &Value) -> Option<&str> {
    todo.get("title").and_then(Value::as_str)
}
