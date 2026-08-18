//! Injectable Harness Console page for the complete E2E dashboard.
//!
//! The frontend build emits an ESM setup module and a fully scoped stylesheet.
//! This module exposes them through one typed internal content function and
//! Message-path `console:script` / `console:style` triggers. The Console owns
//! `console:assets` subscriptions and pushes these registrations to open tabs.

use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::{Error, IIIClient, RegisterFunction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const PAGE_PATH: &str = "harness-e2e/page.js";
pub const STYLES_PATH: &str = "harness-e2e/styles.css";
pub const CONTENT_FUNCTION_ID: &str = "e2e::ui-content";

const PAGE_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/dashboard/dist-console/page.js"
));
const STYLES_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/dashboard/dist-console/styles.css"
));

#[derive(Debug, Deserialize, JsonSchema)]
struct UiContentRequest {
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct UiContentResponse {
    content: String,
    content_type: String,
}

pub fn register(iii: &IIIClient) {
    iii.register_function(
        CONTENT_FUNCTION_ID,
        RegisterFunction::new(|request: UiContentRequest| {
            content(&request.path).ok_or_else(|| {
                Error::Handler(format!(
                    "unknown Harness E2E console asset '{}'; expected {PAGE_PATH} or {STYLES_PATH}",
                    request.path
                ))
            })
        })
        .description("Serve the Harness E2E injectable Console assets.")
        .metadata(json!({ "internal": true })),
    );

    register_asset(iii, "console:script", PAGE_PATH);
    register_asset(iii, "console:style", STYLES_PATH);
}

fn content(path: &str) -> Option<UiContentResponse> {
    match path {
        PAGE_PATH => Some(UiContentResponse {
            content: PAGE_JS.to_owned(),
            content_type: "text/javascript; charset=utf-8".to_owned(),
        }),
        STYLES_PATH => Some(UiContentResponse {
            content: STYLES_CSS.to_owned(),
            content_type: "text/css; charset=utf-8".to_owned(),
        }),
        _ => None,
    }
}

fn register_asset(iii: &IIIClient, trigger_type: &str, path: &str) {
    if let Err(error) = iii.register_trigger(RegisterTriggerInput {
        trigger_type: trigger_type.to_owned(),
        function_id: CONTENT_FUNCTION_ID.to_owned(),
        config: json!({ "path": path }),
        metadata: None,
    }) {
        tracing::warn!(%error, %path, "failed to register Harness E2E Console asset");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;

    #[test]
    fn embedded_assets_match_the_console_contract() {
        assert!(PAGE_JS.contains("export"));
        assert!(PAGE_JS.contains("harness-e2e"));
        assert!(STYLES_CSS.contains("[data-iii-ui=\"harness-e2e\"]"));
        assert!(!STYLES_CSS.contains("@font-face"));
        assert!(PAGE_JS.len() < MAX_ASSET_BYTES);
        assert!(STYLES_CSS.len() < MAX_ASSET_BYTES);
        assert_eq!(console_style_warning_count(STYLES_CSS), 0);
    }

    #[test]
    fn content_function_serves_only_declared_assets() {
        assert_eq!(
            content(PAGE_PATH).unwrap().content_type,
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content(STYLES_PATH).unwrap().content_type,
            "text/css; charset=utf-8"
        );
        assert!(content("harness-e2e/unknown.js").is_none());
    }

    // Mirrors the Console's intentionally cheap top-level selector scan. In
    // particular it splits on every comma, including commas inside :where().
    fn console_style_warning_count(css: &str) -> usize {
        let mut stripped = String::with_capacity(css.len());
        let mut chars = css.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '/' && chars.peek() == Some(&'*') {
                chars.next();
                while let Some(comment) = chars.next() {
                    if comment == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            } else {
                stripped.push(character);
            }
        }

        let mut warnings = usize::from(stripped.contains("@font-face"));
        let mut depth = 0_i32;
        let mut selector = String::new();
        for character in stripped.chars() {
            match character {
                '{' => {
                    if depth == 0 {
                        let candidate = selector.trim();
                        if !candidate.is_empty() && !candidate.starts_with('@') {
                            warnings += candidate
                                .split(',')
                                .map(str::trim)
                                .filter(|part| {
                                    !part.is_empty() && !part.starts_with("[data-iii-ui")
                                })
                                .count();
                        }
                    }
                    depth += 1;
                    selector.clear();
                }
                '}' => {
                    depth = (depth - 1).max(0);
                    selector.clear();
                }
                _ if depth == 0 => selector.push(character),
                _ => {}
            }
        }
        warnings
    }
}
