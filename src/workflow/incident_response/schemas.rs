use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixturePreflightRequest {
    pub workspace_root: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixturePreflightResponse {
    pub repository: String,
    pub workspace_root: String,
    pub known_good_sha: String,
    pub incident_sha: String,
    pub fixture_contract_sha256: String,
    pub hidden_probe_manifest_sha256: String,
    pub clean: bool,
    pub capability_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BaselineRequest {
    pub attempt_id: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BaselineResponse {
    pub deployed_revision: String,
    pub data_sha256: String,
    pub telemetry_sha256: String,
    pub ledger_sha256: String,
    pub audit_sha256: String,
    pub incident_status: String,
    pub active_operations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlertRequest {
    pub event_id: String,
    pub idempotency_key: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlertResponse {
    pub incident_id: String,
    pub alert_fingerprint: String,
    pub request_count: u32,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReproduceRequest {
    pub event_id: String,
    pub reproduction_key: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReproduceResponse {
    pub event_id: String,
    pub attempts: u32,
    pub timeout_point: String,
    pub expected_settlement_count: u32,
    pub observed_settlement_count: u32,
    pub ledger_delta: i64,
    pub audit_entries: u32,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TelemetryRequest {
    pub kind: String,
    pub event_id: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TelemetryResponse {
    pub kind: String,
    pub evidence_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateRequest {
    pub mode: String,
    pub attempt_id: String,
    pub workspace_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_sha: Option<String>,
    #[serde(default)]
    pub probe_ids: Vec<String>,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeResult {
    pub passed: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_sha: Option<String>,
    #[serde(default)]
    pub changed_paths: Vec<String>,
    pub protected_paths_unchanged: bool,
    pub tests_unchanged: bool,
    pub fixture_contract_unchanged: bool,
    pub working_tree_candidate_only: bool,
    pub repair_rounds: u8,
    #[serde(default)]
    pub probes: BTreeMap<String, ProbeResult>,
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub before_after_hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployRequest {
    pub action: String,
    pub revision: String,
    pub attempt_id: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployResponse {
    pub action: String,
    pub deployed_revision: String,
    pub active_operations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileRequest {
    pub attempt_id: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileResponse {
    pub deployed_revision: String,
    pub event_id: String,
    pub settlement_count: u32,
    pub distinct_events_preserved: bool,
    pub audit_history_preserved: bool,
    pub incident_status: String,
    pub active_operations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetRequest {
    pub attempt_id: String,
    pub initial_revision: String,
    #[serde(rename = "_caller_worker_id", default, skip_serializing)]
    #[schemars(skip)]
    pub(crate) _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetResponse {
    pub restored_revision: String,
    pub clean: bool,
    pub active_operations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    pub id: String,
    pub claim: String,
    pub evidence_ids: Vec<String>,
    pub observations: Vec<String>,
    pub falsification_probe: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalysisResult {
    pub analysis_kind: String,
    pub hypotheses: Vec<Hypothesis>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosisResult {
    pub ranked_hypothesis_ids: Vec<String>,
    pub selected_hypothesis_id: String,
    pub supporting_evidence_ids: Vec<String>,
    #[serde(default)]
    pub contradicting_evidence_ids: Vec<String>,
    pub causal_chain: Vec<String>,
    pub falsification_probe_ids: Vec<String>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemediationResult {
    pub summary: String,
    pub focused_test_ids: Vec<String>,
    pub repair_rounds: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IncidentReportResult {
    pub markdown: String,
}
