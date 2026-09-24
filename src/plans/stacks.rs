//! Stacks this Console keeps. A stack is where a suite runs: an iii Compose
//! project plus the executor's keys `iii` and optional `template`, written as
//! `stacks/*.yaml` writes it. The repository's stacks are read-only; a local
//! one starts as a copy of another stack and is edited here, its YAML kept
//! exactly as written. Only YAML that does not parse, or a stack without a
//! `containers` mapping, is refused; everything else is a warning.
use anyhow::{anyhow, bail, ensure, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

/// The repository's stacks, embedded at build time: `stacks/<id>.yaml`.
pub(crate) const REPOSITORY: &[(&str, &str)] = &[
    ("default", include_str!("../../stacks/default.yaml")),
    (
        "harness-template",
        include_str!("../../stacks/harness-template.yaml"),
    ),
];

/// Top-level keys something reads: the executor's, then Compose's.
const KNOWN_KEYS: &[&str] = &[
    "iii",
    "template",
    "containers",
    "namespace",
    "startup_timeout",
    "stop_timeout",
    "required_default",
    "engine",
];

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub(crate) struct LocalStack {
    pub id: String,
    pub label: String,
    /// The stack as written, comments included.
    pub yaml: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct StackCreateRequest {
    /// The stack it starts as a copy of: one of the repository or of this Console.
    pub from: String,
    /// Empty or absent names it after that stack.
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct StackUpdateRequest {
    pub stack_id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The whole stack as YAML, kept exactly as written.
    #[serde(default)]
    pub yaml: Option<String>,
}

/// A container a stack declares, with the version or commit it pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackContainer {
    pub name: String,
    pub version: Option<String>,
    pub commit: Option<String>,
}

/// A stack as the Console lists it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StackView {
    pub id: String,
    pub label: String,
    /// `repository` for a stack of `stacks/` (read-only), `local` for one
    /// this Console keeps.
    pub source: String,
    pub yaml: String,
    /// The iii CLI release it installs.
    pub iii: Option<String>,
    /// The iii-hq/templates project it starts from.
    pub template: Option<String>,
    pub containers: Vec<StackContainer>,
    /// What the executor or Compose may not do as written; never blocking.
    pub warnings: Vec<String>,
    pub updated_at: Option<String>,
}

/// What a stack's YAML declares, and what it may not do as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StackSummary {
    pub iii: Option<String>,
    pub template: Option<String>,
    pub containers: Vec<StackContainer>,
    pub warnings: Vec<String>,
}

impl StackView {
    /// A stored stack that no longer reads is listed with the reason as a
    /// warning, so it can still be opened and fixed.
    pub(crate) fn new(
        id: &str,
        label: &str,
        source: &str,
        yaml: &str,
        updated_at: Option<String>,
    ) -> Self {
        let summary = summarize(yaml).unwrap_or_else(|error| StackSummary {
            iii: None,
            template: None,
            containers: Vec::new(),
            warnings: vec![error.to_string()],
        });
        Self {
            id: id.into(),
            label: label.into(),
            source: source.into(),
            yaml: yaml.into(),
            iii: summary.iii,
            template: summary.template,
            containers: summary.containers,
            warnings: summary.warnings,
            updated_at,
        }
    }
}

impl LocalStack {
    pub(crate) fn apply(&mut self, update: &StackUpdateRequest) {
        if let Some(label) = &update.label {
            self.label = label.trim().to_owned();
        }
        if let Some(yaml) = &update.yaml {
            self.yaml = yaml.clone();
        }
    }

    /// What a stack must be: named, and YAML with a `containers` mapping.
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.label.is_empty()
                && self.label.chars().count() <= 160
                && !self.label.chars().any(char::is_control),
            "Name the stack (up to 160 characters, without control characters)."
        );
        summarize(&self.yaml).map(|_| ())
    }
}

/// Read a stack the way Compose does (YAML 1.2: `on`, `no` and dates stay
/// text, only `true`/`false` are booleans; merge keys apply). Refused only
/// when it does not parse or declares no `containers` mapping.
pub(crate) fn summarize(yaml: &str) -> Result<StackSummary> {
    let mut stack: Value =
        serde_yaml::from_str(yaml).map_err(|error| anyhow!("The stack is not YAML: {error}"))?;
    stack
        .apply_merge()
        .map_err(|error| anyhow!("The stack is not YAML: {error}"))?;
    let Some(containers) = stack.get("containers").and_then(Value::as_mapping) else {
        bail!("A stack needs a `containers` mapping.");
    };
    let mut warnings = Vec::new();
    for key in stack.as_mapping().into_iter().flat_map(|top| top.keys()) {
        let key = text(key).unwrap_or_else(|| format!("{key:?}"));
        if !KNOWN_KEYS.contains(&key.as_str()) {
            warnings.push(format!(
                "`{key}` is not a key of a stack (iii, template) or of a Compose project."
            ));
        }
    }
    let mut listed = Vec::new();
    for (name, container) in containers {
        let name = text(name).unwrap_or_else(|| format!("{name:?}"));
        match container
            .get("worker")
            .map(|worker| text(worker).ok_or(worker))
        {
            None | Some(Err(Value::Null)) => warnings.push(format!(
                "{name} names no worker; Compose refuses a container without one."
            )),
            Some(Ok(worker)) if worker.starts_with("path://") => warnings.push(format!(
                "{name} runs {worker}, a path on this machine; the stack runs it only here."
            )),
            Some(Ok(worker)) if worker.starts_with("package://") => {}
            Some(worker) => warnings.push(format!(
                "{name}: worker {} is neither package:// nor path://.",
                worker.unwrap_or_else(|value| format!("{value:?}"))
            )),
        }
        let commit = container.get("commit");
        if commit.is_some() {
            warnings.push(format!(
                "{name} pins a commit; it takes effect once the executor runs commit pins."
            ));
        }
        listed.push(StackContainer {
            name,
            version: container.get("version").and_then(text),
            commit: commit.and_then(text),
        });
    }
    Ok(StackSummary {
        iii: stack.get("iii").and_then(text),
        template: stack.get("template").and_then(text),
        containers: listed,
        warnings,
    })
}

