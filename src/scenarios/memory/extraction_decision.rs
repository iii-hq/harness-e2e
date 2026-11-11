//! Attempt-local terminal skip witness; absence is infrastructure, never success.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iii_sdk::{ObserverHandle, ObserverState, RegisterFunction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::context::E2eContext;

pub const SOURCE: &str = "memory::on-extraction-decision";
const BOUND: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub version: u8,
    pub session_id: String,
    pub turn_id: String,
    pub status: String,
    pub reason: String,
    pub session_kind: String,
    pub producer_version: String,
}

fn id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256
}

// SemVer 2.0 without accepting a leading v, missing components or numeric
// prerelease leading zeros; a producer version is compatibility, NOT authority.
fn semver(value: &str) -> bool {
    if value.len() > 256 || !value.is_ascii() {
        return false;
    }
    let (version, build) = value
        .split_once('+')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let identifiers = |s: &str, numeric: bool| {
        !s.is_empty()
            && s.split('.').all(|p| {
                !p.is_empty()
                    && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && (!numeric
                        || !p.bytes().all(|b| b.is_ascii_digit())
                        || p.len() == 1
                        || !p.starts_with('0'))
            })
    };
    if build.is_some_and(|s| !identifiers(s, false)) {
        return false;
    }
    let (core, pre) = version
        .split_once('-')
        .map_or((version, None), |(a, b)| (a, Some(b)));
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
        && pre.is_none_or(|s| identifiers(s, true))
}

