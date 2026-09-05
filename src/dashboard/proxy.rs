use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message as AxumMessage, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame as TungCloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message as TungMessage;

use super::bus::{
    BROWSER_FUNCTION_PREFIX, CATALOG_GET, CHANGED_TRIGGER, EVALUATED_VERSIONS_LIST,
    EXECUTIONS_LIST, EXECUTION_GET, LOCAL_SCENARIO_CREATE, PLANS_LIST, PLAN_CONTROL, PLAN_CREATE,
    PLAN_GET, PLAN_RUN_START, PLAN_UPDATE, RUN_CANCEL, RUN_START, RUN_STATUS, TESTS_LIST,
    TEST_HISTORY_GET, TEST_VERSION_GET,
};

#[derive(Default)]
struct BrowserPolicy {
    functions: HashSet<String>,
    triggers: HashSet<String>,
}

pub(super) async fn ws_proxy(
    ws: WebSocketUpgrade,
    State(engine_url): State<Arc<String>>,
) -> Response {
    ws.max_message_size(usize::MAX)
        .max_frame_size(usize::MAX)
        .on_upgrade(move |socket| handle_ws(socket, engine_url))
}

async fn handle_ws(client: WebSocket, engine_url: Arc<String>) {
    let config = WebSocketConfig {
        max_message_size: None,
        max_frame_size: None,
        ..Default::default()
    };
    let (engine, _) = match tokio_tungstenite::connect_async_with_config(
        engine_url.as_str(),
        Some(config),
        false,
    )
    .await
    {
        Ok(pair) => pair,
        Err(error) => {
            tracing::warn!(%error, "dashboard browser could not connect to iii");
            let mut client = client;
            let _ = client
                .send(AxumMessage::Close(Some(AxumCloseFrame {
                    code: 1011,
                    reason: "iii connection failed".into(),
                })))
                .await;
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client.split();
    let (mut engine_tx, mut engine_rx) = engine.split();
    let client_to_engine = async {
        let mut policy = BrowserPolicy::default();
        while let Some(message) = client_rx.next().await {
            let Ok(message) = message else { break };
            let close = matches!(message, AxumMessage::Close(_));
            let message = match message {
                AxumMessage::Text(text) => match filter_browser_text(&text, &mut policy) {
                    Some(text) => AxumMessage::Text(text),
                    None => continue,
                },
                AxumMessage::Binary(_) => continue,
                other => other,
            };
            if let Some(message) = axum_to_tungstenite(message) {
                if engine_tx.send(message).await.is_err() {
                    break;
                }
            }
            if close {
                break;
            }
        }
        let _ = engine_tx.close().await;
    };
    let engine_to_client = async {
        while let Some(message) = engine_rx.next().await {
            let Ok(message) = message else { break };
            let close = matches!(message, TungMessage::Close(_));
            if let Some(message) = tungstenite_to_axum(message) {
                if client_tx.send(message).await.is_err() {
                    break;
                }
            }
            if close {
                break;
            }
        }
        let _ = client_tx.close().await;
    };
    tokio::select! {
        _ = client_to_engine => {}
        _ = engine_to_client => {}
    }
}

fn filter_browser_text(text: &str, policy: &mut BrowserPolicy) -> Option<String> {
    let Ok(mut message) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let kind = message
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "registertriggertype" | "unregistertriggertype" => None,
        "invokefunction" => {
            let id = message
                .get("function_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            allowed_invocation(id).then(|| text.to_string())
        }
        "registerfunction" => {
            let id = message.get("id").and_then(Value::as_str)?.to_string();
            if !id.starts_with(BROWSER_FUNCTION_PREFIX) {
                return None;
            }
            policy.functions.insert(id);
            stamp_internal(&mut message);
            serde_json::to_string(&message).ok()
        }
        "unregisterfunction" => {
            let id = message.get("id").and_then(Value::as_str)?;
            policy.functions.remove(id).then(|| text.to_string())
        }
        "registertrigger" => {
            let id = message.get("id").and_then(Value::as_str)?.to_string();
            let trigger_type = message.get("trigger_type").and_then(Value::as_str)?;
            let function_id = message.get("function_id").and_then(Value::as_str)?;
            if trigger_type != CHANGED_TRIGGER
                || !function_id.starts_with(BROWSER_FUNCTION_PREFIX)
                || !policy.functions.contains(function_id)
            {
                return None;
            }
            policy.triggers.insert(id);
            Some(text.to_string())
        }
        "unregistertrigger" => {
            let id = message.get("id").and_then(Value::as_str)?;
            policy.triggers.remove(id).then(|| text.to_string())
        }
        "invocationresult" | "ping" | "pong" | "reattach" => Some(text.to_string()),
        _ => None,
    }
}

fn allowed_invocation(id: &str) -> bool {
    matches!(
        id,
        EXECUTIONS_LIST
            | EXECUTION_GET
            | EVALUATED_VERSIONS_LIST
            | TESTS_LIST
            | TEST_VERSION_GET
            | TEST_HISTORY_GET
            | CATALOG_GET
            | LOCAL_SCENARIO_CREATE
            | PLANS_LIST
            | PLAN_GET
            | PLAN_CREATE
            | PLAN_UPDATE
            | PLAN_RUN_START
            | PLAN_CONTROL
            | RUN_STATUS
            | RUN_START
            | RUN_CANCEL
    )
}

fn stamp_internal(message: &mut Value) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    match object.get_mut("metadata") {
        Some(Value::Object(metadata)) => {
            metadata.insert("internal".into(), Value::Bool(true));
        }
        Some(_) => {}
        None => {
            object.insert("metadata".into(), serde_json::json!({ "internal": true }));
        }
    }
}

fn axum_to_tungstenite(message: AxumMessage) -> Option<TungMessage> {
    Some(match message {
        AxumMessage::Text(text) => TungMessage::Text(text),
        AxumMessage::Binary(bytes) => TungMessage::Binary(bytes),
        AxumMessage::Ping(bytes) => TungMessage::Ping(bytes),
        AxumMessage::Pong(bytes) => TungMessage::Pong(bytes),
        AxumMessage::Close(frame) => TungMessage::Close(frame.map(|frame| TungCloseFrame {
            code: frame.code.into(),
            reason: frame.reason.into_owned().into(),
        })),
    })
}

fn tungstenite_to_axum(message: TungMessage) -> Option<AxumMessage> {
    Some(match message {
        TungMessage::Text(text) => AxumMessage::Text(text),
        TungMessage::Binary(bytes) => AxumMessage::Binary(bytes),
        TungMessage::Ping(bytes) => AxumMessage::Ping(bytes),
        TungMessage::Pong(bytes) => AxumMessage::Pong(bytes),
        TungMessage::Close(frame) => AxumMessage::Close(frame.map(|frame| AxumCloseFrame {
            code: u16::from(frame.code),
            reason: frame.reason.into_owned().into(),
        })),
        TungMessage::Frame(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allows_only_dashboard_invocations() {
        let mut policy = BrowserPolicy::default();
        let allowed =
            format!(r#"{{"type":"invokefunction","function_id":"{EXECUTIONS_LIST}","data":{{}}}}"#);
        assert!(filter_browser_text(&allowed, &mut policy).is_some());
        for function_id in [
            TEST_HISTORY_GET,
            PLANS_LIST,
            PLAN_GET,
            PLAN_CREATE,
            PLAN_UPDATE,
            PLAN_RUN_START,
            PLAN_CONTROL,
            LOCAL_SCENARIO_CREATE,
        ] {
            let allowed =
                format!(r#"{{"type":"invokefunction","function_id":"{function_id}","data":{{}}}}"#);
            assert!(
                filter_browser_text(&allowed, &mut policy).is_some(),
                "{function_id} should be allowed"
            );
        }
        assert!(filter_browser_text(
            r#"{"type":"invokefunction","function_id":"shell::exec","data":{}}"#,
            &mut policy,
        )
        .is_none());
        assert!(filter_browser_text(
            r#"{"type":"invokefunction","function_id":"e2e::dashboard::profile-plan","data":{}}"#,
            &mut policy,
        )
        .is_none());
    }

    #[test]
    fn scopes_browser_handlers_and_event_subscriptions() {
        let mut policy = BrowserPolicy::default();
        let handler = format!("{BROWSER_FUNCTION_PREFIX}changed::browser-1");
        let register = format!(r#"{{"type":"registerfunction","id":"{handler}"}}"#);
        let stamped = filter_browser_text(&register, &mut policy).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stamped).unwrap()["metadata"]["internal"],
            json!(true),
        );
        let trigger = format!(
            r#"{{"type":"registertrigger","id":"trigger-1","trigger_type":"{CHANGED_TRIGGER}","function_id":"{handler}","config":{{}}}}"#
        );
        assert!(filter_browser_text(&trigger, &mut policy).is_some());
        let foreign_trigger = format!(
            r#"{{"type":"registertrigger","id":"trigger-2","trigger_type":"{CHANGED_TRIGGER}","function_id":"{BROWSER_FUNCTION_PREFIX}changed::other","config":{{}}}}"#
        );
        assert!(filter_browser_text(&foreign_trigger, &mut policy).is_none());
        assert!(
            filter_browser_text(r#"{"type":"unregistertrigger","id":"other"}"#, &mut policy,)
                .is_none()
        );
        assert!(filter_browser_text(
            r#"{"type":"unregistertriggertype","id":"e2e::dashboard::changed"}"#,
            &mut policy,
        )
        .is_none());
    }
}