/// A scalar as the text it reads as.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_stacks_are_every_file_of_stacks_and_read_without_warnings() {
        let mut files = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/stacks"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "yaml")
            })
            .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        files.sort();
        let mut embedded = REPOSITORY
            .iter()
            .map(|(id, _)| (*id).to_owned())
            .collect::<Vec<_>>();
        embedded.sort();
        assert_eq!(embedded, files);
        for (id, yaml) in REPOSITORY {
            let summary = summarize(yaml).unwrap();
            assert_eq!(summary.warnings, Vec::<String>::new(), "{id}");
            assert_eq!(summary.iii.as_deref(), Some("latest"), "{id}");
            assert!(
                summary
                    .containers
                    .iter()
                    .any(|container| container.name == "harness-e2e"
                        && container.version.as_deref() == Some("latest")),
                "{id}"
            );
        }
        assert_eq!(summarize(REPOSITORY[0].1).unwrap().template, None);
        assert_eq!(
            summarize(REPOSITORY[1].1).unwrap().template.as_deref(),
            Some("harness")
        );
    }

    #[test]
    fn a_stack_reads_as_yaml_1_2() {
        let summary = summarize(
            "iii: 0.24.2\ntemplate: yes\ncontainers:\n  on:\n    worker: package://on\n    version: 2026-09-01\n  no:\n    worker: package://no\n    version: 0o17\n  off:\n    worker: package://off\n    version: true\n",
        )
        .unwrap();
        assert_eq!(summary.iii.as_deref(), Some("0.24.2"));
        assert_eq!(summary.template.as_deref(), Some("yes"));
        let containers = summary
            .containers
            .iter()
            .map(|container| (container.name.as_str(), container.version.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            containers,
            vec![
                ("on", Some("2026-09-01")),
                ("no", Some("15")),
                ("off", Some("true"))
            ]
        );
        assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
        // A merge key brings what it names.
        let merged = summarize(
            "x-worker: &harness\n  worker: package://harness\ncontainers:\n  harness:\n    <<: *harness\n    version: 1.0.0\n",
        )
        .unwrap();
        assert_eq!(
            merged.warnings,
            vec!["`x-worker` is not a key of a stack (iii, template) or of a Compose project."]
        );
    }

    #[test]
    fn only_yaml_that_does_not_parse_or_a_stack_without_containers_is_refused() {
        for (yaml, reason) in [
            ("containers: [", "not YAML"),
            ("containers:\n  a: 1\n  a: 2\n", "not YAML"),
            ("", "`containers` mapping"),
            ("- containers\n", "`containers` mapping"),
            ("iii: latest\n", "`containers` mapping"),
            ("containers:\n  - harness\n", "`containers` mapping"),
        ] {
            let error = summarize(yaml).unwrap_err().to_string();
            assert!(error.contains(reason), "{yaml:?}: {error}");
        }
        assert!(summarize("containers: {}\n").unwrap().containers.is_empty());
    }

    #[test]
    fn what_may_not_run_as_written_is_a_warning() {
        let summary = summarize(
            "iii: latest\nregistry: https://example.test\ncontainers:\n  bare: {}\n  plain: package://harness\n  local:\n    worker: path://../workers/harness\n  image:\n    worker: docker://harness\n  pinned:\n    worker: package://harness\n    commit: 0123456789abcdef\n",
        )
        .unwrap();
        assert_eq!(
            summary.warnings,
            vec![
                "`registry` is not a key of a stack (iii, template) or of a Compose project.",
                "bare names no worker; Compose refuses a container without one.",
                "plain names no worker; Compose refuses a container without one.",
                "local runs path://../workers/harness, a path on this machine; the stack runs it only here.",
                "image: worker docker://harness is neither package:// nor path://.",
                "pinned pins a commit; it takes effect once the executor runs commit pins.",
            ]
        );
        assert_eq!(
            summary.containers.last().unwrap(),
            &StackContainer {
                name: "pinned".into(),
                version: None,
                commit: Some("0123456789abcdef".into()),
            }
        );
    }

    #[test]
    fn a_stack_is_named_and_keeps_its_text_as_written() {
        let yaml = "# mine\ncontainers:   # kept\n  harness: {worker: package://harness}\n";
        let mut stack = LocalStack {
            id: "stack-0123456789ab".into(),
            label: "Mine".into(),
            yaml: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        stack.apply(&StackUpdateRequest {
            yaml: Some(yaml.into()),
            label: Some("  Mine, edited ".into()),
            ..StackUpdateRequest::default()
        });
        stack.validate().unwrap();
        assert_eq!(
            (stack.label.as_str(), stack.yaml.as_str()),
            ("Mine, edited", yaml)
        );
        stack.apply(&StackUpdateRequest {
            label: Some(" ".into()),
            ..StackUpdateRequest::default()
        });
        let error = stack.validate().unwrap_err().to_string();
        assert!(error.contains("Name the stack"), "{error}");
    }
}