impl Decision {
    fn parse(raw: Value) -> Result<Self> {
        let decision: Self =
            serde_json::from_value(raw).context("invalid extraction decision schema")?;
        if decision.version != 1
            || !id(&decision.session_id)
            || !id(&decision.turn_id)
            || decision.status != "skipped"
            || decision.reason != "session_kind_not_user"
            || !matches!(decision.session_kind.as_str(), "automation" | "e2e")
            || !semver(&decision.producer_version)
        {
            bail!("invalid terminal skip contract");
        }
        Ok(decision)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bank {
    pub memories: Value,
    pub rules: Value,
}

impl Bank {
    fn checked(memories: Value, rules: Value) -> Result<Self> {
        let rows = memories["memories"]
            .as_array()
            .context("bank list has no memories")?;
        // A full finite read, not a truncated page or an empty malformed reply.
        if memories["has_more"] == true
            || !memories["next_cursor"].is_null()
            || rules["has_more"] == true
            || !rules["next_cursor"].is_null()
        {
            bail!("bank read is paginated");
        }
        if rows.len() >= 500
            || rows
                .iter()
                .any(|m| !m["id"].is_string() || !m["text"].is_string())
        {
            bail!("bank list incomplete or invalid");
        }
        let entries = rules["rules"]
            .as_array()
            .context("bank list has no rules")?;
        if entries
            .iter()
            .any(|r| !r["name"].is_string() || !r["content"].is_string())
        {
            bail!("rule list invalid");
        }
        if memories
            .get("total")
            .is_some_and(|n| n.as_u64() != Some(rows.len() as u64))
            || rules
                .get("total")
                .is_some_and(|n| n.as_u64() != Some(entries.len() as u64))
        {
            bail!("bank count does not match complete read");
        }
        Ok(Self {
            memories: Value::Array(rows.clone()),
            rules: Value::Array(entries.clone()),
        })
    }

    pub fn unchanged(&self, after: &Self) -> bool {
        let canonical = |rows: &Value| {
            let mut rows: Vec<_> = rows
                .as_array()
                .unwrap()
                .iter()
                .map(Value::to_string)
                .collect();
            rows.sort();
            rows
        };
        canonical(&self.memories) == canonical(&after.memories)
            && canonical(&self.rules) == canonical(&after.rules)
    }
}

pub async fn read_bank(context: &E2eContext, bank: &str) -> Result<Bank> {
    let memories = context
        .trigger_value(
            "memory::list",
            json!({"bank":bank,"include_superseded":true,"limit":500}),
        )
        .await?;
    let rules = context
        .trigger_value("memory::rule::list", json!({"bank":bank}))
        .await?;
    Bank::checked(memories, rules)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub decision: Decision,
    pub before_teacher: Bank,
    pub after_skip: Bank,
}

#[derive(Debug, Clone, PartialEq)]
struct Owner {
    id: String,
    // Actual stable descriptor and connection identity, not live counts.
    // No registration epoch exists in this API: an identical-descriptor hot
    // reload on the same authorized connection cannot be distinguished.
    registration: Value,
    worker: Value,
}

async fn owner(context: &E2eContext) -> Result<Owner> {
    let namespace = context
        .client()
        .namespace()
        .unwrap_or_else(|| "default".into());
    let info = context
        .trigger_value(
            "engine::functions::info",
            json!({"function_id":SOURCE,"namespace":namespace}),
        )
        .await?;
    // FunctionDetail exposes schemas/owner/description, not the SDK's
    // FunctionInfo.private flag. Do not fabricate that missing API field.
    // The pinned descriptor is declared .private() in Memory events.rs;
    // authority still requires the engine-stamped owner and own activation.
    if info["function_id"] != SOURCE
        || info["namespace"] != namespace
        || info.get("private").is_some_and(|value| value != true)
    {
        bail!("private extraction source unavailable");
    }
    let name = info["worker_name"]
        .as_str()
        .context("source has no host")?
        .to_owned();
    let worker = context
        .trigger_value(
            "engine::workers::info",
            json!({"name":name,"namespace":namespace}),
        )
        .await?;
    let w = &worker["worker"];
    let worker_id = w["id"]
        .as_str()
        .filter(|s| id(s))
        .context("source host has no id")?
        .to_owned();
    if !id(&name)
        || w["namespace"] != namespace
        || w["status"] != "connected"
        || !worker["functions"].as_array().is_some_and(|fs| {
            fs.iter().any(|f| {
                f["function_id"] == SOURCE
                    && f["namespace"] == namespace
                    && f["worker_name"] == name
            })
        })
    {
        bail!("source host is not current and connected");
    }
    let connected_at = w["connected_at_ms"]
        .as_u64()
        .filter(|n| *n > 0)
        .context("source connection timestamp missing/malformed")?;
    for key in ["request_schema", "response_schema"] {
        let schema = &info[key];
        let required = schema["required"]
            .as_array()
            .context("source schema required fields missing")?;
        if schema["type"] != "object"
            || schema["additionalProperties"] != false
            || [
                "version",
                "session_id",
                "turn_id",
                "status",
                "reason",
                "session_kind",
                "producer_version",
            ]
            .iter()
            .any(|field| !required.iter().any(|value| value == field))
            || schema["properties"]["version"]["minimum"].as_f64() != Some(1.0)
            || schema["properties"]["version"]["maximum"].as_f64() != Some(1.0)
        {
            bail!("source schema/version mismatch");
        }
    }
    for key in ["request_schema", "response_schema"] {
        let schema = &info[key];
        for field in ["session_id", "turn_id", "producer_version"] {
            if schema["properties"][field]["type"] != "string" {
                bail!("source descriptor string field missing/malformed");
            }
        }
        for (field, values) in [
            ("status", json!(["skipped"])),
            ("reason", json!(["session_kind_not_user"])),
            ("session_kind", json!(["automation", "e2e"])),
        ] {
            let property = &schema["properties"][field];
            let resolved = property["$ref"]
                .as_str()
                .and_then(|r| r.strip_prefix('#'))
                .and_then(|pointer| schema.pointer(pointer))
                .unwrap_or(property);
            if resolved["type"] != "string" || resolved["enum"] != values {
                bail!("source descriptor enum mismatch");
            }
        }
    }
    // Pin only documented descriptor fields. Observer/trigger counts and
    // unrelated worker telemetry are neither stable nor authority.
    let registration = json!({"function_id":info["function_id"], "namespace":info["namespace"],
        "worker_name":info["worker_name"],
        "description":info["description"], "request_schema":info["request_schema"],
        "response_schema":info["response_schema"]});
    Ok(Owner {
        id: worker_id,
        registration,
        worker: json!({"id":w["id"],"namespace":w["namespace"],"connected_at_ms":connected_at}),
    })
}

async fn subscription(context: &E2eContext, handle: &ObserverHandle, target: &str) -> Result<()> {
    if handle.state() != ObserverState::Active {
        bail!("NOT_MEASURED skip observer inactive");
    }
    let namespace = context
        .client()
        .namespace()
        .unwrap_or_else(|| "default".into());
    let listed = context
        .trigger_value(
            "engine::observers::list",
            json!({"function_id":SOURCE,"namespace":namespace}),
        )
        .await?;
    let rows = listed["observers"]
        .as_array()
        .context("skip observer inventory unavailable")?;
    let own: Vec<_> = rows.iter().filter(|row| row["id"] == handle.id()).collect();
    if own.len() != 1
        || own[0]["function_id"] != SOURCE
        || own[0]["namespace"] != namespace
        || own[0]["target"]["function_id"] != target
        || own[0]["target"]["namespace"] != namespace
    {
        bail!("NOT_MEASURED skip subscription missing or wrong source/target");
    }
    Ok(())
}

#[derive(Default)]
struct Pending {
    armed: bool,
    turn: Option<String>,
    early: Vec<Decision>,
    accepted: Option<Decision>,
    issue: Option<String>,
    callbacks: usize,
}

pub struct Capture {
    pending: Arc<Mutex<Pending>>,
    notify: Arc<Notify>,
    handle: ObserverHandle,
    function: Option<iii_sdk::runtime::FunctionRef>,
    listener: Option<iii_sdk::observe::StateListener>,
    owner: Owner,
    target: String,
}

impl Capture {
    pub async fn bind(context: &E2eContext, session: &str, kind: &str) -> Result<Self> {
        if !id(session) || !matches!(kind, "automation" | "e2e") {
            bail!("invalid teacher identity");
        }
        let initial = owner(context).await?;
        let pending = Arc::new(Mutex::new(Pending::default()));
        let notify = Arc::new(Notify::new());
        let slot = Arc::new(Mutex::new(None::<ObserverHandle>));
        let target = format!("e2e::memory::skip::{}", uuid::Uuid::new_v4());
        let function = context.client().register_function(target.clone(), {
            let pending = pending.clone();
            let notify = notify.clone();
            let slot = slot.clone();
            let client = context.client().clone();
            let expected = initial.clone();
            let session = session.to_owned();
            let kind = kind.to_owned();
            let target = target.clone();
            RegisterFunction::new_async(move |raw: Value| {
                // SDK task-local context must be captured before constructing the future.
                let invocation = iii_sdk::current_invocation();
                let admitted = {
                    let mut p = pending.lock().unwrap();
                    if p.armed {
                        p.callbacks += 1;
                        if p.callbacks > 16 {
                            p.issue = Some("post-arm callback bound exceeded".into());
                        }
                    }
                    p.armed && p.issue.is_none()
                };
                let pending = pending.clone();
                let notify = notify.clone();
                let slot = slot.clone();
                let context = E2eContext::from_client(client.clone());
                let expected = expected.clone();
                let session = session.clone();
                let kind = kind.clone();
                let target = target.clone();
                async move {
                    let activation = invocation
                        .as_ref()
                        .and_then(|ctx| ctx.activation.as_ref())
                        .cloned();
                    let authentic = invocation.as_ref().is_some_and(|ctx| {
                        ctx.function_id == target
                            && id(&ctx.invocation_id)
                            && ctx.caller.as_ref().and_then(|c| c.worker_id.as_deref())
                                == Some(expected.id.as_str())
                            && ctx.activation.as_ref().is_some_and(|a| {
                                a.activation_id.as_deref().is_none_or(id)
                                    && slot.lock().unwrap().as_ref().is_some_and(|h| {
                                        h.id() == a.observer_id
                                            && h.state() == ObserverState::Active
                                    })
                            })
                    });
                    if !authentic || !admitted {
                        return Ok::<_, iii_sdk::errors::Error>(json!({"accepted":false}));
                    }
                    if raw["session_id"].as_str().is_some_and(|id| id != session) {
                        return Ok(json!({"accepted":false}));
                    }
                    let handle = slot.lock().unwrap().clone().expect("authenticated handle");
                    let active = subscription(&context, &handle, &target).await;
                    let current = owner(&context).await;
                    let decision = Decision::parse(raw);
                    let mut p = pending.lock().unwrap();
                    if !p.armed {
                        return Ok(json!({"accepted":false}));
                    }
                    // Never accept an old activation after asynchronous registry RPCs.
                    let still_active = activation.as_ref().is_some_and(|a| {
                        a.observer_id == handle.id() && handle.state() == ObserverState::Active
                    });
                    if !still_active || active.is_err() || current.as_ref().ok() != Some(&expected)
                    {
                        p.issue = Some("source owner/epoch changed or lookup unavailable".into());
                    }
                    let accepted = match decision {
                        Ok(d)
                            if d.session_id == session
                                && d.session_kind == kind
                                && p.issue.is_none() =>
                        {
                            if let Some(turn) = &p.turn {
                                if d.turn_id == *turn {
                                    if p.accepted.as_ref().is_some_and(|old| old != &d) {
                                        p.issue = Some(
                                            "conflicting decision for own session/turn".into(),
                                        );
                                        false
                                    } else {
                                        p.accepted = Some(d);
                                        true
                                    }
                                } else {
                                    false
                                }
                            } else if p.early.len() < 16 {
                                if !p.early.contains(&d) {
                                    p.early.push(d);
                                }
                                true
                            } else {
                                p.issue = Some("early callback bound exceeded".into());
                                false
                            }
                        }
                        Ok(_) => false,
                        Err(error) => {
                            p.issue = Some(error.to_string());
                            false
                        }
                    };
                    drop(p);
                    notify.notify_one();
                    Ok(json!({"accepted":accepted}))
                }
            })
            .description("Attempt-local authenticated extraction skip collector")
        });
        let handle = context.client().observe(
            iii_sdk::Source::function(SOURCE).deliver(iii_sdk::Deliver::Output),
            target.clone(),
        );
        *slot.lock().unwrap() = Some(handle.clone());
        let mut capture = Self {
            pending,
            notify,
            handle,
            function: Some(function),
            listener: None,
            owner: initial,
            target,
        };
        let pending = capture.pending.clone();
        let notify = capture.notify.clone();
        let was_active = Arc::new(std::sync::atomic::AtomicBool::new(
            capture.handle.state() == ObserverState::Active,
        ));
        capture.listener = Some(capture.handle.on_state_change(move |state, _| {
            use std::sync::atomic::Ordering;
            if *state == ObserverState::Active {
                was_active.store(true, Ordering::SeqCst);
            } else if was_active.load(Ordering::SeqCst) || pending.lock().unwrap().armed {
                pending.lock().unwrap().issue = Some("skip observer inactive/reconnected".into());
                notify.notify_one();
            }
        }));
        tokio::time::timeout(BOUND, capture.handle.ready())
            .await
            .context("skip observer activation timeout")??;
        subscription(context, &capture.handle, &capture.target).await?;
        if owner(context).await? != capture.owner {
            bail!("source changed during subscription");
        }
        if capture.pending.lock().unwrap().issue.is_some() {
            bail!("skip observer changed during activation");
        }
        Ok(capture)
    }

    pub fn arm(&self) {
        self.pending.lock().unwrap().armed = true;
    }

    pub fn turn(&self, turn: &str) -> Result<()> {
        if !id(turn) {
            bail!("teacher returned invalid turn id");
        }
        let mut p = self.pending.lock().unwrap();
        if p.turn.is_some() {
            bail!("skip collector cannot span turns");
        }
        p.turn = Some(turn.to_owned());
        let matching: Vec<_> = p
            .early
            .iter()
            .filter(|d| d.turn_id == turn)
            .cloned()
            .collect();
        if matching.windows(2).any(|pair| pair[0] != pair[1]) {
            p.issue = Some("conflicting early decision for own session/turn".into());
        }
        p.accepted = matching.first().cloned();
        p.early.clear();
        Ok(())
    }

    pub async fn wait_then_read(
        &self,
        context: &E2eContext,
        bank: &str,
        before: Bank,
    ) -> Result<Evidence> {
        self.wait_then_read_bounded(context, bank, before, BOUND)
            .await
    }

    async fn wait_then_read_bounded(
        &self,
        context: &E2eContext,
        bank: &str,
        before: Bank,
        bound: Duration,
    ) -> Result<Evidence> {
        let wait = async {
            loop {
                let notified = self.notify.notified();
                {
                    let p = self.pending.lock().unwrap();
                    if let Some(issue) = &p.issue {
                        bail!("NOT_MEASURED extraction skip: {issue}");
                    }
                    if self.handle.state() != ObserverState::Active {
                        bail!("NOT_MEASURED skip observer inactive");
                    }
                    if let Some(d) = &p.accepted {
                        break Ok::<_, anyhow::Error>(d.clone());
                    }
                }
                notified.await;
            }
        };
        let decision = tokio::time::timeout(bound, wait)
            .await
            .context("NOT_MEASURED terminal skip missing/lost")??;
        if owner(context).await? != self.owner {
            bail!("NOT_MEASURED source owner/epoch changed");
        }
        subscription(context, &self.handle, &self.target).await?;
        if self.pending.lock().unwrap().issue.is_some()
            || self.handle.state() != ObserverState::Active
        {
            bail!("NOT_MEASURED observer changed before bank read");
        }
        // No list may run until the actual correlated terminal skip arrived.
        let after_skip = read_bank(context, bank).await?;
        subscription(context, &self.handle, &self.target).await?;
        if owner(context).await? != self.owner
            || self.handle.state() != ObserverState::Active
            || self.pending.lock().unwrap().issue.is_some()
        {
            bail!("NOT_MEASURED source changed at bank read barrier");
        }
        Ok(Evidence {
            decision,
            before_teacher: before,
            after_skip,
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.unsubscribe();
        }
        crate::observe::release(self.handle.clone());
        if let Some(function) = self.function.take() {
            function.unregister();
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{mpsc, oneshot};
    use tokio_tungstenite::tungstenite::Message;

    // Deliberately serialized independently of the producer and Decision type.
    const RAW: &str = r#"{"version":1,"session_id":"teacher","turn_id":"own-turn","status":"skipped","reason":"session_kind_not_user","session_kind":"e2e","producer_version":"0.12.3-rc.1+local"}"#;
    pub(in crate::scenarios::memory) fn raw() -> Value {
        serde_json::from_str(RAW).unwrap()
    }
    pub(in crate::scenarios::memory) fn empty() -> Bank {
        Bank::checked(
            json!({"memories":[],"total":0}),
            json!({"rules":[],"total":0}),
        )
        .unwrap()
    }

    #[test]
    fn raw_schema_is_exact_and_semver_is_not_authority() {
        assert!(Decision::parse(raw()).is_ok());
        let mut count = 0;
        for (key, value) in [
            ("version", json!(2)),
            ("version", Value::Null),
            ("version", json!(1.5)),
            ("status", json!("completed")),
            ("reason", json!("timeout")),
            ("session_kind", json!("user")),
            ("session_kind", json!("subagent")),
            ("session_id", Value::Null),
            ("session_id", json!("")),
            ("session_id", json!(" ")),
            ("session_id", json!("x".repeat(257))),
            ("turn_id", json!("x".repeat(257))),
            ("turn_id", json!("")),
            ("turn_id", Value::Null),
            ("unknown", json!(true)),
            ("producer_version", json!("v1.2.3")),
            ("producer_version", json!("1.2")),
            ("producer_version", json!("01.2.3")),
            ("producer_version", json!("1.2.3-01")),
            ("producer_version", json!("1.2.3+")),
            ("producer_version", Value::Null),
        ] {
            let mut body = raw();
            body[key] = value;
            assert!(Decision::parse(body).is_err(), "{key}");
            count += 1;
        }
        assert_eq!(count, 21);
        // Apply separately so fixture construction cannot accidentally overwrite the body.
        for (key, value) in [
            ("session_id", json!("")),
            ("turn_id", Value::Null),
            ("unknown", json!(true)),
            ("version", json!(2)),
            ("producer_version", json!("1.2.3-01")),
        ] {
            let mut body = raw();
            body[key] = value;
            assert!(Decision::parse(body).is_err(), "{key}");
        }
        for key in [
            "version",
            "session_id",
            "turn_id",
            "status",
            "reason",
            "session_kind",
            "producer_version",
        ] {
            let mut body = raw();
            body.as_object_mut().unwrap().remove(key);
            assert!(Decision::parse(body).is_err());
        }
    }

    #[test]
    fn complete_bank_count_and_both_mutation_channels() {
        let before = empty();
        assert!(before.unchanged(&empty()));
        let facts = Bank::checked(
            json!({"memories":[{"id":"f","text":"new"}],"total":1}),
            json!({"rules":[],"total":0}),
        )
        .unwrap();
        let rules = Bank::checked(
            json!({"memories":[],"total":0}),
            json!({"rules":[{"name":"learned","content":"new"}],"total":1}),
        )
        .unwrap();
        assert!(!before.unchanged(&facts));
        assert!(!before.unchanged(&rules));
        assert!(Bank::checked(json!({"memories":[],"total":1}), json!({"rules":[]})).is_err());
        assert!(Bank::checked(json!({}), json!({"rules":[]})).is_err());
        assert!(Bank::checked(json!({"memories":[]}), json!({"rules":[{}]})).is_err());
    }

    // Engine FunctionDetail schemas and WorkerInfoOutput connection identity;
    // neither contract declares sourceEpoch.
    fn source_info(mode: usize) -> Value {
        let schema = json!({"type":"object", "additionalProperties":false,
            "required":["version","session_id","turn_id","status","reason","session_kind","producer_version"],
            "properties":{"version":{"type":"integer","minimum":1,"maximum":1},
                "session_id":{"type":"string","minLength":1,"maxLength":256},
                "turn_id":{"type":"string","minLength":1,"maxLength":256},
                "status":{"$ref":"#/definitions/ExtractionDecisionStatus"},
                "reason":{"$ref":"#/definitions/ExtractionDecisionReason"},
                "session_kind":{"$ref":"#/definitions/SkippedSessionKind"},
                "producer_version":{"type":"string"}},
            "definitions":{"ExtractionDecisionStatus":{"type":"string","enum":["skipped"]},
                "ExtractionDecisionReason":{"type":"string","enum":["session_kind_not_user"]},
                "SkippedSessionKind":{"type":"string","enum":["automation","e2e"]}}});
        let mut info = json!({"function_id":SOURCE,"namespace":"default","worker_name":"memory",
            "description":"Private operational terminal extraction decision, version 1.","request_schema":schema,"response_schema":schema,
            "registered_triggers":[]});
        match mode {
            5 => info["description"] = json!("incompatible descriptor version 2"),
            12 => {
                info.as_object_mut().unwrap().remove("request_schema");
            }
            13 => info["private"] = json!(false),
            14 => info["namespace"] = json!("foreign"),
            15 => info["response_schema"]["properties"]["version"]["maximum"] = json!(2),
            _ => {}
        }
        info
    }

    fn worker_info(mode: usize) -> Value {
        let mut info = json!({"worker":{"id":if mode == 3 {"rotated"} else {"memory-owner"},
            "name":"memory","namespace":"default","status":"connected",
            "connected_at_ms":if mode == 10 {2} else {1}},
            "functions":[{"function_id":SOURCE,"namespace":"default","worker_name":"memory"}]});
        match mode {
            11 => {
                info["worker"]
                    .as_object_mut()
                    .unwrap()
                    .remove("connected_at_ms");
            }
            16 => info["worker"]["connected_at_ms"] = json!("invalid"),
            17 => info["worker"]["connected_at_ms"] = json!(0),
            18 => {
                info["worker"].as_object_mut().unwrap().remove("id");
            }
            _ => {}
        }
        info
    }

    #[tokio::test]
    async fn bank_read_rotation_invalidates_previously_accepted_skip() {
        let mock = Mock::start().await;
        let capture = Capture::bind(&mock.context, "teacher", "e2e")
            .await
            .unwrap();
        capture.arm();
        capture.turn("own-turn").unwrap();
        assert_eq!(
            mock.send(raw(), Some("memory-owner"), true).await["accepted"],
            true
        );
        mock.mode.store(20, Ordering::SeqCst);
        assert!(capture
            .wait_then_read(&mock.context, "bank", empty())
            .await
            .is_err());
        assert_eq!(mock.reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn actual_metadata_required_connection_descriptor_and_rotation_fail_closed() {
        for mode in [3, 5, 10, 11, 12, 13, 14, 15, 16, 17, 18] {
            let mock = Mock::start().await;
            let capture = Capture::bind(&mock.context, "teacher", "e2e")
                .await
                .unwrap();
            capture.arm();
            capture.turn("own-turn").unwrap();
            mock.mode.store(mode, Ordering::SeqCst);
            assert_eq!(
                mock.send(raw(), Some("memory-owner"), true).await["accepted"],
                false,
                "mode {mode}"
            );
            assert!(capture
                .wait_then_read_bounded(&mock.context, "bank", empty(), Duration::from_millis(10))
                .await
                .is_err());
            assert_eq!(mock.reads.load(Ordering::SeqCst), 0);
            // A new description is prose, not a version: a fresh bind takes it.
            let bound = Capture::bind(&mock.context, "teacher", "e2e").await;
            assert_eq!(bound.is_ok(), matches!(mode, 3 | 5 | 10), "mode {mode}");
        }
    }

    struct Callback {
        body: Value,
        caller: Option<String>,
        activation: bool,
        done: oneshot::Sender<Value>,
    }
    pub(in crate::scenarios::memory) struct Mock {
        pub(in crate::scenarios::memory) context: E2eContext,
        tx: mpsc::Sender<Callback>,
        pub(in crate::scenarios::memory) mode: Arc<AtomicUsize>,
        pub(in crate::scenarios::memory) reads: Arc<AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    impl Mock {
        pub(in crate::scenarios::memory) async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let (tx, mut rx) = mpsc::channel::<Callback>(8);
            let mode = Arc::new(AtomicUsize::new(0));
            let server_mode = mode.clone();
            let reads = Arc::new(AtomicUsize::new(0));
            let server_reads = reads.clone();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.send(Message::Text(json!({"type":"workerregistered","worker_id":"reader","features":["observe.v2"]}).to_string())).await.unwrap();
                let mut observer = String::new();
                let mut target = String::new();
                let mut pending = std::collections::HashMap::new();
                loop {
                    tokio::select! {
                        callback = rx.recv() => {
                            let Some(callback) = callback else { break; };
                            let mode = server_mode.load(Ordering::SeqCst);
                            if mode == 2 { socket.send(Message::Text(json!({"type":"observerstatechanged","id":observer,"state":"suspended","fires":0}).to_string())).await.unwrap(); }
                            let invocation = uuid::Uuid::new_v4().to_string();
                            let mut message = json!({"type":"invokefunction","invocation_id":invocation,"function_id":target,"data":callback.body});
                            if let Some(caller) = callback.caller { message["caller"] = json!({"worker_id":caller,"worker_name":"memory"}); }
                            if callback.activation { message["activation"] = json!({"observer_id":if mode == 4 {"wrong-observer"} else {observer.as_str()}, "activation_id":uuid::Uuid::new_v4().to_string()}); }
                            pending.insert(invocation,callback.done);
                            socket.send(Message::Text(message.to_string())).await.unwrap();
                        }
                        frame = socket.next() => {
                            let text = match frame { Some(Ok(Message::Text(text))) => text,
                                Some(Ok(Message::Ping(_))) => { socket.flush().await.unwrap(); continue; },
                                Some(Ok(Message::Pong(_))) => continue, _ => break };
                            let message: Value = serde_json::from_str(&text).unwrap();
                            let mode = server_mode.load(Ordering::SeqCst);
                            let reply = match message["type"].as_str().unwrap() {
                                "registerworker" => Some(json!({"type":"workerregistered","worker_id":"reader","features":["observe.v2"]})),
                                "registerfunction" => Some(json!({"type":"functionregistered","id":message["id"],"namespace":"default","revision":0,"observers":[]})),
                                "observe" => {
                                    assert_eq!(message["source"]["deliver"],"output");
                                    observer = message["id"].as_str().unwrap().into(); target = message["target"]["function_id"].as_str().unwrap().into();
                                    Some(json!({"type":"observeresult","id":observer,"state":if mode == 1 {"error"} else {"active"},"error":if mode == 1 {json!({"code":"FORBIDDEN","message":"refused"})} else {Value::Null}}))
                                }
                                "invokefunction" => {
                                    let result = match message["function_id"].as_str().unwrap() {
                                        "engine::workers::register" => json!({"id":"reader","registered":true}),
                                        "engine::observers::list" => if mode == 8 { json!({"observers":[]}) } else { json!({"observers":[{"id":observer,"function_id":SOURCE,"namespace":"default","target":{"function_id":if mode == 9 {"wrong-target"} else {target.as_str()},"namespace":"default"}}]}) },
                                        "engine::functions::info" => source_info(mode),
                                        "engine::workers::info" => worker_info(mode),
                                        "memory::list" => { server_reads.fetch_add(1,Ordering::SeqCst); if mode == 20 { server_mode.store(3, Ordering::SeqCst); }
                                            if mode == 6 || mode == 19 { json!({"memories":[{"id":"new","text":"new fact","deleted":mode == 19}],"total":1}) } else { json!({"memories":[],"total":0}) } },
                                        "memory::rule::list" => { server_reads.fetch_add(1,Ordering::SeqCst); if mode == 7 { json!({"rules":[{"name":"learned","content":"new rule"}],"total":1}) } else { json!({"rules":[],"total":0}) } },
                                        other => panic!("unexpected synthetic call {other}"),
                                    };
                                    Some(json!({"type":"invocationresult","invocation_id":message["invocation_id"],"function_id":message["function_id"],"result":result}))
                                }
                                "invocationresult" => { if let Some(done) = pending.remove(message["invocation_id"].as_str().unwrap()) { let _ = done.send(message["result"].clone()); } None },
                                _ => None,
                            };
                            if let Some(reply) = reply { socket.send(Message::Text(reply.to_string())).await.unwrap(); }
                        }
                    }
                }
            });
            let client = iii_sdk::register_worker(
                &url,
                iii_sdk::InitOptions {
                    identity: iii_sdk::WorkerIdentityMode::Explicit,
                    namespace: Some("default".into()),
                    ..Default::default()
                },
            );
            while client.get_connection_state() != iii_sdk::runtime::IIIConnectionState::Connected {
                tokio::task::yield_now().await;
            }
            Self {
                context: E2eContext::from_client(client),
                tx,
                mode,
                reads,
                server,
            }
        }
        pub(in crate::scenarios::memory) async fn send(
            &self,
            body: Value,
            caller: Option<&str>,
            activation: bool,
        ) -> Value {
            let (done, result) = oneshot::channel();
            self.tx
                .send(Callback {
                    body,
                    caller: caller.map(str::to_owned),
                    activation,
                    done,
                })
                .await
                .unwrap();
            result.await.unwrap()
        }
    }

    #[tokio::test]
    async fn sdk_loopback_runtime_wait_then_actual_bank_read_and_negative_controls() {
        tokio::time::timeout(Duration::from_secs(20), async {
            // Real SDK messages feed the production collector and production bank RPCs.
            for mutation in [0, 6, 7, 19] {
                let mock = Mock::start().await;
                let before = read_bank(&mock.context, "bank").await.unwrap();
                assert_eq!(mock.reads.load(Ordering::SeqCst), 2);
                let capture = Capture::bind(&mock.context, "teacher", "e2e")
                    .await
                    .unwrap();
                capture.arm();
                assert_eq!(
                    mock.send(raw(), Some("memory-owner"), true).await["accepted"],
                    true
                );
                capture.turn("own-turn").unwrap();
                // Duplicate source events do not expand or replace the pair.
                assert_eq!(
                    mock.send(raw(), Some("memory-owner"), true).await["accepted"],
                    true
                );
                mock.mode.store(mutation, Ordering::SeqCst);
                let evidence = capture
                    .wait_then_read(&mock.context, "bank", before)
                    .await
                    .unwrap();
                assert_eq!(mock.reads.load(Ordering::SeqCst), 4);
                assert_eq!(
                    evidence.before_teacher.unchanged(&evidence.after_skip),
                    mutation == 0
                );
            }
            for control in 0..16 {
                let mock = Mock::start().await;
                let capture = Capture::bind(&mock.context, "teacher", "e2e")
                    .await
                    .unwrap();
                // Old events before this attempt was armed never qualify.
                if control == 8 {
                    assert_eq!(
                        mock.send(raw(), Some("memory-owner"), true).await["accepted"],
                        false
                    );
                }
                capture.arm();
                capture.turn("own-turn").unwrap();
                let mut body = raw();
                match control {
                    3 => mock.mode.store(3, Ordering::SeqCst),
                    4 => mock.mode.store(4, Ordering::SeqCst),
                    5 => mock.mode.store(5, Ordering::SeqCst),
                    6 => body["turn_id"] = json!("other-turn"),
                    7 => body["session_kind"] = json!("automation"),
                    9 => mock.mode.store(2, Ordering::SeqCst),
                    10 => body["unknown"] = json!(true),
                    11 => body["session_id"] = json!("other-session"),
                    12 => body["producer_version"] = json!("invalid"),
                    14 => mock.mode.store(8, Ordering::SeqCst),
                    15 => mock.mode.store(9, Ordering::SeqCst),
                    _ => {}
                }
                if control != 0 && control != 8 {
                    let caller = if control == 13 {
                        None
                    } else if control == 1 {
                        Some("counterfeit")
                    } else {
                        Some("memory-owner")
                    };
                    assert_eq!(
                        mock.send(body, caller, control != 2).await["accepted"],
                        false,
                        "control {control}"
                    );
                }
                assert!(
                    capture
                        .wait_then_read_bounded(
                            &mock.context,
                            "bank",
                            empty(),
                            Duration::from_millis(10)
                        )
                        .await
                        .is_err(),
                    "control {control}"
                );
                assert_eq!(
                    mock.reads.load(Ordering::SeqCst),
                    0,
                    "no read before skip: {control}"
                );
            }
            // The waiter starts with no decision; a real callback, not a delay,
            // unlocks the read barrier (Notify retains a racing notification).
            let mock = Mock::start().await;
            let capture = Capture::bind(&mock.context, "teacher", "e2e")
                .await
                .unwrap();
            capture.arm();
            capture.turn("own-turn").unwrap();
            let waiter = capture.wait_then_read(&mock.context, "bank", empty());
            let delivery = async {
                tokio::task::yield_now().await;
                assert_eq!(mock.reads.load(Ordering::SeqCst), 0);
                assert_eq!(
                    mock.send(raw(), Some("memory-owner"), true).await["accepted"],
                    true
                );
            };
            let (result, ()) = tokio::join!(waiter, delivery);
            assert!(result.unwrap().before_teacher.unchanged(&empty()));
            assert_eq!(mock.reads.load(Ordering::SeqCst), 2);
            drop(capture);
            drop(mock);
            let mock = Mock::start().await;
            mock.mode.store(1, Ordering::SeqCst);
            assert!(Capture::bind(&mock.context, "teacher", "e2e")
                .await
                .is_err());
        })
        .await
        .unwrap();
    }
}
