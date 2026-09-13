//! Deterministic checks for the Registry planning deliverable.
//!
//! `registry_planning` asks the subject for an implementation plan and scores
//! it against the twenty binary planning metrics in `metrics.json`. Each
//! metric is answered here by inspecting the plan text itself: contract
//! terms that must be stated, pinned-source paths that must be cited, an
//! ordered implementation sequence, proposed tests that carry an expected
//! result, and the absence of excluded work. Every observation cites the plan
//! lines it matched, so the awarded points are reproducible from the
//! deliverable alone. The checks are structural: they prove that the plan
//! states the contract, not that the stated plan would work.

use serde_json::{json, Value};

pub(super) const VALIDATOR: &str = "deterministic-plan-checks/v1";

/// Backend files at the pinned Registry commit that can host or call the
/// comparison behaviour (reference plan, section 3).
const BACKEND_PATHS: &[&str] = &[
    "api/src/services/worker.service.ts",
    "api/src/repositories/worker.repository.ts",
    "api/src/controllers/",
    "api/src/main.ts",
    "api/src/lib/types.ts",
    "api/src/db/schema.ts",
    "api/src/lib/",
];
/// Frontend files at the pinned Registry commit that participate in the
/// worker page (reference plan, section 3).
const FRONTEND_PATHS: &[&str] = &[
    "app/src/app/workers/[slug]/page.tsx",
    "app/src/components/page-tabs.tsx",
    "app/src/components/versions-panel.tsx",
    "app/src/lib/data.ts",
    "app/src/lib/types.ts",
];
/// Phrases that state set semantics for schema `required` and `enum` arrays.
const SET_TERMS: &[&str] = &[
    "as sets",
    "as a set",
    "set semantics",
    "set comparison",
    "set equality",
    "order-insensitive",
    "order insensitive",
    "unordered",
    "ignore order",
    "ignoring order",
    "ignores order",
    "regardless of order",
    "irrespective of order",
    "sorted before compar",
    "sort before compar",
    "normalize order",
    "normalise order",
];
/// Work the requirements exclude from the deliverable. A plan may mention
/// these only in a sentence that rules them out.
const EXCLUDED_WORK: &[&str] = &[
    "download chart",
    "binary-size chart",
    "binary size chart",
    "30-day download",
    "30 day download",
    "download statistics",
    "cache-hit",
    "cache hit",
    "json patch library",
    "json-patch library",
    "database migration",
    "new migration",
    "publish",
    "promote",
];
const NEGATIONS: &[&str] = &[
    "not ",
    "no ",
    "never",
    "exclud",
    "out of scope",
    "outside",
    "without",
    "avoid",
    "skip",
    "beyond",
    "n't",
];
const EXPECTATION_TERMS: &[&str] = &[
    "expect", "should", "must", "return", "assert", "->", "→", "yield", "produce", "result",
    "respond", "show", "display", "render", "receive", "==", "equal", "contain", "fail", "succeed",
    "error", "status", "reject", "accept",
];

struct Line {
    normalized: String,
    original: String,
}

struct Section<'a> {
    title: String,
    lines: &'a [Line],
}

struct Document {
    lines: Vec<Line>,
    text: String,
}

struct Outcome {
    passed: bool,
    reason: String,
    evidence: Vec<String>,
}

fn normalize(line: &str) -> String {
    line.replace('`', "")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Document {
    fn new(plan: &str) -> Self {
        let lines = plan
            .lines()
            .map(|line| Line {
                normalized: normalize(line),
                original: line.trim().chars().take(240).collect(),
            })
            .collect::<Vec<_>>();
        let text = lines
            .iter()
            .map(|line| line.normalized.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Self { lines, text }
    }

    fn find(&self, term: &str) -> Option<&Line> {
        self.lines
            .iter()
            .find(|line| line.normalized.contains(term))
    }

    fn find_any<'a>(&self, terms: &[&'a str]) -> Option<(&'a str, &Line)> {
        terms
            .iter()
            .find_map(|term| self.find(term).map(|line| (*term, line)))
    }

    /// Markdown sections: each heading with the lines up to the next heading
    /// of the same or a higher level.
    fn sections(&self) -> Vec<Section<'_>> {
        let mut sections = Vec::new();
        let headings = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| heading_level(&line.normalized).map(|level| (index, level)))
            .collect::<Vec<_>>();
        for (position, (start, level)) in headings.iter().enumerate() {
            let end = headings[position + 1..]
                .iter()
                .find(|(_, next_level)| *next_level <= *level)
                .map(|(index, _)| *index)
                .unwrap_or(self.lines.len());
            sections.push(Section {
                title: self.lines[*start]
                    .normalized
                    .trim_start_matches('#')
                    .trim()
                    .to_string(),
                lines: &self.lines[*start + 1..end],
            });
        }
        sections
    }
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (hashes > 0 && line[hashes..].starts_with(' ')).then_some(hashes)
}

