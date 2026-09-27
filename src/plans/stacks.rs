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

/// Past this a stack is refused before it is read: a workflow input holds
/// 64 KiB, and the text bounds what its aliases can expand to.
const MAX_YAML_BYTES: usize = 32 * 1024;
/// What reading a stack may build once its aliases expand: scalar bytes plus
/// `NODE_COST` per node. Refused past it, before anything is built.
const MAX_EXPANDED_BYTES: usize = 1024 * 1024;
const NODE_COST: usize = 64;
/// Tags the executor's loader (PyYAML's safe loader) constructs.
const SAFE_TAGS: &[&str] = &[
    "str",
    "int",
    "float",
    "bool",
    "null",
    "map",
    "seq",
    "set",
    "omap",
    "pairs",
    "binary",
    "timestamp",
];

/// Read a stack the way Compose does (YAML 1.2: `on`, `no` and dates stay
/// text, only `true`/`false` are booleans; merge keys apply). Refused only
/// when it is past 32 KiB or its aliases expand past 1 MiB (integrity), does
/// not parse, or declares no `containers` mapping.
pub(crate) fn summarize(yaml: &str) -> Result<StackSummary> {
    ensure!(
        yaml.len() <= MAX_YAML_BYTES,
        "A stack is at most 32 KiB of YAML; this one is {} bytes.",
        yaml.len()
    );
    let not_yaml = |error: serde_yaml::Error| anyhow!("The stack is not YAML: {error}");
    let budget = std::cell::Cell::new(MAX_EXPANDED_BYTES);
    match serde::de::DeserializeSeed::deserialize(
        Budget(&budget),
        serde_yaml::Deserializer::from_str(yaml),
    ) {
        Err(_) if budget.get() == 0 => {
            bail!("The stack expands past 1 MiB through its aliases.")
        }
        result => result.map_err(not_yaml)?,
    }
    let mut stack: Value = serde_yaml::from_str(yaml).map_err(not_yaml)?;
    let mut tags = std::collections::BTreeSet::new();
    untag(&mut stack, &mut tags);
    stack.apply_merge().map_err(not_yaml)?;
    let Some(containers) = stack.get("containers").and_then(Value::as_mapping) else {
        bail!("A stack needs a `containers` mapping.");
    };
    // Tags of the `!!` handle Compose drops without a trace: read them in the text.
    for (at, _) in yaml.match_indices("!!") {
        let tag = yaml[at + 2..]
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | ']' | '}'))
            .next()
            .unwrap_or_default();
        if !tag.is_empty() && !SAFE_TAGS.contains(&tag) {
            tags.insert(format!("!!{tag}"));
        }
    }
    let mut warnings = tags
        .iter()
        .map(|tag| format!("The executor refuses tag {tag}."))
        .collect::<Vec<_>>();
    for key in stack.as_mapping().into_iter().flat_map(|top| top.keys()) {
        let key = text(key).unwrap_or_else(|| format!("{key:?}"));
        if !KNOWN_KEYS.contains(&key.as_str()) {
            warnings.push(format!(
                "`{key}` is not a key of a stack (iii, template) or of a Compose project."
            ));
        }
    }
    for key in ["iii", "template"] {
        if let Some(read) = stack.get(key).and_then(|value| not_text(yaml, value)) {
            warnings.push(format!(
                "`{key}` reads as {read}; quote it to keep it as written."
            ));
        }
    }
    let mut listed = Vec::new();
    for (name, container) in containers {
        let name = text(name).unwrap_or_else(|| format!("{name:?}"));
        if !container.is_mapping() {
            warnings.push(format!(
                "{name} is not a mapping; a container is a mapping with `worker:`, and the executor fails on anything else."
            ));
        }
        match container.get("worker").filter(|worker| !worker.is_null()) {
            None if !container.is_mapping() => {}
            None => warnings.push(format!(
                "{name} names no worker; Compose refuses a container without one."
            )),
            Some(Value::String(worker)) if worker.starts_with("path://") => warnings.push(format!(
                "{name} runs {worker}, a path on this machine; the stack runs it only here."
            )),
            Some(Value::String(worker)) if worker.starts_with("package://") => {}
            Some(worker) => warnings.push(format!(
                "{name}: worker {} is neither package:// nor path://.",
                text(worker).unwrap_or_else(|| format!("{worker:?}"))
            )),
        }
        let commit = container.get("commit").filter(|commit| !commit.is_null());
        let package = matches!(container.get("worker"), Some(Value::String(worker)) if worker.starts_with("package://"));
        if commit.is_some() && !package {
            warnings.push(format!(
                "{name} pins a commit, but only a package:// worker is built from one; the executor refuses it."
            ));
        }
        for key in ["version", "commit"] {
            if let Some(read) = container.get(key).and_then(|value| not_text(yaml, value)) {
                warnings.push(format!(
                    "{name}: `{key}` reads as {read}; quote it to keep it as written."
                ));
            }
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

/// What a value that should be text reads as, when it is not text to the
/// executor or to Compose; `None` for text and for an absent (null) value.
/// ponytail: of the texts only leading-zero integers (`0123456`, an int to
/// the executor) are caught, and by the text: the same value quoted in one
/// place and plain in another warns for both. PyYAML's `1_0.5` and `1:30.5`
/// floats are not caught; libyaml events would give the exact style.
fn not_text(yaml: &str, value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => {
            let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
            let leading_zero = digits.len() > 1
                && digits.starts_with('0')
                && digits.bytes().all(|b| b.is_ascii_digit());
            (leading_zero && written_plain(yaml, text)).then(|| {
                format!(
                    "the number {} to the executor",
                    text.parse::<i128>()
                        .map_or_else(|_| text.clone(), |n| n.to_string())
                )
            })
        }
        Value::Number(number) => Some(format!("the number {number}")),
        Value::Bool(value) => Some(format!("the boolean {value}")),
        _ => Some("a list or a mapping".into()),
    }
}

/// Whether `value` is written unquoted somewhere in `yaml`: as a mapping
/// value, a sequence item or a flow entry, up to a line end, comment or
/// flow delimiter.
fn written_plain(yaml: &str, value: &str) -> bool {
    yaml.match_indices(value).any(|(at, _)| {
        let head = &yaml[..at];
        let trimmed = head.trim_end_matches([' ', '\t']);
        let before = match trimmed.chars().next_back() {
            Some(':' | '-' | '?') => trimmed.len() < head.len(),
            Some('[' | '{' | ',') => true,
            _ => false,
        };
        before
            && yaml[at + value.len()..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || matches!(c, '#' | ',' | ']' | '}'))
    })
}

/// Tags of the `!` handle, collected and removed so the rest reads through
/// them.
fn untag(value: &mut Value, tags: &mut std::collections::BTreeSet<String>) {
    if let Value::Tagged(tagged) = value {
        tags.insert(tagged.tag.to_string());
        *value = std::mem::take(&mut tagged.value);
        return untag(value, tags);
    }
    match value {
        Value::Sequence(items) => items.iter_mut().for_each(|item| untag(item, tags)),
        Value::Mapping(entries) => {
            for key in entries.keys() {
                if let Value::Tagged(tagged) = key {
                    tags.insert(tagged.tag.to_string());
                }
            }
            entries.values_mut().for_each(|item| untag(item, tags));
        }
        _ => {}
    }
}

/// Walks a stack as its aliases expand, spending the budget on every node
/// and scalar byte and failing once it runs out, so an alias bomb stops
/// before anything is built.
struct Budget<'a>(&'a std::cell::Cell<usize>);

