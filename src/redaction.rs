use std::collections::BTreeSet;

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const REDACTED: &str = "[REDACTED]";
const DEFAULT_SECRET_ENV_NAMES: &[&str] = &[
    "OPENAI_API_KEY",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "AWS_SECRET_ACCESS_KEY",
    "CLOUDFLARE_API_TOKEN",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RedactionReport {
    pub redacted_values: u32,
    pub redacted_fields: u32,
    pub rules: BTreeSet<String>,
}

impl RedactionReport {
    pub fn changed(&self) -> bool {
        self.redacted_values > 0 || self.redacted_fields > 0
    }

    pub fn merge(&mut self, other: Self) {
        self.redacted_values = self.redacted_values.saturating_add(other.redacted_values);
        self.redacted_fields = self.redacted_fields.saturating_add(other.redacted_fields);
        self.rules.extend(other.rules);
    }
}

#[derive(Debug, Clone, Default)]
pub struct RedactionPolicy {
    known_values: Vec<String>,
}

impl RedactionPolicy {
    pub fn from_environment() -> Self {
        let mut names = DEFAULT_SECRET_ENV_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<_>>();
        if let Ok(extra) = std::env::var("HARNESS_E2E_SECRET_ENV_NAMES") {
            names.extend(
                extra
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
        let known_values = names
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| value.len() >= 8)
            .collect();
        Self { known_values }
    }

    #[cfg(test)]
    pub fn with_known_values(values: impl IntoIterator<Item = String>) -> Self {
        Self {
            known_values: values
                .into_iter()
                .filter(|value| value.len() >= 8)
                .collect(),
        }
    }

    pub fn redact_value(&self, value: &mut Value) -> RedactionReport {
        let mut report = RedactionReport {
            ..RedactionReport::default()
        };
        redact_value(self, value, &mut report);
        report
    }

    pub fn sanitize_bytes(
        &self,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<(Vec<u8>, RedactionReport)> {
        if media_type == "application/json" || media_type.ends_with("+json") {
            let mut value: Value =
                serde_json::from_slice(bytes).context("decode JSON before redaction")?;
            let report = self.redact_value(&mut value);
            if !report.changed() {
                self.assert_clean(bytes)?;
                return Ok((bytes.to_vec(), report));
            }
            let mut sanitized =
                serde_json::to_vec_pretty(&value).context("encode redacted JSON")?;
            sanitized.push(b'\n');
            self.assert_clean(&sanitized)?;
            return Ok((sanitized, report));
        }
        if media_type.starts_with("text/") {
            let text =
                std::str::from_utf8(bytes).context("decode text artifact before redaction")?;
            let (sanitized, report) = self.redact_text(text);
            self.assert_clean(sanitized.as_bytes())?;
            return Ok((sanitized.into_bytes(), report));
        }
        self.assert_clean(bytes)?;
        Ok((
            bytes.to_vec(),
            RedactionReport {
                ..Default::default()
            },
        ))
    }

    pub fn assert_clean(&self, bytes: &[u8]) -> Result<()> {
        let text = String::from_utf8_lossy(bytes);
        let findings = self.findings(&text);
        if !findings.is_empty() {
            bail!(
                "secret scanner rejected artifact; matched rules: {}",
                findings.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        Ok(())
    }

    pub fn redact_text(&self, text: &str) -> (String, RedactionReport) {
        let mut sanitized = text.to_string();
        let mut report = RedactionReport {
            ..Default::default()
        };
        for secret in &self.known_values {
            let count = sanitized.matches(secret).count();
            if count > 0 {
                sanitized = sanitized.replace(secret, REDACTED);
                report.redacted_values = report
                    .redacted_values
                    .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
                report.rules.insert("known_secret".into());
            }
        }
        while let Some((start, end, rule)) = first_shape_finding(&sanitized) {
            sanitized.replace_range(start..end, REDACTED);
            report.redacted_values = report.redacted_values.saturating_add(1);
            report.rules.insert(rule.into());
        }
        (sanitized, report)
    }

    fn findings(&self, text: &str) -> BTreeSet<String> {
        let mut findings = BTreeSet::new();
        if self
            .known_values
            .iter()
            .any(|secret| text.contains(secret.as_str()))
        {
            findings.insert("known_secret".into());
        }
        let mut remaining = text;
        while let Some((_, end, rule)) = first_shape_finding(remaining) {
            findings.insert(rule.into());
            remaining = &remaining[end..];
        }
        findings
    }
}

fn redact_value(policy: &RedactionPolicy, value: &mut Value, report: &mut RedactionReport) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if sensitive_key(key) && !value.is_null() {
                    *value = Value::String(REDACTED.into());
                    report.redacted_fields = report.redacted_fields.saturating_add(1);
                    report.rules.insert("sensitive_field".into());
                } else {
                    redact_value(policy, value, report);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_value(policy, value, report);
            }
        }
        Value::String(text) => {
            let (sanitized, nested) = policy.redact_text(text);
            *text = sanitized;
            report.merge(nested);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn sensitive_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "authorization"
            | "cookie"
            | "password"
            | "passwd"
            | "secret"
            | "client_secret"
            | "api_key"
            | "apikey"
            | "access_token"
            | "refresh_token"
            | "private_key"
    )
}

fn first_shape_finding(text: &str) -> Option<(usize, usize, &'static str)> {
    if let Some(start) = text.find("-----BEGIN PRIVATE KEY-----") {
        let end = text[start..]
            .find("-----END PRIVATE KEY-----")
            .map(|offset| start + offset + "-----END PRIVATE KEY-----".len())
            .unwrap_or_else(|| text.len());
        return Some((start, end, "private_key"));
    }
    if let Some(start) = find_ascii_case_insensitive(text, "bearer ") {
        let token_start = start + "bearer ".len();
        let token_end = token_end(text, token_start);
        if token_end.saturating_sub(token_start) >= 12 {
            return Some((start, token_end, "bearer_token"));
        }
    }
    let shapes = [
        ("github_pat_", 24, "github_token"),
        ("ghp_", 20, "github_token"),
        ("xoxb-", 20, "slack_token"),
        ("xoxa-", 20, "slack_token"),
        ("xoxp-", 20, "slack_token"),
        ("sk-", 20, "api_token"),
        ("AKIA", 16, "aws_access_key"),
    ];
    shapes
        .into_iter()
        .filter_map(|(prefix, minimum, rule)| {
            let start = text.find(prefix)?;
            let end = token_end(text, start);
            (end.saturating_sub(start) >= minimum).then_some((start, end, rule))
        })
        .min_by_key(|(start, _, _)| *start)
}

fn token_end(text: &str, start: usize) -> usize {
    text[start..]
        .char_indices()
        .find(|(_, character)| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
        })
        .map_or(text.len(), |(offset, _)| start + offset)
}

fn find_ascii_case_insensitive(text: &str, pattern: &str) -> Option<usize> {
    text.as_bytes()
        .windows(pattern.len())
        .position(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_sensitive_fields_known_values_and_token_shapes() {
        let policy = RedactionPolicy::with_known_values(["super-secret-value".into()]);
        let mut value = json!({
            "api_key": "not-even-a-shaped-token",
            "message": "known=super-secret-value bearer=Bearer abcdefghijklmnop",
            "nested": ["github_pat_abcdefghijklmnopqrstuvwxyz"]
        });
        let report = policy.redact_value(&mut value);
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(report.changed());
        assert!(!rendered.contains("super-secret-value"));
        assert!(!rendered.contains("github_pat_"));
        assert!(!rendered.contains("abcdefghijklmnop"));
        policy.assert_clean(rendered.as_bytes()).unwrap();
    }

    #[test]
    fn json_sanitization_is_stable_and_scannable() {
        let policy = RedactionPolicy::default();
        let input = br#"{"password":"unsafe","ok":"visible"}"#;
        let (sanitized, report) = policy.sanitize_bytes("application/json", input).unwrap();
        assert_eq!(report.redacted_fields, 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&sanitized).unwrap()["ok"],
            "visible"
        );
    }

    #[test]
    fn binary_artifacts_fail_closed_when_a_known_secret_is_present() {
        let policy = RedactionPolicy::with_known_values(["binary-secret".into()]);
        let error = policy
            .sanitize_bytes("application/octet-stream", b"prefix binary-secret suffix")
            .unwrap_err();
        assert!(error.to_string().contains("known_secret"));
    }
}