fn is_numbered_item(line: &str) -> bool {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    (digits > 0 && matches!(line[digits..].chars().next(), Some('.') | Some(')')))
        || line.starts_with("step ")
}

fn is_list_item(line: &str) -> bool {
    line.starts_with("- ") || line.starts_with("* ") || is_numbered_item(line)
}

fn table_cells(line: &str) -> Option<Vec<&str>> {
    if !line.starts_with('|') {
        return None;
    }
    let cells = line
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    if cells
        .iter()
        .all(|cell| cell.chars().all(|character| matches!(character, '-' | ':')))
    {
        return None;
    }
    Some(cells)
}

/// Every group must be matched by at least one of its terms.
fn all_groups(document: &Document, groups: &[&[&str]]) -> Outcome {
    let mut matched = Vec::new();
    let mut missing = Vec::new();
    let mut evidence = Vec::new();
    for group in groups {
        match document.find_any(group) {
            Some((term, line)) => {
                matched.push(term.to_string());
                if !evidence.contains(&line.original) {
                    evidence.push(line.original.clone());
                }
            }
            None => missing.push(group.join(" | ")),
        }
    }
    Outcome {
        passed: missing.is_empty(),
        reason: if missing.is_empty() {
            format!("plan states {}", matched.join(", "))
        } else {
            format!(
                "plan does not state any of: {}",
                missing
                    .iter()
                    .map(|group| format!("[{group}]"))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        },
        evidence,
    }
}

fn any_path(document: &Document, paths: &[&str], label: &str) -> Outcome {
    match document.find_any(paths) {
        Some((path, line)) => Outcome {
            passed: true,
            reason: format!("plan cites the pinned {label} path {path}"),
            evidence: vec![line.original.clone()],
        },
        None => Outcome {
            passed: false,
            reason: format!(
                "plan cites none of the pinned {label} integration paths: {}",
                paths.join(", ")
            ),
            evidence: Vec::new(),
        },
    }
}

fn first_item_with<'a>(items: &[&'a Line], terms: &[&str]) -> Option<(usize, &'a Line)> {
    items
        .iter()
        .enumerate()
        .find(|(_, line)| terms.iter().any(|term| line.normalized.contains(term)))
        .map(|(index, line)| (index, *line))
}

/// The ordered implementation sequence must schedule the backend comparison
/// no later than the interface that consumes it and the tests that verify it.
fn dependency_order(document: &Document) -> Outcome {
    const SEQUENCE_TITLES: &[&str] = &[
        "sequence",
        "steps",
        "order",
        "phase",
        "milestone",
        "implementation plan",
        "work plan",
        "roadmap",
        "timeline",
    ];
    const BACKEND: &[&str] = &["api", "endpoint", "backend", "server", "compar", "service"];
    const FRONTEND: &[&str] = &[
        "tab",
        "frontend",
        "ui ",
        "page",
        "component",
        "render",
        "interface",
    ];
    const TESTS: &[&str] = &["test", "verif", "playwright", "vitest", "e2e"];
    let sections = document.sections();
    let sequence = sections
        .iter()
        .filter(|section| {
            SEQUENCE_TITLES
                .iter()
                .any(|title| section.title.contains(title))
        })
        .map(|section| {
            let items = section
                .lines
                .iter()
                .filter(|line| is_numbered_item(&line.normalized))
                .collect::<Vec<_>>();
            (section, items)
        })
        .max_by_key(|(_, items)| items.len());
    let Some((section, items)) = sequence.filter(|(_, items)| items.len() >= 3) else {
        return Outcome {
            passed: false,
            reason:
                "plan has no implementation sequence section with at least three numbered steps"
                    .into(),
            evidence: Vec::new(),
        };
    };
    let backend = first_item_with(&items, BACKEND);
    let frontend = first_item_with(&items, FRONTEND);
    let tests = first_item_with(&items, TESTS);
    let Some((backend_index, backend_line)) = backend else {
        return Outcome {
            passed: false,
            reason: format!(
                "sequence '{}' never schedules the backend comparison",
                section.title
            ),
            evidence: Vec::new(),
        };
    };
    let consumers_follow = frontend.is_none_or(|(index, _)| backend_index <= index)
        && tests.is_none_or(|(index, _)| backend_index <= index);
    let mut evidence = vec![backend_line.original.clone()];
    for (_, line) in frontend.into_iter().chain(tests) {
        if !evidence.contains(&line.original) {
            evidence.push(line.original.clone());
        }
    }
    Outcome {
        passed: consumers_follow,
        reason: format!(
            "sequence '{}': backend step {}, interface step {}, verification step {}",
            section.title,
            backend_index + 1,
            frontend.map_or("absent".to_string(), |(index, _)| (index + 1).to_string()),
            tests.map_or("absent".to_string(), |(index, _)| (index + 1).to_string()),
        ),
        evidence,
    }
}