impl Budget<'_> {
    fn spend<E: serde::de::Error>(&self, bytes: usize) -> Result<(), E> {
        match self.0.get().checked_sub(NODE_COST + bytes) {
            Some(left) => {
                self.0.set(left);
                Ok(())
            }
            None => {
                self.0.set(0);
                Err(E::custom("expansion budget spent"))
            }
        }
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for Budget<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Budget<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("YAML")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_i128<E: serde::de::Error>(self, _: i128) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_u128<E: serde::de::Error>(self, _: u128) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<(), E> {
        self.spend(text.len())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<(), E> {
        self.spend(0)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut items: A) -> Result<(), A::Error> {
        self.spend(0)?;
        while items.next_element_seed(Budget(self.0))?.is_some() {}
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut entries: A) -> Result<(), A::Error> {
        self.spend(0)?;
        while entries.next_key_seed(Budget(self.0))?.is_some() {
            entries.next_value_seed(Budget(self.0))?;
        }
        Ok(())
    }
    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, tagged: A) -> Result<(), A::Error> {
        use serde::de::VariantAccess;
        let ((), value) = tagged.variant_seed(Budget(self.0))?;
        value.newtype_variant_seed(Budget(self.0))
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
            "iii: 0.24.2\ntemplate: yes\ncontainers:\n  on:\n    worker: package://on\n    version: 2026-09-01\n  no:\n    worker: package://no\n    version: off\n  off:\n    worker: package://off\n    version: '1.10'\n",
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
                ("no", Some("off")),
                ("off", Some("1.10"))
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
            "iii: latest\nregistry: https://example.test\ncontainers:\n  bare: {}\n  plain: package://harness\n  empty:\n  local:\n    worker: path://../workers/harness\n  image:\n    worker: docker://harness\n  pinned:\n    worker: package://harness\n    commit: 0123456789abcdef\n  unpinned:\n    worker: package://harness\n    commit: ~\n  built:\n    worker: path://./harness\n    commit: abcdef0\n",
        )
        .unwrap();
        assert_eq!(
            summary.warnings,
            vec![
                "`registry` is not a key of a stack (iii, template) or of a Compose project.",
                "bare names no worker; Compose refuses a container without one.",
                "plain is not a mapping; a container is a mapping with `worker:`, and the executor fails on anything else.",
                "empty is not a mapping; a container is a mapping with `worker:`, and the executor fails on anything else.",
                "local runs path://../workers/harness, a path on this machine; the stack runs it only here.",
                "image: worker docker://harness is neither package:// nor path://.",
                "built runs path://./harness, a path on this machine; the stack runs it only here.",
                "built pins a commit, but only a package:// worker is built from one; the executor refuses it.",
            ]
        );
        assert_eq!(
            summary.containers[summary.containers.len() - 3..summary.containers.len() - 1],
            [
                StackContainer {
                    name: "pinned".into(),
                    version: None,
                    commit: Some("0123456789abcdef".into()),
                },
                StackContainer {
                    name: "unpinned".into(),
                    version: None,
                    commit: None,
                }
            ]
        );
    }

    #[test]
    fn a_value_that_is_not_text_to_the_executor_is_a_warning_to_quote_it() {
        let summary = summarize(
            "iii: 0.24\ntemplate: true\ncontainers:\n  float:\n    worker: package://a\n    version: 1.10\n  exponent: {worker: package://b, version: 1e3}\n  octal:\n    worker: package://c\n    commit: 0123456\n  quoted:\n    worker: package://d\n    version: '1.10'\n    commit: \"0765432\"\n  listed:\n    worker: package://e\n    version: [1]\n",
        )
        .unwrap();
        assert_eq!(
            summary.warnings,
            vec![
                "`iii` reads as the number 0.24; quote it to keep it as written.",
                "`template` reads as the boolean true; quote it to keep it as written.",
                "float: `version` reads as the number 1.1; quote it to keep it as written.",
                "exponent: `version` reads as the number 1000.0; quote it to keep it as written.",
                "octal: `commit` reads as the number 123456 to the executor; quote it to keep it as written.",
                "listed: `version` reads as a list or a mapping; quote it to keep it as written.",
            ]
        );
        assert_eq!(summary.containers[2].commit.as_deref(), Some("0123456"));
    }

    #[test]
    fn a_tag_the_executor_refuses_is_a_warning_and_read_through() {
        let summary = summarize(
            "iii: !!str latest\ncontainers: !reset\n  harness:\n    worker: !env WORKER\n    config_override: !!python/object:os.system {}\n    version: !!int 3\n",
        )
        .unwrap();
        assert_eq!(
            summary.warnings,
            vec![
                "The executor refuses tag !!python/object:os.system.",
                "The executor refuses tag !env.",
                "The executor refuses tag !reset.",
                "harness: worker WORKER is neither package:// nor path://.",
                "harness: `version` reads as the number 3; quote it to keep it as written.",
            ]
        );
        assert_eq!(summary.containers[0].name, "harness");
    }

    #[test]
    fn a_stack_past_32_kib_or_expanding_past_1_mib_is_refused_before_it_is_built() {
        let started = std::time::Instant::now();
        // The bomb that took the worker down: 100 KB behind 30000 aliases.
        let bomb = format!(
            "x: &a {}\ny: [{}]\ncontainers: {{}}\n",
            "a".repeat(100_000),
            vec!["*a"; 30_000].join(", ")
        );
        let error = summarize(&bomb).unwrap_err().to_string();
        assert!(error.contains("at most 32 KiB"), "{error}");
        // Under 32 KiB: a flat one, and one nested a level (8 GB expanded).
        for bomb in [
            format!(
                "x: &a {}\ny: [{}]\ncontainers: {{}}\n",
                "a".repeat(16_000),
                vec!["*a"; 3_000].join(",")
            ),
            format!(
                "s: &s {}\na: &a [{}]\nb: [{}]\ncontainers: {{}}\n",
                "a".repeat(8_000),
                vec!["*s"; 2_000].join(","),
                vec!["*a"; 2_000].join(",")
            ),
        ] {
            assert!(bomb.len() <= MAX_YAML_BYTES, "{}", bomb.len());
            let error = summarize(&bomb).unwrap_err().to_string();
            assert!(error.contains("expands past 1 MiB"), "{error}");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // Aliases a stack means to use still read.
        let shared = summarize(
            "x: &worker package://harness\ncontainers:\n  a: {worker: *worker}\n  b: {worker: *worker}\n",
        )
        .unwrap();
        assert_eq!(shared.containers.len(), 2);
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
