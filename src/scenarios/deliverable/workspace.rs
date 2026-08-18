//! Reading and structurally checking the files a build-shaped scenario asks
//! the agent to produce. Everything here is deterministic: no rendering, no
//! execution, no judgement of taste.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub(in crate::scenarios) fn root(scenario_id: &str, run_id: &str) -> PathBuf {
    crate::scenarios::kit::workspace_root(scenario_id, run_id)
}

pub(in crate::scenarios) fn read(root: &Path, relative: &str) -> Option<String> {
    std::fs::read_to_string(root.join(relative)).ok()
}

pub(in crate::scenarios) fn read_json(root: &Path, relative: &str) -> Option<Value> {
    serde_json::from_str(&read(root, relative)?).ok()
}

pub(in crate::scenarios) fn write(
    root: &Path,
    relative: &str,
    contents: &str,
) -> anyhow::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(())
}

pub(in crate::scenarios) fn remove(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

/// Every file under `directory`, relative to the workspace root, sorted.
pub(in crate::scenarios) fn files_under(root: &Path, directory: &str) -> Vec<String> {
    let mut files = Vec::new();
    collect(&root.join(directory), root, &mut files);
    files.sort();
    files
}

fn collect(current: &Path, root: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, files);
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// References to another host, which a self-contained deliverable must not
/// carry. Data URIs and relative paths are fine.
pub(in crate::scenarios) fn external_references(text: &str) -> Vec<String> {
    let mut references = Vec::new();
    for scheme in ["http://", "https://"] {
        let mut rest = text;
        while let Some(index) = rest.find(scheme) {
            let tail = &rest[index..];
            let end = tail
                .find(['"', '\'', ' ', ')', '<', '\n'])
                .unwrap_or(tail.len());
            references.push(tail[..end].to_string());
            rest = &tail[end.max(1)..];
        }
    }
    references.sort();
    references.dedup();
    references
}

pub(in crate::scenarios) fn count_elements(html: &str, tag: &str) -> usize {
    let opening = format!("<{tag}");
    html.match_indices(&opening)
        .filter(|(index, _)| {
            html[*index + opening.len()..]
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || next == '>' || next == '/')
        })
        .count()
}

/// The raw text of every element with this tag name, opening tag only.
pub(in crate::scenarios) fn elements(html: &str, tag: &str) -> Vec<String> {
    let opening = format!("<{tag}");
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(index) = rest.find(&opening) {
        let tail = &rest[index..];
        let end = tail.find('>').map_or(tail.len(), |offset| offset + 1);
        found.push(tail[..end].to_string());
        rest = &tail[end.max(1)..];
    }
    found
}

pub(in crate::scenarios) fn attribute(element: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = element.find(&needle)? + needle.len();
    let tail = &element[start..];
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

pub(in crate::scenarios) fn images_without_alt(html: &str) -> usize {
    elements(html, "img")
        .iter()
        .filter(|element| attribute(element, "alt").is_none_or(|alt| alt.trim().is_empty()))
        .count()
}

/// Local `href` targets, ignoring anchors and external schemes.
pub(in crate::scenarios) fn local_links(html: &str) -> Vec<String> {
    elements(html, "a")
        .iter()
        .filter_map(|element| attribute(element, "href"))
        .filter(|href| {
            !href.starts_with('#') && !href.contains("://") && !href.starts_with("mailto:")
        })
        .collect()
}

/// `a --> b` edges and the nodes a mermaid flowchart declares.
pub(in crate::scenarios) fn mermaid_edges(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (left, right) = line.split_once("-->")?;
            let left = mermaid_node(left)?;
            let right = mermaid_node(right)?;
            Some((left, right))
        })
        .collect()
}

fn mermaid_node(fragment: &str) -> Option<String> {
    let fragment = fragment.trim();
    let name: String = fragment
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_references_ignore_relative_paths() {
        assert!(external_references("<link href=\"./style.css\">").is_empty());
        assert_eq!(
            external_references("<script src=\"https://cdn.example.com/three.js\"></script>"),
            vec!["https://cdn.example.com/three.js".to_string()]
        );
    }

    #[test]
    fn element_counting_does_not_match_longer_tag_names() {
        let html = "<h1>Title</h1><h1 class=\"x\">Second</h1><h10>no</h10>";
        assert_eq!(count_elements(html, "h1"), 2);
    }

    #[test]
    fn images_need_a_non_empty_alt() {
        let html = "<img src=\"a.png\" alt=\"a\"><img src=\"b.png\"><img src=\"c.png\" alt=\"  \">";
        assert_eq!(images_without_alt(html), 2);
    }

    #[test]
    fn only_local_links_are_returned() {
        let html =
            "<a href=\"about.html\">a</a><a href=\"https://x.dev\">b</a><a href=\"#top\">c</a>";
        assert_eq!(local_links(html), vec!["about.html".to_string()]);
    }

    #[test]
    fn mermaid_edges_are_parsed_from_a_flowchart() {
        let source = "flowchart LR\n  client --> gateway\n  gateway --> store\n  %% comment\n";
        assert_eq!(
            mermaid_edges(source),
            vec![
                ("client".to_string(), "gateway".to_string()),
                ("gateway".to_string(), "store".to_string()),
            ]
        );
    }
}
