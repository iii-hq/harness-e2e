//! Shared wire types for runner-hosted post-turn validation functions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lenient view of the post-turn hook envelope sent by Harness.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct HookEnvelope {
    #[serde(default)]
    pub point: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub result: Value,
}

/// Response returned by a post-turn validation function.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct HookVerdict {
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
