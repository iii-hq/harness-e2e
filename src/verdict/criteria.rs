//! Criterion health within one configuration, the run read first. A
//! configuration is one side's test: its definition, and the stack and model
//! that side ran. Each run is classified by its cause before any criterion is
//! read; the criteria of the counted runs that fail in exactly the same runs
//! are one group, so a run that went wrong as a whole is one case, not one
//! per criterion. Below `MIN_RUNS` counted runs a configuration's rates are
//! insufficient data. Warnings only: nothing here changes a score, the
//! verdict or what is shown.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Key, Run, Side, Sides};

/// Fewer counted runs than this are insufficient data.
pub const MIN_RUNS: usize = 10;
/// Two-sided 95%.
const Z: f64 = 1.959_963_984_540_054;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunClass {
    /// Counted, with points.
    Evaluated,
    /// Counted, with none.
    Zeroed,
    /// The stack lacked what the test relies on.
    PrerequisiteFailed,
    /// Not counted for any other cause.
    Infrastructure,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunClasses {
    pub evaluated: usize,
    pub zeroed: usize,
    pub prerequisite_failed: usize,
    pub infrastructure: usize,
}

impl RunClasses {
    pub(super) fn add(&mut self, class: RunClass) {
        *match class {
            RunClass::Evaluated => &mut self.evaluated,
            RunClass::Zeroed => &mut self.zeroed,
            RunClass::PrerequisiteFailed => &mut self.prerequisite_failed,
            RunClass::Infrastructure => &mut self.infrastructure,
        } += 1;
    }
}

pub(super) fn class_of(run: &Run) -> RunClass {
    match (run.counted, run.cause.as_deref()) {
        (true, _) if run.score.unwrap_or(0.0) > 0.0 => RunClass::Evaluated,
        (true, _) => RunClass::Zeroed,
        (false, Some("test_environment_incomplete")) => RunClass::PrerequisiteFailed,
        (false, _) => RunClass::Infrastructure,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Full in every counted run: a guard, but it shows no improvement.
    Saturated,
    /// Full in none.
    NeverFull,
    Oscillating,
    /// Fewer than `MIN_RUNS` counted runs.
    InsufficientData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Criterion {
    pub id: String,
    /// Runs that awarded all its points.
    pub full: usize,
    /// Counted runs that evaluated it.
    pub n: usize,
    pub rate: f64,
    /// Wilson 95% interval of `rate`.
    pub low: f64,
    pub high: f64,
    pub class: Class,
}

/// Criteria that fall short of full points in exactly the same runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    pub criteria: Vec<String>,
    /// The runs they fall short in.
    pub failing: usize,
    /// The configuration's counted runs.
    pub of: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Configuration {
    pub side: Side,
    pub scenario: String,
    pub behavior: Option<String>,
    pub runs: RunClasses,
    /// At least `MIN_RUNS` counted runs.
    pub sufficient: bool,
    pub criteria: Vec<Criterion>,
    pub groups: Vec<Group>,
}

/// A group of a compared test whose points moved, the groups formed over
/// both sides' counted runs; their deltas add up to the test's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Change {
    pub scenario: String,
    pub behavior: Option<String>,
    pub criteria: Vec<String>,
    /// The runs they fall short in, of `runs`.
    pub failing: Sides<usize>,
    pub runs: Sides<usize>,
    /// Their points per run.
    pub points: Sides<f64>,
    /// B − A, in the test's points.
    pub delta: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub min_runs: usize,
    pub configurations: Vec<Configuration>,
    /// The groups that explain the score changes, the largest first.
    pub changes: Vec<Change>,
}

/// Each criterion, in the order the runs list them, with the runs (indices)
/// that fall short of its full points and the runs that evaluated it.
fn shortfalls(runs: &[&Run]) -> Vec<(String, Vec<usize>, usize)> {
    let mut criteria = Vec::<(String, Vec<usize>, usize)>::new();
    let mut at = BTreeMap::<&str, usize>::new();
    for (index, run) in runs.iter().enumerate() {
        for points in &run.criteria {
            let slot = *at.entry(&points.id).or_insert_with(|| {
                criteria.push((points.id.clone(), Vec::new(), 0));
                criteria.len() - 1
            });
            let (_, failing, n) = &mut criteria[slot];
            *n += 1;
            if points.awarded < points.possible {
                failing.push(index);
            }
        }
    }
    criteria
}

/// Criteria by the runs they fall short in, in first-seen order; criteria
/// that never do are in none.
fn groups(shortfalls: &[(String, Vec<usize>, usize)]) -> Vec<(Vec<usize>, Vec<String>)> {
    let mut groups = Vec::<(Vec<usize>, Vec<String>)>::new();
    for (id, failing, _) in shortfalls
        .iter()
        .filter(|(_, failing, _)| !failing.is_empty())
    {
        match groups.iter_mut().find(|(runs, _)| runs == failing) {
            Some((_, ids)) => ids.push(id.clone()),
            None => groups.push((failing.clone(), vec![id.clone()])),
        }
    }
    groups
}

fn wilson(full: usize, n: usize) -> (f64, f64) {
    let (k, n) = (full as f64, n as f64);
    let p = k / n;
    let denominator = 1.0 + Z * Z / n;
    let center = (p + Z * Z / (2.0 * n)) / denominator;
    let half = Z * (p * (1.0 - p) / n + Z * Z / (4.0 * n * n)).sqrt() / denominator;
    ((center - half).max(0.0), (center + half).min(1.0))
}

fn configuration(side: Side, key: &Key, runs: &[&Run]) -> Configuration {
    let mut classes = RunClasses::default();
    for run in runs {
        classes.add(class_of(run));
    }
    let counted = counted(runs);
    let sufficient = counted.len() >= MIN_RUNS;
    let shortfalls = shortfalls(&counted);
    let criteria = shortfalls
        .iter()
        .map(|(id, failing, n)| {
            let full = n - failing.len();
            let (low, high) = wilson(full, *n);
            Criterion {
                id: id.clone(),
                full,
                n: *n,
                rate: full as f64 / *n as f64,
                low,
                high,
                class: match full {
                    _ if !sufficient => Class::InsufficientData,
                    full if full == *n => Class::Saturated,
                    0 => Class::NeverFull,
                    _ => Class::Oscillating,
                },
            }
        })
        .collect();
    Configuration {
        side,
        scenario: key.0.clone(),
        behavior: key.1.clone(),
        runs: classes,
        sufficient,
        criteria,
        groups: groups(&shortfalls)
            .into_iter()
            .map(|(failing, criteria)| Group {
                criteria,
                failing: failing.len(),
                of: counted.len(),
            })
            .collect(),
    }
}

/// The groups of one compared test, formed over A's counted runs then B's,
/// whose points per run moved.
fn changes(key: &Key, a: &[&Run], b: &[&Run]) -> Vec<Change> {
    let both = a.iter().chain(b).copied().collect::<Vec<_>>();
    let points = |runs: &[&Run], criteria: &[String]| {
        let total = runs
            .iter()
            .flat_map(|run| &run.criteria)
            .filter(|points| criteria.contains(&points.id))
            .map(|points| points.awarded)
            .sum::<f64>();
        total / runs.len() as f64
    };
    groups(&shortfalls(&both))
        .into_iter()
        .map(|(failing, criteria)| {
            let points = Sides {
                a: points(a, &criteria),
                b: points(b, &criteria),
            };
            Change {
                scenario: key.0.clone(),
                behavior: key.1.clone(),
                failing: Sides {
                    a: failing.iter().filter(|index| **index < a.len()).count(),
                    b: failing.iter().filter(|index| **index >= a.len()).count(),
                },
                runs: Sides {
                    a: a.len(),
                    b: b.len(),
                },
                delta: points.b - points.a,
                points,
                criteria,
            }
        })
        .filter(|change| change.delta.abs() > 1e-9)
        .collect()
}

/// The counted runs, ordered by attempt.
fn counted<'a>(runs: &[&'a Run]) -> Vec<&'a Run> {
    let mut counted = runs
        .iter()
        .copied()
        .filter(|run| run.counted)
        .collect::<Vec<_>>();
    counted.sort_by(|left, right| left.attempt_id.cmp(&right.attempt_id));
    counted
}

