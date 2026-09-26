use std::collections::BTreeSet;
use std::sync::OnceLock;

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
    "TYPESAFE_API_KEY",
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

/// Shorter values are not looked for: they would match anywhere.
pub const MIN_CREDENTIAL_LENGTH: usize = 8;

/// The credentials this process received, by name, longest value first:
/// the defaults above, the provider keys `config/provider-credentials.json`
/// lists, and the names `HARNESS_E2E_SECRET_ENV_NAMES` and
/// `HARNESS_E2E_CREDENTIALS` (what the launcher put in the stack's `.env`)
/// give, each as its variable holds it.
fn environment_credentials() -> Vec<(String, String)> {
    credentials_from(|name| std::env::var(name).ok())
}

fn credentials_from(variable: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut names = DEFAULT_SECRET_ENV_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .chain(crate::plans::credentials::known_names())
        .collect::<BTreeSet<_>>();
    for list in ["HARNESS_E2E_SECRET_ENV_NAMES", "HARNESS_E2E_CREDENTIALS"] {
        if let Some(extra) = variable(list) {
            names.extend(
                extra
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
    }
    let mut credentials = names
        .into_iter()
        .filter_map(|name| {
            let value = variable(&name)?.trim().to_string();
            (value.len() >= MIN_CREDENTIAL_LENGTH).then_some((name, value))
        })
        .collect::<Vec<_>>();
    credentials.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    credentials
}

#[cfg(test)]
thread_local! {
    static TEST_CREDENTIALS: std::cell::RefCell<Option<Vec<(String, String)>>> =
        const { std::cell::RefCell::new(None) };
}

fn with_credentials<T>(use_them: impl FnOnce(&[(String, String)]) -> T) -> T {
    #[cfg(test)]
    if let Some(credentials) = TEST_CREDENTIALS.with(|cell| cell.borrow().clone()) {
        return use_them(&credentials);
    }
    static CREDENTIALS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    use_them(CREDENTIALS.get_or_init(environment_credentials))
}

/// Run `body` as if this thread's process had received `credentials`.
#[cfg(test)]
pub(crate) fn with_test_credentials<T>(
    credentials: &[(&str, &str)],
    body: impl FnOnce() -> T,
) -> T {
    let mut owned = credentials
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    owned.sort_by_key(|credential| std::cmp::Reverse(credential.1.len()));
    TEST_CREDENTIALS.with(|cell| *cell.borrow_mut() = Some(owned));
    let result = body();
    TEST_CREDENTIALS.with(|cell| *cell.borrow_mut() = None);
    result
}

/// `bytes` with each credential this process received replaced by
/// `[redacted:NAME]`, as written and as JSON escapes it. Every artifact the
/// runner writes passes through here before it is hashed, so the digests
/// that reference it are over what is kept. Bytes that are not UTF-8
/// (images) are kept as they are.
pub fn redact_credentials(bytes: Vec<u8>) -> Vec<u8> {
    let mut text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(binary) => return binary.into_bytes(),
    };
    with_credentials(|credentials| {
        for (name, value) in credentials {
            let escaped = serde_json::to_string(value).unwrap_or_default();
            let escaped = escaped.trim_matches('"');
            for form in [escaped, value.as_str()] {
                if !form.is_empty() && text.contains(form) {
                    text = text.replace(form, &format!("[redacted:{name}]"));
                }
            }
        }
    });
    text.into_bytes()
}

#[derive(Debug, Clone, Default)]
pub struct RedactionPolicy {
    known_values: Vec<String>,
}

impl RedactionPolicy {
    /// The values `redact_credentials` replaces: what the runner redacts
    /// from a deliverable before hashing it is what an artifact holds.
    pub fn from_environment() -> Self {
        Self {
            known_values: with_credentials(|credentials| {
                credentials.iter().map(|(_, value)| value.clone()).collect()
            }),
        }
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
        .chain(jwt(text).map(|(start, end)| (start, end, "jwt")))
        .min_by_key(|(start, _, _)| *start)
}

/// The first JWT, as OAuth access tokens are: three base64url segments
/// joined by dots, the header and the payload JSON objects (`eyJ`, base64
/// of `{"`). Base64 as an image in JSON is encoded has no dot, so it never
/// reads as one.
fn jwt(text: &str) -> Option<(usize, usize)> {
    let segment_end = |from: usize| {
        text[from..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .map_or(text.len(), |offset| from + offset)
    };
    let mut from = 0;
    while let Some(offset) = text[from..].find("eyJ") {
        let start = from + offset;
        let header = segment_end(start);
        if text[header..].starts_with(".eyJ") {
            let payload = segment_end(header + 1);
            if text[payload..].starts_with('.') {
                let end = segment_end(payload + 1);
                if end - start >= 40 {
                    return Some((start, end));
                }
            }
        }
        from = start + 3;
    }
    None
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
    fn redacts_a_jwt_whole_and_leaves_short_lookalikes() {
        let policy = RedactionPolicy::default();
        let token = "eyJhbGciOiJSUzI1NiJ9.eyJleHAiOjE5MDAwMDAwMDB9.c2lnbmF0dXJlLWJ5dGVz";
        let (text, report) = policy.redact_text(&format!("token={token} ok"));
        assert_eq!(text, "token=[REDACTED] ok");
        assert!(report.rules.contains("jwt"));
        assert!(policy.assert_clean(token.as_bytes()).is_err());
        // Unsigned, still one.
        let unsigned = "eyJhbGciOiJub25lIn0.eyJleHAiOjE5MDAwMDAwMDAsImEiOjF9.";
        assert_eq!(policy.redact_text(unsigned).0, "[REDACTED]");
        for lookalike in [
            "eyJhbGciOi",
            "eyJhbGciOiJSUzI1NiJ9eyJleHAiOjE5MDAwMDAwMDB9c2lnbmF0dXJl",
            "eyJhbGciOiJSUzI1NiJ9.notapayloadatallbutlongenough.sig",
        ] {
            assert_eq!(policy.redact_text(lookalike).0, lookalike);
        }
    }

    #[test]
    fn an_image_in_json_is_never_read_as_a_jwt() {
        use base64::Engine as _;
        let policy = RedactionPolicy::default();
        // Screenshots whose base64 holds `eyJ` and a long run after it.
        for png in [
            &include_bytes!(
                "../docs/design/restructure-stage4-2026-09-11/comparison-by-test-dark.png"
            )[..],
            &include_bytes!(
                "../docs/design/restructure-stage5-2026-09-11/comparison-grouped-light.png"
            )[..],
        ] {
            let encoded = base64::engine::general_purpose::STANDARD.encode(png);
            assert!(encoded.contains("eyJ"));
            let evidence =
                serde_json::to_vec(&json!({"files": [{"name": "board.png", "base64": encoded}]}))
                    .unwrap();
            let (kept, report) = policy
                .sanitize_bytes("application/json", &evidence)
                .unwrap();
            assert_eq!(kept, evidence);
            assert!(!report.changed());
            policy.assert_clean(&evidence).unwrap();
        }
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
    fn a_received_credential_is_redacted_by_name_as_written_and_as_json_escapes_it() {
        let quoted = r#"sk-"quoted\key"#;
        with_test_credentials(
            &[
                ("OPENAI_API_KEY", "sk-openai-0123456789"),
                ("ODD_KEY", quoted),
            ],
            || {
                let json = serde_json::to_vec(&serde_json::json!({
                    "output": "OPENAI_API_KEY=sk-openai-0123456789",
                    "odd": quoted,
                }))
                .unwrap();
                let redacted = redact_credentials(json);
                let value: Value = serde_json::from_slice(&redacted).unwrap();
                assert_eq!(value["output"], "OPENAI_API_KEY=[redacted:OPENAI_API_KEY]");
                assert_eq!(value["odd"], "[redacted:ODD_KEY]");
                // As written, in text; an image is kept as it is.
                assert_eq!(
                    redact_credentials(format!("x {quoted} y").into_bytes()),
                    b"x [redacted:ODD_KEY] y"
                );
                let png = [0x89, b'P', b'N', b'G', 0xff, 0xfe];
                assert_eq!(redact_credentials(png.to_vec()), png);
                // What deliverables are redacted by before they are hashed.
                assert!(RedactionPolicy::from_environment()
                    .findings("sk-openai-0123456789")
                    .contains("known_secret"));
            },
        );
    }

    #[test]
    fn the_runner_redacts_the_defaults_the_catalog_and_the_names_it_is_given() {
        let environment = std::collections::HashMap::from([
            ("HARNESS_E2E_CREDENTIALS", "MY_GATEWAY_TOKEN, OTHER_KEY"),
            ("MY_GATEWAY_TOKEN", " gw-0123456789 "),
            ("OTHER_KEY", "short"),
            ("DEEPSEEK_API_KEY", "sk-deepseek-0123456789"),
            ("GITHUB_TOKEN", "ghs_0123456789"),
            ("NOT_NAMED", "never-0123456789"),
            // A subscription login's access token, named by the catalog.
            ("CLAUDE_CODE_ACCESS_TOKEN", "sk-ant-oat01-0123456789"),
        ]);
        assert_eq!(
            credentials_from(|name| environment.get(name).map(|value| (*value).to_owned())),
            [
                (
                    "CLAUDE_CODE_ACCESS_TOKEN".to_owned(),
                    "sk-ant-oat01-0123456789".to_owned()
                ),
                (
                    "DEEPSEEK_API_KEY".to_owned(),
                    "sk-deepseek-0123456789".to_owned()
                ),
                ("GITHUB_TOKEN".to_owned(), "ghs_0123456789".to_owned()),
                ("MY_GATEWAY_TOKEN".to_owned(), "gw-0123456789".to_owned()),
            ]
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