/// Every proposed test in the plan's main test or check section (the one
/// listing the most items) must state an observable expected result, either
/// as a table column or as an expectation phrase.
fn test_expectations(document: &Document) -> Outcome {
    const TEST_TITLES: &[&str] = &[
        "test",
        "check",
        "verification",
        "validation",
        "acceptance",
        "expected",
    ];
    const NOT_TEST_TITLES: &[&str] = &["sequence", "implementation", "steps", "order", "risk"];
    let sections = document.sections();
    let mut best: Option<(usize, Vec<String>, Vec<String>)> = None;
    for section in sections.iter().filter(|section| {
        TEST_TITLES
            .iter()
            .any(|title| section.title.contains(title))
            && !NOT_TEST_TITLES
                .iter()
                .any(|title| section.title.contains(title))
    }) {
        let mut items = 0_usize;
        let mut without = Vec::new();
        let mut with = Vec::new();
        let mut header_seen = false;
        for line in section.lines {
            if let Some(cells) = table_cells(&line.normalized) {
                if !header_seen {
                    header_seen = true;
                    continue;
                }
                items += 1;
                if cells.iter().skip(1).any(|cell| !cell.is_empty()) {
                    with.push(line.original.clone());
                } else {
                    without.push(line.original.clone());
                }
            } else if is_list_item(&line.normalized) {
                items += 1;
                if EXPECTATION_TERMS
                    .iter()
                    .any(|term| line.normalized.contains(term))
                {
                    with.push(line.original.clone());
                } else {
                    without.push(line.original.clone());
                }
            }
        }
        if best.as_ref().is_none_or(|(count, _, _)| items > *count) {
            best = Some((items, with, without));
        }
    }
    let (items, with, without) = best.unwrap_or_default();
    if items == 0 {
        return Outcome {
            passed: false,
            reason: "plan has no test or check section listing proposed tests".into(),
            evidence: Vec::new(),
        };
    }
    Outcome {
        passed: without.is_empty(),
        reason: format!(
            "{} of {items} proposed tests state an expected result",
            items - without.len()
        ),
        evidence: if without.is_empty() {
            with.into_iter().take(3).collect()
        } else {
            without.into_iter().take(3).collect()
        },
    }
}

