use std::fmt;
use std::ops::Deref;

use anyhow::{bail, Context, Result};
use schemars::gen::SchemaGenerator;
use schemars::schema::Schema;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::artifact::sha256_value;

pub const CONTROL_PLANE_CONTRACT_NAME: &str = "harness-control-plane";
pub const CONTROL_PLANE_CONTRACT_VERSION: u64 = 1;

/// A normalized view of a wire response together with the exact payload that
/// produced it. Consumers evaluate the typed view, while reports serialize the
/// original payload so additive fields are not discarded.
#[derive(Clone)]
pub struct Observed<T> {
    normalized: T,
    raw: Value,
}

impl<T> Observed<T> {
    pub fn into_normalized(self) -> T {
        self.normalized
    }
}

#[cfg(test)]
impl<T> Observed<T>
where
    T: Serialize,
{
    pub fn from_normalized(normalized: T) -> Self {
        let raw = serde_json::to_value(&normalized).expect("normalized wire value serializes");
        Self { normalized, raw }
    }
}

impl<T> Deref for Observed<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.normalized
    }
}

impl<T> fmt::Debug for Observed<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Observed")
            .field("normalized", &self.normalized)
            .field("raw", &self.raw)
            .finish()
    }
}

impl<T> Serialize for Observed<T> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.raw.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Observed<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Value::deserialize(deserializer)?;
        let normalized = serde_json::from_value(raw.clone()).map_err(serde::de::Error::custom)?;
        Ok(Self { normalized, raw })
    }
}