fn by_test(runs: &[Run]) -> BTreeMap<Key, Vec<&Run>> {
    let mut by = BTreeMap::<Key, Vec<&Run>>::new();
    for run in runs {
        by.entry((run.scenario.clone(), run.behavior.clone()))
            .or_default()
            .push(run);
    }
    by
}

pub(super) fn health(a: &[Run], b: &[Run], shared: &[Key]) -> Health {
    let (tests_a, tests_b) = (by_test(a), by_test(b));
    let configurations = [(Side::A, &tests_a), (Side::B, &tests_b)]
        .into_iter()
        .flat_map(|(side, tests)| {
            tests
                .iter()
                .map(move |(key, runs)| configuration(side, key, runs))
        })
        .collect();
    let mut changes = shared
        .iter()
        .flat_map(|key| changes(key, &counted(&tests_a[key]), &counted(&tests_b[key])))
        .collect::<Vec<_>>();
    changes.sort_by(|left, right| right.delta.abs().total_cmp(&left.delta.abs()));
    Health {
        min_runs: MIN_RUNS,
        configurations,
        changes,
    }
}

pub(super) fn text(health: &Health, label_a: &str, label_b: &str) -> String {
    let mut out = String::new();
    if !health.changes.is_empty() {
        out.push_str("Criteria that moved the score, grouped by the runs they fail in together:\n");
    }
    for change in &health.changes {
        out.push_str(&format!(
            "  {}: {} — short in {} of {} runs on {label_a}, {} of {} on {label_b}: {:+.1} points\n",
            change.scenario,
            change.criteria.join(", "),
            change.failing.a,
            change.runs.a,
            change.failing.b,
            change.runs.b,
            change.delta
        ));
    }
    let insufficient = health
        .configurations
        .iter()
        .filter(|configuration| !configuration.sufficient)
        .count();
    out.push_str(&format!(
        "Criterion health: {} configurations, {insufficient} with fewer than {} counted runs \
         (insufficient data).\n",
        health.configurations.len(),
        health.min_runs
    ));
    for configuration in &health.configurations {
        let label = match configuration.side {
            Side::A => label_a,
            Side::B => label_b,
        };
        for group in &configuration.groups {
            out.push_str(&format!(
                "  {label} {}: {} short together in {} of {} runs\n",
                configuration.scenario,
                group.criteria.join(", "),
                group.failing,
                group.of
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::fixture;
    use super::*;

    fn health_of(a: &str, b: &str) -> Health {
        let (a, b) = (fixture(a), fixture(b));
        super::super::compare(
            &a,
            &b,
            &super::super::Rule {
                permutations: 100,
                ..Default::default()
            },
        )
        .criteria
    }

    fn find<'a>(health: &'a Health, side: Side, scenario: &str) -> &'a Configuration {
        health
            .configurations
            .iter()
            .find(|c| c.side == side && c.scenario == scenario)
            .unwrap()
    }

    #[test]
    fn the_kanban_board_drop_is_one_group_of_four_criteria_in_two_runs() {
        let health = health_of("se-1.8.44.json", "se-1.8.50.json");
        let board = find(&health, Side::B, "kanban_c3_board");
        let mut four = board
            .groups
            .iter()
            .find(|group| group.criteria.len() == 4)
            .unwrap()
            .clone();
        four.criteria.sort();
        assert_eq!(
            four.criteria,
            [
                "criterion_empty",
                "criterion_retry",
                "criterion_settings",
                "criterion_total"
            ]
        );
        assert_eq!((four.failing, four.of), (2, 5));
        // The comparison names the same group as what moved the board's score.
        let change = health
            .changes
            .iter()
            .find(|change| change.scenario == "kanban_c3_board" && change.criteria.len() == 4)
            .unwrap();
        assert_eq!((change.failing.a, change.failing.b), (0, 2));
        assert!(change.delta < 0.0);
        let board_delta = health
            .changes
            .iter()
            .filter(|change| change.scenario == "kanban_c3_board")
            .map(|change| change.delta)
            .sum::<f64>();
        assert!((board_delta + 16.0).abs() < 1e-9, "{board_delta}");
    }

    #[test]
    fn registry_recall_and_precision_are_one_case_never_full() {
        let health = health_of("se-1.8.44.json", "se-1.8.50.json");
        for side in [Side::A, Side::B] {
            let verification = find(&health, side, "registry_verification");
            let group = verification
                .groups
                .iter()
                .find(|group| group.criteria.contains(&"verification.recall".to_owned()))
                .unwrap();
            assert_eq!(
                group.criteria,
                ["verification.recall", "verification.precision"]
            );
            assert_eq!(group.failing, group.of);
        }
    }

    #[test]
    fn few_runs_are_insufficient_data_and_whole_runs_are_classified_first() {
        let se = health_of("se-1.8.44.json", "se-1.8.50.json");
        let regression = health_of("regression-1.8.44.json", "regression-1.8.50.json");
        for health in [&se, &regression] {
            assert!(health.configurations.iter().all(|c| !c.sufficient));
            assert!(health
                .configurations
                .iter()
                .flat_map(|c| &c.criteria)
                .all(|c| c.class == Class::InsufficientData));
        }
        // The port collision is infrastructure, not one zero per criterion.
        let implementation = find(&se, Side::B, "registry_implementation");
        assert_eq!(implementation.runs.infrastructure, 1);
        assert!(implementation.groups.iter().all(|group| group.of == 4));
        let ledger = find(&regression, Side::B, "contention_ledger");
        assert_eq!((ledger.runs.evaluated, ledger.runs.zeroed), (4, 1));
        // Groups that come and go within a configuration, as KAN-497 counts them.
        let oscillating = |health: &Health| {
            health
                .configurations
                .iter()
                .flat_map(|c| &c.groups)
                .filter(|group| group.failing < group.of)
                .count()
        };
        assert_eq!((oscillating(&se), oscillating(&regression)), (15, 14));
    }

    #[test]
    fn rates_carry_a_wilson_interval_and_a_class_from_ten_runs() {
        assert!(wilson(10, 10).1 > 0.999_999);
        let (low, high) = wilson(5, 10);
        assert!((low - 0.2366).abs() < 1e-4 && (high - 0.7634).abs() < 1e-4);
        let run = |attempt: usize, awarded: f64| Run {
            attempt_id: format!("{attempt:02}"),
            scenario: "t".into(),
            counted: true,
            score: Some(awarded),
            criteria: vec![
                super::super::Points {
                    id: "steady".into(),
                    awarded: 5.0,
                    possible: 5.0,
                },
                super::super::Points {
                    id: "flaky".into(),
                    awarded,
                    possible: 5.0,
                },
            ],
            ..Run::default()
        };
        let runs = (0..10)
            .map(|i| run(i, if i % 2 == 0 { 5.0 } else { 0.0 }))
            .collect::<Vec<_>>();
        let configuration = configuration(
            Side::A,
            &("t".into(), None),
            &runs.iter().collect::<Vec<_>>(),
        );
        assert!(configuration.sufficient);
        assert_eq!(
            configuration
                .criteria
                .iter()
                .map(|c| c.class)
                .collect::<Vec<_>>(),
            [Class::Saturated, Class::Oscillating]
        );
    }
}