/// Excluded work may appear only in sentences that rule it out.
fn scope(document: &Document) -> Outcome {
    let mut violations = Vec::new();
    let mut exclusions = Vec::new();
    for sentence in document
        .text
        .split(['.', '!', '?', ';', '\n'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
    {
        let Some(term) = EXCLUDED_WORK.iter().find(|term| sentence.contains(*term)) else {
            continue;
        };
        let snippet: String = sentence.chars().take(240).collect();
        if NEGATIONS.iter().any(|negation| sentence.contains(negation)) {
            exclusions.push(snippet);
        } else {
            violations.push(format!("{term}: {snippet}"));
        }
    }
    Outcome {
        passed: violations.is_empty(),
        reason: if violations.is_empty() {
            format!(
                "no excluded work is proposed ({} sentence(s) rule excluded work out)",
                exclusions.len()
            )
        } else {
            format!("plan proposes excluded work: {}", violations.join(" / "))
        },
        evidence: if violations.is_empty() {
            exclusions.into_iter().take(3).collect()
        } else {
            violations.into_iter().take(3).collect()
        },
    }
}

fn check(document: &Document, id: &str) -> Option<Outcome> {
    Some(match id {
        "planning.endpoint" => all_groups(
            document,
            &[&["/w/:slug/compare/:from...:to", "compare/:from...:to"]],
        ),
        "planning.exact_version" => all_groups(
            document,
            &[
                &["tag"],
                &["range"],
                &[
                    "reject",
                    "not accept",
                    "refuse",
                    "disallow",
                    "invalid",
                    "not allow",
                ],
            ],
        ),
        "planning.invalid_version" => all_groups(document, &[&["400"], &["invalid_version"]]),
        "planning.missing_worker" => all_groups(document, &[&["404"], &["worker_not_found"]]),
        "planning.missing_version" => all_groups(
            document,
            &[
                &["404"],
                &["version_not_found"],
                &["error.version", "first missing"],
            ],
        ),
        "planning.missing_metadata" => all_groups(
            document,
            &[&["unavailable", "metadata_missing"], &["null"], &["empty"]],
        ),
        "planning.reverse_kinds" => all_groups(document, &[&["revers"], &["added"], &["removed"]]),
        "planning.reverse_values" => all_groups(document, &[&["revers"], &["before"], &["after"]]),
        "planning.reverse_impact" => all_groups(
            document,
            &[
                &["revers"],
                &["impact"],
                &[
                    "recalculat",
                    "recomput",
                    "re-evaluat",
                    "reevaluat",
                    "reappl",
                    "re-appl",
                    "apply the impact",
                    "applies the impact",
                    "applying the impact",
                    "impact policy to the reversed",
                ],
            ],
        ),
        "planning.object_order" => all_groups(
            document,
            &[
                &[
                    "key order",
                    "key ordering",
                    "property order",
                    "order of keys",
                    "order of properties",
                    "key permutation",
                ],
                &[
                    "ignor",
                    "insensitive",
                    "no difference",
                    "not produce",
                    "regardless",
                    "not a change",
                    "not count",
                    "not matter",
                    "canonical",
                ],
            ],
        ),
        "planning.required_order" => all_groups(document, &[&["required"], SET_TERMS]),
        "planning.enum_order" => all_groups(document, &[&["enum"], SET_TERMS]),
        "planning.config_array_order" => all_groups(
            document,
            &[
                &["config"],
                &["array"],
                &["order"],
                &[
                    "preserve",
                    "significant",
                    "ordered",
                    "remain",
                    "keep",
                    "retain",
                    "matter",
                    "detect",
                ],
            ],
        ),
        "planning.backend_location" => any_path(document, BACKEND_PATHS, "backend"),
        "planning.frontend_location" => any_path(document, FRONTEND_PATHS, "frontend"),
        "planning.dependency_order" => dependency_order(document),
        "planning.test_expectations" => test_expectations(document),
        "planning.shared_url" => all_groups(
            document,
            &[
                &["tab=changelog"],
                &["from="],
                &["to="],
                &[
                    "restore",
                    "reopen",
                    "back/forward",
                    "back and forward",
                    "history",
                    "sharea",
                    "share",
                ],
            ],
        ),
        "planning.stale_results" => all_groups(
            document,
            &[
                &[
                    "stale",
                    "previous result",
                    "old result",
                    "outdated",
                    "prior result",
                    "previous pair",
                    "previous comparison",
                    "earlier result",
                    "previous differences",
                ],
                &["fail", "error"],
                &[
                    "clear",
                    "discard",
                    "not show",
                    "never show",
                    "must not",
                    "reset",
                    "hide",
                    "replace",
                    "not leave",
                    "not display",
                    "not present",
                    "not remain",
                ],
            ],
        ),
        "planning.scope" => scope(document),
        _ => return None,
    })
}

/// Observations for every planning metric, in the atomic-award format the
/// Registry scenarios share. A metric without a deterministic check is
/// reported as unavailable rather than silently scored.
pub(super) fn observations(plan: &str, metrics: &[Value]) -> Value {
    let document = Document::new(plan);
    let observations = metrics
        .iter()
        .map(|metric| {
            let id = metric["id"].as_str().unwrap_or_default();
            match check(&document, id) {
                Some(outcome) => json!({
                    "id": id,
                    "status": "measured",
                    "value": u8::from(outcome.passed),
                    "reason": outcome.reason,
                    "evidence": outcome.evidence,
                }),
                None => json!({
                    "id": id,
                    "status": "unavailable",
                    "reason": "no deterministic check is defined for this planning metric",
                }),
            }
        })
        .collect::<Vec<_>>();
    json!({ "validator": VALIDATOR, "observations": observations })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planning_metrics() -> &'static [Value] {
        super::super::registry::metrics(1)
    }

    fn values(plan: &str) -> Vec<(String, u64, String)> {
        observations(plan, planning_metrics())["observations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                assert_eq!(item["status"], "measured", "{item}");
                (
                    item["id"].as_str().unwrap().to_string(),
                    item["value"].as_u64().unwrap(),
                    item["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn every_planning_metric_has_a_deterministic_check() {
        let document = Document::new("");
        for metric in planning_metrics() {
            let id = metric["id"].as_str().unwrap();
            assert!(check(&document, id).is_some(), "{id}");
            assert_eq!(metric["measurement"], "binary", "{id}");
        }
        assert_eq!(planning_metrics().len(), 20);
    }

    #[test]
    fn the_reference_plan_satisfies_every_planning_check() {
        let failed = values(super::super::registry::REFERENCE)
            .into_iter()
            .filter(|(_, value, _)| *value == 0)
            .collect::<Vec<_>>();
        assert!(failed.is_empty(), "{failed:#?}");
    }

    #[test]
    fn an_empty_plan_fails_every_check_except_scope() {
        for (id, value, reason) in values("") {
            if id == "planning.scope" {
                assert_eq!(value, 1, "{reason}");
            } else {
                assert_eq!(value, 0, "{id}: {reason}");
            }
        }
    }

    #[test]
    fn evidence_cites_the_matching_plan_lines() {
        let plan = "# Plan\n\nAdd `GET /w/:slug/compare/:from...:to` to the API.\n";
        let observations = observations(plan, planning_metrics());
        let endpoint = observations["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "planning.endpoint")
            .unwrap();
        assert_eq!(endpoint["value"], 1);
        assert_eq!(
            endpoint["evidence"][0],
            "Add `GET /w/:slug/compare/:from...:to` to the API."
        );
    }

    #[test]
    fn proposed_excluded_work_fails_scope_unless_ruled_out() {
        let proposing = "## Deliverable\n\nAdd a 30-day download chart to the Changelog tab.\n";
        let ruling_out = "## Scope\n\nDo not add a 30-day download chart; it is out of scope.\n";
        let value = |plan: &str| {
            values(plan)
                .into_iter()
                .find(|(id, _, _)| id == "planning.scope")
                .unwrap()
        };
        assert_eq!(value(proposing).1, 0, "{}", value(proposing).2);
        assert_eq!(value(ruling_out).1, 1, "{}", value(ruling_out).2);
    }

    #[test]
    fn sequence_and_test_sections_are_checked_structurally() {
        let ordered = "## Implementation sequence\n\n1. Build the comparison endpoint in the API.\n2. Add the Changelog tab to the page.\n3. Run Vitest and Playwright tests.\n\n## Tests\n\n- 1.0.0 to 1.1.0 returns only additive changes.\n- Invalid SemVer must return 400.\n";
        let reversed = "## Implementation sequence\n\n1. Add the Changelog tab to the page.\n2. Run Playwright tests.\n3. Build the comparison endpoint in the API.\n\n## Tests\n\n- compare versions\n";
        let by_id = |plan: &str, wanted: &str| {
            values(plan)
                .into_iter()
                .find(|(id, _, _)| id == wanted)
                .unwrap()
        };
        assert_eq!(by_id(ordered, "planning.dependency_order").1, 1);
        assert_eq!(by_id(ordered, "planning.test_expectations").1, 1);
        assert_eq!(by_id(reversed, "planning.dependency_order").1, 0);
        assert_eq!(by_id(reversed, "planning.test_expectations").1, 0);
    }
}
