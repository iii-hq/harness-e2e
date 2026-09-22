//! The screenshots a JSON deliverable carries, named in the run report.
//!
//! Scenarios embed images as base64 files under `attachments` (Kanban) or
//! `files` (registry, trending topics), with `captures.json` manifests naming
//! the browser captures. A reader of the report, the release-control console
//! among them, can then say how many screenshots a run has and what they show
//! without downloading the artifact that holds the bytes.

use std::collections::BTreeMap;

use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotReference {
    /// JSON Pointer to the embedded file inside the deliverable: `/attachments/board-desktop.png`.
    pub pointer: String,
    /// The capture's caption from its `captures.json`, else the file's path
    /// without the attempt folder it was packed under.
    pub caption: String,
    pub media_type: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Every PNG or JPEG embedded in a deliverable's `attachments` or `files`. A
/// file a `captures.json` lists as anything but `captured` is left out, as the
/// console leaves it out.
pub fn embedded_screenshots(content: &Value) -> Vec<ScreenshotReference> {
    let mut screenshots = Vec::new();
    for field in ["attachments", "files"] {
        let Some(files) = content.get(field).and_then(Value::as_object) else {
            continue;
        };
        let captures = captures(files);
        for (name, file) in files {
            let Some((bytes, media_type)) = image(file) else {
                continue;
            };
            let caption = match captures.get(name.as_str()) {
                Some(Some(caption)) => caption.clone(),
                Some(None) => continue,
                None => without_bundle_folder(name).to_string(),
            };
            screenshots.push(ScreenshotReference {
                pointer: format!("/{field}/{}", name.replace('~', "~0").replace('/', "~1")),
                caption,
                media_type: media_type.to_string(),
                sha256: crate::artifact::sha256_bytes(&bytes),
                size_bytes: bytes.len() as u64,
            });
        }
    }
    screenshots
}

/// Each file a `captures.json` names: its caption when captured, `None` when not.
fn captures(files: &serde_json::Map<String, Value>) -> BTreeMap<String, Option<String>> {
    let mut named = BTreeMap::new();
    for (path, file) in files {
        if file_name(path) != "captures.json" || file["encoding"] != "utf8" {
            continue;
        }
        let Some(manifest) = file["content"]
            .as_str()
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
        else {
            continue;
        };
        let directory = path.rsplit_once('/').map_or("", |(directory, _)| directory);
        for capture in manifest["captures"].as_array().into_iter().flatten() {
            let Some(screenshot) = capture["screenshot"].as_str() else {
                continue;
            };
            let Some(screenshot) = relative(directory, screenshot) else {
                continue;
            };
            let caption = (capture["status"] == "captured").then(|| {
                capture["caption"]
                    .as_str()
                    .or_else(|| capture["id"].as_str())
                    .unwrap_or("UI evidence")
                    .to_string()
            });
            named.insert(screenshot, caption);
        }
    }
    named
}

/// `directory/screenshot` with `.` segments dropped; `None` when it climbs out.
fn relative(directory: &str, screenshot: &str) -> Option<String> {
    let mut path = Vec::new();
    for segment in directory.split('/').chain(screenshot.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                path.pop()?;
            }
            segment => path.push(segment),
        }
    }
    Some(path.join("/"))
}

fn image(file: &Value) -> Option<(Vec<u8>, &'static str)> {
    if file["encoding"] != "base64" {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file["content"].as_str()?)
        .ok()?;
    let media_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else {
        return None;
    };
    Some((bytes, media_type))
}

/// `registry_verification-<sha256>/workspace/output/a.png` → `workspace/output/a.png`:
/// the attempt folder names nothing a reader needs, the rest of the path may
/// (`original/desktop/home.png` and `varied/mobile/home.png` share a file name).
fn without_bundle_folder(path: &str) -> &str {
    match path.split_once('/') {
        Some((folder, rest))
            if folder.len() > 65
                && folder.as_bytes()[folder.len() - 65] == b'-'
                && folder[folder.len() - 64..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            rest
        }
        _ => path,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0];

    fn encoded(bytes: &[u8]) -> Value {
        json!({"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(bytes)})
    }

    #[test]
    fn names_kanban_attachments_by_file() {
        let content = json!({"attachments": {
            "board-desktop.png": {"media_type":"image/png","encoding":"base64",
                "content":base64::engine::general_purpose::STANDARD.encode(PNG)},
            "controller.log": {"media_type":"text/plain","content":"ok"},
        }});
        assert_eq!(
            embedded_screenshots(&content),
            vec![ScreenshotReference {
                pointer: "/attachments/board-desktop.png".into(),
                caption: "board-desktop.png".into(),
                media_type: "image/png".into(),
                sha256: crate::artifact::sha256_bytes(PNG),
                size_bytes: PNG.len() as u64,
            }]
        );
    }

    #[test]
    fn takes_captions_from_captures_and_drops_what_was_not_captured() {
        let captures = json!({"captures": [
            {"id":"changelog","status":"captured","screenshot":"changelog.jpg","caption":"Changelog history for orders-worker"},
            {"id":"compare","status":"unavailable","screenshot":"compare.jpg","caption":"Comparison"},
        ]});
        let content = json!({"files": {
            "run-1/screenshots/captures.json": {"encoding":"utf8","content":captures.to_string()},
            "run-1/screenshots/changelog.jpg": encoded(JPEG),
            "run-1/screenshots/compare.jpg": encoded(JPEG),
            "registry_verification-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/workspace/output/screenshots/03-changelog-history.png": encoded(PNG),
            "run-1/workspace/output/report.md": {"encoding":"utf8","content":"observed"},
        }});
        let screenshots = embedded_screenshots(&content);
        let captions = screenshots
            .iter()
            .map(|screenshot| (screenshot.pointer.as_str(), screenshot.caption.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            captions,
            vec![
                (
                    "/files/registry_verification-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef~1workspace~1output~1screenshots~103-changelog-history.png",
                    "workspace/output/screenshots/03-changelog-history.png"
                ),
                (
                    "/files/run-1~1screenshots~1changelog.jpg",
                    "Changelog history for orders-worker"
                ),
            ]
        );
        assert_eq!(screenshots[1].media_type, "image/jpeg");
    }

    #[test]
    fn ignores_text_and_bytes_that_are_not_images() {
        let content = json!({"files": {
            "a.png": {"encoding":"base64","content":"not base64!"},
            "b.bin": encoded(b"plain bytes"),
        }});
        assert!(embedded_screenshots(&content).is_empty());
        assert!(embedded_screenshots(&json!("text deliverable")).is_empty());
    }
}