impl<T> JsonSchema for Observed<T>
where
    T: JsonSchema,
{
    fn schema_name() -> String {
        T::schema_name()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExposeMode {
    #[default]
    AgentTrigger,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct FunctionPolicy {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub expose: ExposeMode,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum MessageInput {
    Text(String),
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct SendOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub functions: Option<FunctionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct SessionInit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SendRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub message: MessageInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<SendOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SendResponseV1 {
    pub session_id: String,
    pub turn_id: String,
    pub accepted: bool,
    #[serde(default)]
    pub merged: Option<bool>,
    #[serde(default)]
    pub queued: Option<bool>,
    #[serde(default)]
    pub deduplicated: Option<bool>,
}

pub type SendResponse = Observed<SendResponseV1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    AwaitingFunctions,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StatusReportV1 {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub status: TurnStatus,
    pub step: u64,
    pub turn_count: u32,
    pub max_turns: u32,
    #[serde(default)]
    pub pending_function_calls: Vec<String>,
    #[serde(default)]
    pub children: Vec<Value>,
    #[serde(default)]
    pub queued: Vec<Value>,
    #[serde(default)]
    pub expects_wake: bool,
    #[serde(default)]
    pub result_error: Option<String>,
    #[serde(default)]
    pub validation_retries: u32,
    #[serde(default)]
    pub transient_resumes: u32,
}

pub type StatusReport = Observed<StatusReportV1>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TurnCompletedEventV1 {
    pub session_id: String,
    pub turn_id: String,
    pub status: TurnStatus,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub result_error: Option<String>,
}

pub type TurnCompletedEvent = Observed<TurnCompletedEventV1>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionTreeNodeV1 {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionTreeResponseBodyV1 {
    pub root_session_id: String,
    pub sessions: Vec<SessionTreeNodeV1>,
    pub complete: bool,
}

pub type SessionTreeResponseV1 = Observed<SessionTreeResponseBodyV1>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StopResponseV1 {
    pub stopping: bool,
}

pub type StopResponse = Observed<StopResponseV1>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeardownResponseBodyV1 {
    pub removed: u64,
}

pub type TeardownResponseV1 = Observed<TeardownResponseBodyV1>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SessionUsageTotalsV1 {
    pub sessions: u64,
    pub turns: u64,
    pub function_calls: u64,
    pub function_call_errors: u64,
    #[serde(default)]
    pub validation_retries: Option<u64>,
    #[serde(default)]
    pub transient_resumes: Option<u64>,
    #[serde(default)]
    pub wake_resumes: Option<u64>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionUsageV1 {
    pub session_id: String,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    pub depth: u32,
    pub turns: u64,
    pub function_calls: u64,
    pub function_call_errors: u64,
    #[serde(default)]
    pub validation_retries: Option<u64>,
    #[serde(default)]
    pub transient_resumes: Option<u64>,
    #[serde(default)]
    pub wake_resumes: Option<u64>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub context: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionMetricsResponseBodyV1 {
    pub root_session_id: String,
    pub complete: bool,
    pub totals: SessionUsageTotalsV1,
    pub by_session: Vec<SessionUsageV1>,
    #[serde(default)]
    pub traces: Option<Value>,
}

pub type SessionMetricsResponseV1 = Observed<SessionMetricsResponseBodyV1>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CatalogModelV1 {
    pub id: String,
    pub provider: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub supports_vision: Option<bool>,
}

pub type Model = Observed<CatalogModelV1>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FunctionContractEvidence {
    pub function_id: String,
    pub contract: Value,
    pub request_schema: Value,
    pub response_schema: Value,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ControlPlaneEvidence {
    pub name: String,
    pub version: u64,
    pub functions: Vec<FunctionContractEvidence>,
}

#[derive(Clone, Copy)]
enum JsonType {
    Any,
    Array,
    Boolean,
    Integer,
    Object,
    String,
}

impl JsonType {
    fn name(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Array => "array",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Object => "object",
            Self::String => "string",
        }
    }
}

#[derive(Clone, Copy)]
struct SchemaField {
    path: &'static str,
    kind: JsonType,
    required: bool,
}

#[derive(Clone, Copy)]
struct FunctionRequirement {
    function_id: &'static str,
    capability: &'static str,
    request: &'static [SchemaField],
    response: &'static [SchemaField],
}

const SEND_REQUEST: &[SchemaField] = &[
    SchemaField {
        path: "message",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "session_id",
        kind: JsonType::String,
        required: false,
    },
    SchemaField {
        path: "model",
        kind: JsonType::String,
        required: false,
    },
    SchemaField {
        path: "provider",
        kind: JsonType::String,
        required: false,
    },
    SchemaField {
        path: "idempotency_key",
        kind: JsonType::String,
        required: false,
    },
    SchemaField {
        path: "session",
        kind: JsonType::Object,
        required: false,
    },
    SchemaField {
        path: "options",
        kind: JsonType::Object,
        required: false,
    },
    SchemaField {
        path: "options.max_turns",
        kind: JsonType::Integer,
        required: false,
    },
    SchemaField {
        path: "options.max_output_tokens",
        kind: JsonType::Integer,
        required: false,
    },
    SchemaField {
        path: "options.max_total_tokens",
        kind: JsonType::Integer,
        required: false,
    },
    SchemaField {
        path: "options.functions",
        kind: JsonType::Object,
        required: false,
    },
    SchemaField {
        path: "options.functions.allow",
        kind: JsonType::Array,
        required: false,
    },
    SchemaField {
        path: "options.functions.deny",
        kind: JsonType::Array,
        required: false,
    },
    SchemaField {
        path: "options.metadata",
        kind: JsonType::Any,
        required: false,
    },
];

const SEND_RESPONSE: &[SchemaField] = &[
    SchemaField {
        path: "session_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "turn_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "accepted",
        kind: JsonType::Boolean,
        required: true,
    },
];

const STATUS_REQUEST: &[SchemaField] = &[SchemaField {
    path: "session_id",
    kind: JsonType::String,
    required: true,
}];

const STATUS_RESPONSE: &[SchemaField] = &[
    SchemaField {
        path: "session_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "turn_id",
        kind: JsonType::String,
        required: false,
    },
    SchemaField {
        path: "status",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "step",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "turn_count",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "max_turns",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "pending_function_calls",
        kind: JsonType::Array,
        required: true,
    },
    SchemaField {
        path: "children",
        kind: JsonType::Array,
        required: true,
    },
    SchemaField {
        path: "queued",
        kind: JsonType::Array,
        required: false,
    },
    SchemaField {
        path: "expects_wake",
        kind: JsonType::Boolean,
        required: false,
    },
];

const ROOT_SESSION_REQUEST: &[SchemaField] = &[SchemaField {
    path: "root_session_id",
    kind: JsonType::String,
    required: true,
}];

const TREE_RESPONSE: &[SchemaField] = &[
    SchemaField {
        path: "root_session_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "sessions",
        kind: JsonType::Array,
        required: true,
    },
    SchemaField {
        path: "sessions[].session_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "complete",
        kind: JsonType::Boolean,
        required: true,
    },
];

const METRICS_RESPONSE: &[SchemaField] = &[
    SchemaField {
        path: "root_session_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "complete",
        kind: JsonType::Boolean,
        required: true,
    },
    SchemaField {
        path: "totals.sessions",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "totals.turns",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "totals.function_calls",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "totals.function_call_errors",
        kind: JsonType::Integer,
        required: true,
    },
    SchemaField {
        path: "by_session",
        kind: JsonType::Array,
        required: true,
    },
    SchemaField {
        path: "by_session[].session_id",
        kind: JsonType::String,
        required: true,
    },
    SchemaField {
        path: "by_session[].depth",
        kind: JsonType::Integer,
        required: true,
    },
];

const STOP_RESPONSE: &[SchemaField] = &[SchemaField {
    path: "stopping",
    kind: JsonType::Boolean,
    required: true,
}];

const TEARDOWN_RESPONSE: &[SchemaField] = &[SchemaField {
    path: "removed",
    kind: JsonType::Integer,
    required: true,
}];

const CONTROL_PLANE: &[FunctionRequirement] = &[
    FunctionRequirement {
        function_id: "harness::send",
        capability: "send",
        request: SEND_REQUEST,
        response: SEND_RESPONSE,
    },
    FunctionRequirement {
        function_id: "harness::status",
        capability: "status",
        request: STATUS_REQUEST,
        response: STATUS_RESPONSE,
    },
    FunctionRequirement {
        function_id: "harness::session-tree",
        capability: "session-tree",
        request: ROOT_SESSION_REQUEST,
        response: TREE_RESPONSE,
    },
    FunctionRequirement {
        function_id: "harness::metrics",
        capability: "metrics",
        request: ROOT_SESSION_REQUEST,
        response: METRICS_RESPONSE,
    },
    FunctionRequirement {
        function_id: "harness::stop",
        capability: "stop",
        request: STATUS_REQUEST,
        response: STOP_RESPONSE,
    },
    FunctionRequirement {
        function_id: "harness::teardown",
        capability: "teardown",
        request: ROOT_SESSION_REQUEST,
        response: TEARDOWN_RESPONSE,
    },
];

pub fn control_plane_function_ids() -> impl Iterator<Item = &'static str> {
    CONTROL_PLANE
        .iter()
        .map(|requirement| requirement.function_id)
}

pub fn validate_control_plane(
    raw: &Value,
    allow_legacy_metadata: bool,
) -> Result<ControlPlaneEvidence> {
    let functions = raw
        .get("functions")
        .and_then(Value::as_array)
        .context("engine::functions::info response is missing functions[]")?;

    let mut observed = Vec::with_capacity(CONTROL_PLANE.len());
    for requirement in CONTROL_PLANE {
        let detail = functions
            .iter()
            .find(|entry| {
                entry.get("function_id").and_then(Value::as_str) == Some(requirement.function_id)
            })
            .with_context(|| {
                format!(
                    "engine::functions::info omitted required function {}",
                    requirement.function_id
                )
            })?;
        if let Some(error) = detail.get("error").and_then(Value::as_str) {
            bail!(
                "required function {} is unavailable: {error}",
                requirement.function_id
            );
        }
        let contract = if detail.pointer("/metadata/contract").is_some() || !allow_legacy_metadata {
            validate_metadata(detail, requirement)?;
            detail
                .pointer("/metadata/contract")
                .cloned()
                .context("validated contract metadata disappeared")?
        } else {
            serde_json::json!({
                "name": CONTROL_PLANE_CONTRACT_NAME,
                "version": CONTROL_PLANE_CONTRACT_VERSION,
                "capabilities": [requirement.capability],
                "legacy_metadata": true,
            })
        };
        validate_schema(detail, "request_schema", requirement.request)
            .with_context(|| format!("{} request contract", requirement.function_id))?;
        validate_schema(detail, "response_schema", requirement.response)
            .with_context(|| format!("{} response contract", requirement.function_id))?;
        let request_schema = detail["request_schema"].clone();
        let response_schema = detail["response_schema"].clone();
        let sha256 = sha256_value(&serde_json::json!({
            "contract": contract,
            "request_schema": request_schema,
            "response_schema": response_schema,
        }))?;
        observed.push(FunctionContractEvidence {
            function_id: requirement.function_id.to_string(),
            contract,
            request_schema,
            response_schema,
            sha256,
        });
    }
    Ok(ControlPlaneEvidence {
        name: CONTROL_PLANE_CONTRACT_NAME.to_string(),
        version: CONTROL_PLANE_CONTRACT_VERSION,
        functions: observed,
    })
}

fn validate_metadata(detail: &Value, requirement: &FunctionRequirement) -> Result<()> {
    let contract = detail
        .pointer("/metadata/contract")
        .with_context(|| format!("{} has no metadata.contract", requirement.function_id))?;
    let name = contract.get("name").and_then(Value::as_str);
    if name != Some(CONTROL_PLANE_CONTRACT_NAME) {
        bail!(
            "{} advertises contract name {:?}; expected {CONTROL_PLANE_CONTRACT_NAME}",
            requirement.function_id,
            name
        );
    }
    let version = contract.get("version").and_then(Value::as_u64);
    if version != Some(CONTROL_PLANE_CONTRACT_VERSION) {
        bail!(
            "{} advertises contract version {:?}; expected {}",
            requirement.function_id,
            version,
            CONTROL_PLANE_CONTRACT_VERSION
        );
    }
    let capabilities = contract
        .get("capabilities")
        .and_then(Value::as_array)
        .with_context(|| {
            format!(
                "{} metadata.contract is missing capabilities[]",
                requirement.function_id
            )
        })?;
    if !capabilities
        .iter()
        .any(|capability| capability.as_str() == Some(requirement.capability))
    {
        bail!(
            "{} does not advertise required capability {}",
            requirement.function_id,
            requirement.capability
        );
    }
    Ok(())
}

fn validate_schema(detail: &Value, key: &str, fields: &[SchemaField]) -> Result<()> {
    let root = detail.get(key).with_context(|| format!("missing {key}"))?;
    if !root.is_object() {
        bail!("{key} is not a JSON Schema object");
    }
    for field in fields {
        let schema = schema_at_path(root, field.path)
            .with_context(|| format!("schema is missing field {}", field.path))?;
        if !schema_accepts_type(root, schema, field.kind) {
            bail!(
                "field {} does not accept expected JSON type {}",
                field.path,
                field.kind.name()
            );
        }
        if field.required && !schema_marks_required(root, field.path) {
            bail!("field {} is no longer required", field.path);
        }
    }
    Ok(())
}

fn schema_at_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = schema_variant(root, root)?;
    for segment in path.split('.') {
        let (name, descend_array) = segment
            .strip_suffix("[]")
            .map_or((segment, false), |name| (name, true));
        current = schema_variant(root, current)?
            .get("properties")?
            .get(name)?;
        if descend_array {
            current = schema_variant(root, current)?.get("items")?;
        }
    }
    schema_variant(root, current)
}

fn schema_marks_required(root: &Value, path: &str) -> bool {
    let mut current = match schema_variant(root, root) {
        Some(schema) => schema,
        None => return false,
    };
    let mut segments = path.split('.').peekable();
    while let Some(segment) = segments.next() {
        let (name, descend_array) = segment
            .strip_suffix("[]")
            .map_or((segment, false), |name| (name, true));
        let Some(container) = schema_variant(root, current) else {
            return false;
        };
        if segments.peek().is_none() {
            return container
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.iter().any(|field| field.as_str() == Some(name)));
        }
        let Some(mut property) = container
            .get("properties")
            .and_then(|value| value.get(name))
        else {
            return false;
        };
        if descend_array {
            let Some(items) = schema_variant(root, property).and_then(|value| value.get("items"))
            else {
                return false;
            };
            property = items;
        }
        current = property;
    }
    false
}

fn schema_variant<'a>(root: &'a Value, schema: &'a Value) -> Option<&'a Value> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return root
            .pointer(reference.strip_prefix('#')?)
            .and_then(|resolved| {
                if std::ptr::eq(resolved, schema) {
                    Some(resolved)
                } else {
                    schema_variant(root, resolved)
                }
            });
    }
    for combinator in ["allOf", "anyOf", "oneOf"] {
        if let Some(options) = schema.get(combinator).and_then(Value::as_array) {
            if let Some(candidate) = options.iter().find(|candidate| {
                !matches!(candidate.get("type").and_then(Value::as_str), Some("null"))
            }) {
                return schema_variant(root, candidate);
            }
        }
    }
    Some(schema)
}

fn schema_accepts_type(root: &Value, schema: &Value, expected: JsonType) -> bool {
    if matches!(expected, JsonType::Any) || schema == &Value::Bool(true) {
        return true;
    }
    let expected = expected.name();
    let Some(schema) = schema_variant(root, schema) else {
        return false;
    };
    match schema.get("type") {
        Some(Value::String(kind)) => kind == expected,
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind.as_str() == Some(expected)),
        _ if expected == "object" => schema.get("properties").is_some(),
        _ if expected == "array" => schema.get("items").is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_harness_catalog_satisfies_the_runner_contract() {
        let raw = serde_json::json!({
            "functions": [
                schema_fixture(include_str!("../golden/schemas/harness.send.json")),
                schema_fixture(include_str!("../golden/schemas/harness.status.json")),
                schema_fixture(include_str!("../golden/schemas/harness.session-tree.json")),
                schema_fixture(include_str!("../golden/schemas/harness.metrics.json")),
                schema_fixture(include_str!("../golden/schemas/harness.stop.json")),
                schema_fixture(include_str!("../golden/schemas/harness.teardown.json")),
            ]
        });
        validate_control_plane(&raw, false).unwrap();
    }

    #[test]
    fn legacy_mode_still_validates_schemas_when_contract_metadata_is_absent() {
        let raw = serde_json::json!({
            "functions": [
                legacy_schema_fixture(include_str!("../golden/schemas/harness.send.json")),
                legacy_schema_fixture(include_str!("../golden/schemas/harness.status.json")),
                legacy_schema_fixture(include_str!("../golden/schemas/harness.session-tree.json")),
                legacy_schema_fixture(include_str!("../golden/schemas/harness.metrics.json")),
                legacy_schema_fixture(include_str!("../golden/schemas/harness.stop.json")),
                legacy_schema_fixture(include_str!("../golden/schemas/harness.teardown.json")),
            ]
        });
        validate_control_plane(&raw, true).unwrap();
        let strict_error = format!("{:#}", validate_control_plane(&raw, false).unwrap_err());
        assert!(strict_error.contains("metadata.contract"));
    }

    #[test]
    fn response_types_decode_the_valid_contract_fixture() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../fixtures/contracts/valid-responses.json"))
                .unwrap();
        let send: SendResponse = serde_json::from_value(fixtures["send"].clone()).unwrap();
        let status: StatusReport = serde_json::from_value(fixtures["status"].clone()).unwrap();
        let tree: SessionTreeResponseV1 =
            serde_json::from_value(fixtures["session_tree"].clone()).unwrap();
        let metrics: SessionMetricsResponseV1 =
            serde_json::from_value(fixtures["metrics"].clone()).unwrap();
        let stop: StopResponse = serde_json::from_value(fixtures["stop"].clone()).unwrap();
        let teardown: TeardownResponseV1 =
            serde_json::from_value(fixtures["teardown"].clone()).unwrap();

        assert!(send.accepted);
        assert_eq!(status.status, TurnStatus::Completed);
        assert!(tree.complete);
        assert_eq!(metrics.totals.sessions, 1);
        assert!(stop.stopping);
        assert_eq!(teardown.removed, 0);
    }

    #[test]
    fn response_types_accept_additive_fields_and_preserve_the_raw_payload() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../fixtures/contracts/additive-responses.json"
        ))
        .unwrap();
        let response: SendResponse = serde_json::from_value(fixtures["send"].clone()).unwrap();
        assert!(response.accepted);
        assert_eq!(serde_json::to_value(&response).unwrap(), fixtures["send"]);

        let metrics: SessionMetricsResponseV1 =
            serde_json::from_value(fixtures["metrics"].clone()).unwrap();
        assert_eq!(metrics.totals.turns, 1);
        assert_eq!(serde_json::to_value(&metrics).unwrap(), fixtures["metrics"]);
    }

    #[test]
    fn turn_completed_event_ignores_unknown_fields_and_keeps_raw() {
        let raw = serde_json::json!({
            "session_id": "e2e_root",
            "turn_id": "turn-1",
            "status": "completed",
            "terminal": true,
            "timestamp": 1_700_000_000_000i64,
            "parent_session_id": "parent",
            "result": { "text": "ok" },
            "extra_consumer_field": { "nested": true }
        });
        let event: TurnCompletedEvent = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(event.session_id, "e2e_root");
        assert_eq!(event.turn_id, "turn-1");
        assert_eq!(event.status, TurnStatus::Completed);
        assert!(event.terminal);
        assert_eq!(event.result_error, None);
        assert_eq!(serde_json::to_value(&event).unwrap(), raw);
    }

    #[test]
    fn response_types_reject_missing_or_invalid_required_fields() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../fixtures/contracts/incompatible-responses.json"
        ))
        .unwrap();
        let missing = serde_json::from_value::<SendResponse>(fixtures["send"].clone())
            .unwrap_err()
            .to_string();
        assert!(missing.contains("accepted"), "unexpected error: {missing}");

        let invalid =
            serde_json::from_value::<SessionMetricsResponseV1>(fixtures["metrics"].clone())
                .unwrap_err()
                .to_string();
        assert!(
            invalid.contains("expected u64"),
            "unexpected error: {invalid}"
        );
    }

    #[test]
    fn preflight_reports_an_incompatible_schema_field() {
        let mut send = schema_fixture(include_str!("../golden/schemas/harness.send.json"));
        send["response_schema"]["properties"]["accepted"]["type"] =
            Value::String("string".to_string());
        let raw = serde_json::json!({
            "functions": [
                send,
                schema_fixture(include_str!("../golden/schemas/harness.status.json")),
                schema_fixture(include_str!("../golden/schemas/harness.session-tree.json")),
                schema_fixture(include_str!("../golden/schemas/harness.metrics.json")),
                schema_fixture(include_str!("../golden/schemas/harness.stop.json")),
                schema_fixture(include_str!("../golden/schemas/harness.teardown.json")),
            ]
        });
        let error = format!("{:#}", validate_control_plane(&raw, false).unwrap_err());
        assert!(error.contains("accepted"), "unexpected error: {error}");
        assert!(error.contains("boolean"), "unexpected error: {error}");
    }

    fn schema_fixture(source: &str) -> Value {
        let mut fixture: Value = serde_json::from_str(source).unwrap();
        fixture["worker_name"] = Value::String("harness".to_string());
        fixture["registered_triggers"] = Value::Array(Vec::new());
        fixture
    }

    fn legacy_schema_fixture(source: &str) -> Value {
        let mut fixture = schema_fixture(source);
        fixture["metadata"] = serde_json::json!({ "internal": true });
        fixture
    }
}
