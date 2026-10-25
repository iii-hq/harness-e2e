//! Criterion health within one configuration, the run read first. A
//! configuration is one side's runs of a test under one definition, one model
//! and one stack (each worker's resolved version, the engine and the lock); a
//! run whose results record no model or no stack is its own configuration,
//! with a warning. Each run is classified by its cause before any criterion
//! is read; the criteria of the counted runs that fail in exactly the same
//! runs are one group, so a run that went wrong as a whole is one case, not
//! one per criterion. A criterion the evaluator left unevaluated in a run (no
//! points awarded, null or not listed, not zero) is not counted against it
//! there: it is "not evaluated". A criterion evaluated in fewer than
//! `MIN_RUNS` counted runs is insufficient data. The changes of a compared
//! test group its criteria by the runs they fall short in and the runs that
//! did not evaluate them, so their deltas add up to the test's. Warnings
//! only: nothing here changes a score, the verdict or what is shown.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Key, Run, Side, Sides};

/// Fewer counted runs than this are insufficient data: the repetitions of
/// `software-engineering-x9`, the fewest a version comparison runs.
pub const MIN_RUNS: usize = 9;
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
    /// Evaluated in fewer than `MIN_RUNS` counted runs.
    InsufficientData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Criterion {
    pub id: String,
    /// Runs that awarded all its points.
    pub full: usize,
    /// Counted runs that evaluated it.
    pub n: usize,
    /// Counted runs that did not (null, or not listed).
    pub not_evaluated: usize,
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
    /// `provider/model`, none when its results do not record it.
    pub model: Option<String>,
    /// Each worker's resolved version, `iii` the engine and `lock` the stack
    /// lock digest; empty when its results do not record it.
    pub stack: BTreeMap<String, String>,
    pub runs: RunClasses,
    /// At least `MIN_RUNS` counted runs.
    pub sufficient: bool,
    /// The criteria some counted run evaluated.
    pub criteria: Vec<Criterion>,
    /// The criteria no counted run evaluated.
    pub not_evaluated: Vec<String>,
    pub groups: Vec<Group>,
}

/// A group of a compared test whose points moved, the groups formed over
/// both sides' counted runs by the runs they fall short in and the runs that
/// did not evaluate them; their deltas add up to the test's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Change {
    pub scenario: String,
    pub behavior: Option<String>,
    pub criteria: Vec<String>,
    /// The runs they fall short in, of `runs`.
    pub failing: Sides<usize>,
    /// The runs that did not evaluate them (no points), of `runs`.
    pub not_evaluated: Sides<usize>,
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
    /// A side whose runs do not record their model or stack.
    pub warnings: Vec<String>,
}

/// One criterion over a configuration's runs.
struct Shortfall {
    id: String,
    /// The runs (indices) that evaluated it and fell short of its points.
    failing: Vec<usize>,
    /// The runs that evaluated it.
    n: usize,
    /// The runs (indices) that did not: no points, or not listed.
    unevaluated: Vec<usize>,
}

/// Each criterion, in the order the runs list them.
fn shortfalls(runs: &[&Run]) -> Vec<Shortfall> {
    let mut known = BTreeSet::new();
    let ids = runs
        .iter()
        .flat_map(|run| &run.criteria)
        .map(|points| points.id.as_str())
        .filter(|id| known.insert(*id))
        .collect::<Vec<_>>();
    ids.into_iter()
        .map(|id| {
            let mut criterion = Shortfall {
                id: id.to_owned(),
                failing: Vec::new(),
                n: 0,
                unevaluated: Vec::new(),
            };
            for (index, run) in runs.iter().enumerate() {
                let points = run.criteria.iter().find(|points| points.id == id);
                match points.and_then(|points| Some((points.awarded?, points.possible))) {
                    None => criterion.unevaluated.push(index),
                    Some((awarded, possible)) => {
                        criterion.n += 1;
                        if awarded < possible {
                            criterion.failing.push(index);
                        }
                    }
                }
            }
            criterion
        })
        .collect()
}

/// A group: the runs its criteria fall short in, those that did not evaluate
/// them (kept apart only when `unevaluated`), and the criteria.
type Grouped = (Vec<usize>, Vec<usize>, Vec<String>);

/// Criteria by the runs they fall short in, and with `unevaluated` by the
/// runs that did not evaluate them too, in first-seen order; criteria that
/// never fall short (nor, with it, go unevaluated) are in none.
fn groups(shortfalls: &[Shortfall], unevaluated: bool) -> Vec<Grouped> {
    let mut groups = Vec::<Grouped>::new();
    for criterion in shortfalls {
        let missing = if unevaluated {
            criterion.unevaluated.clone()
        } else {
            Vec::new()
        };
        if criterion.failing.is_empty() && missing.is_empty() {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(failing, without, _)| *failing == criterion.failing && *without == missing)
        {
            Some((_, _, ids)) => ids.push(criterion.id.clone()),
            None => groups.push((
                criterion.failing.clone(),
                missing,
                vec![criterion.id.clone()],
            )),
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
    let identity = runs.first();
    let mut classes = RunClasses::default();
    for run in runs {
        classes.add(class_of(run));
    }
    let counted = counted(runs);
    let sufficient = counted.len() >= MIN_RUNS;
    let shortfalls = shortfalls(&counted);
    let criteria = shortfalls
        .iter()
        .filter(|criterion| criterion.n > 0)
        .map(|criterion| {
            let n = criterion.n;
            let full = n - criterion.failing.len();
            let (low, high) = wilson(full, n);
            Criterion {
                id: criterion.id.clone(),
                full,
                n,
                not_evaluated: criterion.unevaluated.len(),
                rate: full as f64 / n as f64,
                low,
                high,
                class: match full {
                    _ if n < MIN_RUNS => Class::InsufficientData,
                    full if full == n => Class::Saturated,
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
        model: identity.and_then(|run| run.model.clone()),
        stack: identity.map(|run| run.stack.clone()).unwrap_or_default(),
        runs: classes,
        sufficient,
        criteria,
        not_evaluated: shortfalls
            .iter()
            .filter(|criterion| criterion.n == 0)
            .map(|criterion| criterion.id.clone())
            .collect(),
        groups: groups(&shortfalls, false)
            .into_iter()
            .map(|(failing, _, criteria)| Group {
                criteria,
                failing: failing.len(),
                of: counted.len(),
            })
            .collect(),
    }
}

/// The groups of one compared test, formed over A's counted runs with a
/// score then B's (the runs its Δ reads), whose points per run moved.
fn changes(key: &Key, a: &[&Run], b: &[&Run]) -> Vec<Change> {
    fn scored<'a>(runs: &[&'a Run]) -> Vec<&'a Run> {
        runs.iter()
            .copied()
            .filter(|run| run.score.is_some())
            .collect()
    }
    let (a, b) = (scored(a), scored(b));
    let (a, b) = (a.as_slice(), b.as_slice());
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let both = a.iter().chain(b).copied().collect::<Vec<_>>();
    let points = |runs: &[&Run], criteria: &[String]| {
        let total = runs
            .iter()
            .flat_map(|run| &run.criteria)
            .filter(|points| criteria.contains(&points.id))
            .filter_map(|points| points.awarded)
            .sum::<f64>();
        total / runs.len() as f64
    };
    let split = |runs: &[usize]| Sides {
        a: runs.iter().filter(|index| **index < a.len()).count(),
        b: runs.iter().filter(|index| **index >= a.len()).count(),
    };
    groups(&shortfalls(&both), true)
        .into_iter()
        .map(|(failing, unevaluated, criteria)| {
            let points = Sides {
                a: points(a, &criteria),
                b: points(b, &criteria),
            };
            Change {
                scenario: key.0.clone(),
                behavior: key.1.clone(),
                failing: split(&failing),
                not_evaluated: split(&unevaluated),
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

/// A test's runs by the model and stack that ran them.
type Identity<'a> = (Option<&'a str>, &'a BTreeMap<String, String>);

fn by_configuration(runs: &[Run]) -> BTreeMap<(Key, Identity<'_>), Vec<&Run>> {
    let mut by = BTreeMap::<(Key, Identity), Vec<&Run>>::new();
    for run in runs {
        let key = (run.scenario.clone(), run.behavior.clone());
        by.entry((key, (run.model.as_deref(), &run.stack)))
            .or_default()
            .push(run);
    }
    by
}

/// A side whose runs do not record what ran them.
fn identity_warnings(a: &[Run], b: &[Run]) -> Vec<String> {
    let mut warnings = Vec::new();
    for (side, runs) in [("A", a), ("B", b)] {
        for (what, lacks) in [
            (
                "model",
                runs.iter().filter(|run| run.model.is_none()).count(),
            ),
            (
                "stack",
                runs.iter().filter(|run| run.stack.is_empty()).count(),
            ),
        ] {
            if lacks > 0 {
                warnings.push(format!(
                    "{side}: {lacks} of {} runs have no {what} in their results: criterion health \
                     cannot tell their configuration apart, nor the comparison whether the side \
                     mixes them.",
                    runs.len()
                ));
            }
        }
    }
    warnings
}

pub(super) fn health(a: &[Run], b: &[Run], shared: &[Key]) -> Health {
    let (tests_a, tests_b) = (by_test(a), by_test(b));
    let configurations = [
        (Side::A, by_configuration(a)),
        (Side::B, by_configuration(b)),
    ]
    .into_iter()
    .flat_map(|(side, configurations)| {
        configurations
            .into_iter()
            .map(move |((key, _), runs)| configuration(side, &key, &runs))
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
        warnings: identity_warnings(a, b),
    }
}

pub(super) fn text(health: &Health, label_a: &str, label_b: &str) -> String {
    let mut out = String::new();
    if !health.changes.is_empty() {
        out.push_str("Criteria that moved the score, grouped by the runs they fail in together:\n");
    }
    for change in &health.changes {
        let unevaluated = if change.not_evaluated.a + change.not_evaluated.b > 0 {
            format!(
                ", not evaluated in {} on {label_a} and {} on {label_b}",
                change.not_evaluated.a, change.not_evaluated.b
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {}: {} — short in {} of {} runs on {label_a}, {} of {} on {label_b}{unevaluated}: \
             {:+.1} points\n",
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
        if !configuration.not_evaluated.is_empty() {
            out.push_str(&format!(
                "  {label} {}: {} not evaluated in any counted run\n",
                configuration.scenario,
                configuration.not_evaluated.join(", ")
            ));
        }
    }
    for warning in &health.warnings {
        out.push_str(&format!("Warning: {warning}\n"));
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
    fn registry_recall_falls_short_in_every_run_and_precision_is_never_evaluated() {
        let health = health_of("se-1.8.44.json", "se-1.8.50.json");
        for side in [Side::A, Side::B] {
            let verification = find(&health, side, "registry_verification");
            let group = verification
                .groups
                .iter()
                .find(|group| group.criteria.contains(&"verification.recall".to_owned()))
                .unwrap();
            // Precision has no points in any run: unevaluated, not failed.
            assert_eq!(group.criteria, ["verification.recall"]);
            assert_eq!(group.failing, group.of);
            assert_eq!(verification.not_evaluated, ["verification.precision"]);
            assert!(verification
                .criteria
                .iter()
                .all(|criterion| criterion.id != "verification.precision"));
        }
        let text = text(&health, "A", "B");
        assert!(text.contains("registry_verification: verification.precision not evaluated"));
    }

    #[test]
    fn a_criterion_left_unevaluated_in_a_run_is_not_counted_there() {
        let run = |attempt: &str, awarded: Option<f64>| Run {
            attempt_id: attempt.into(),
            scenario: "t".into(),
            counted: true,
            score: Some(awarded.unwrap_or(0.0)),
            criteria: vec![super::super::Points {
                id: "c".into(),
                awarded,
                possible: 5.0,
            }],
            ..Run::default()
        };
        let runs = [run("1", Some(5.0)), run("2", None), run("3", Some(0.0))];
        let configuration = configuration(
            Side::A,
            &("t".into(), None),
            &runs.iter().collect::<Vec<_>>(),
        );
        let criterion = &configuration.criteria[0];
        assert_eq!(
            (criterion.full, criterion.n, criterion.not_evaluated),
            (1, 2, 1)
        );
        assert_eq!(configuration.groups[0].failing, 1);
    }

    #[test]
    fn group_deltas_read_the_scored_runs_the_test_delta_reads() {
        let run = |attempt: &str, score: Option<f64>| Run {
            attempt_id: attempt.into(),
            scenario: "t".into(),
            behavior: Some("s".into()),
            cause: Some("evaluated".into()),
            counted: true,
            score,
            cost_usd: Some(1.0),
            criteria: score
                .map(|score| super::super::Points {
                    id: "c".into(),
                    awarded: Some(score),
                    possible: 100.0,
                })
                .into_iter()
                .collect(),
            ..Run::default()
        };
        let a = [run("a1", Some(100.0)), run("a2", Some(100.0))];
        // A counted run without a score: out of the test's Δ, and the group's.
        let b = [
            run("b1", Some(60.0)),
            run("b2", Some(80.0)),
            run("b3", None),
        ];
        let comparison = super::super::compare(&a, &b, &super::super::Rule::default());
        let groups = comparison
            .criteria
            .changes
            .iter()
            .map(|change| change.delta)
            .sum::<f64>();
        assert_eq!(comparison.tests[0].delta, -30.0);
        assert!(
            (groups - comparison.tests[0].delta).abs() < 1e-9,
            "{groups}"
        );
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
        // The run the subject's own stack broke is a counted zero.
        let implementation = find(&se, Side::B, "registry_implementation");
        assert_eq!(
            (
                implementation.runs.zeroed,
                implementation.runs.infrastructure
            ),
            (1, 0)
        );
        assert!(implementation.groups.iter().all(|group| group.of == 5));
        let ledger = find(&regression, Side::B, "contention_ledger");
        assert_eq!((ledger.runs.evaluated, ledger.runs.zeroed), (4, 1));
        // Groups that come and go within a configuration, as KAN-497 counts
        // them (15 in software engineering with the zero set aside; counted,
        // what fails only in it is one more).
        let oscillating = |health: &Health| {
            health
                .configurations
                .iter()
                .flat_map(|c| &c.groups)
                .filter(|group| group.failing < group.of)
                .count()
        };
        assert_eq!((oscillating(&se), oscillating(&regression)), (16, 14));
    }

    /// A counted run of `t` whose criteria award these points out of 10.
    fn scored(attempt: &str, awards: &[(&str, Option<f64>)]) -> Run {
        Run {
            attempt_id: attempt.into(),
            scenario: "t".into(),
            behavior: Some("sha256:t".into()),
            cause: Some("evaluated".into()),
            counted: true,
            score: Some(awards.iter().filter_map(|(_, awarded)| *awarded).sum()),
            cost_usd: Some(1.0),
            model: Some("deepseek/flash".into()),
            stack: [("harness".to_owned(), "1.8.50".to_owned())].into(),
            criteria: awards
                .iter()
                .map(|(id, awarded)| super::super::Points {
                    id: (*id).into(),
                    awarded: *awarded,
                    possible: 10.0,
                })
                .collect(),
            ..Run::default()
        }
    }

    #[test]
    fn a_configuration_is_one_definition_model_and_stack_and_a_class_needs_nine_runs_of_its_own() {
        let rule = super::super::Rule {
            permutations: 100,
            ..Default::default()
        };
        // Nine counted runs; `rare` is evaluated in two of them only.
        let runs = (0..9)
            .map(|i| {
                let rare = (i < 2).then_some(10.0);
                scored(&format!("a{i}"), &[("solid", Some(10.0)), ("rare", rare)])
            })
            .collect::<Vec<_>>();
        let health = super::super::compare(&runs, &runs, &rule).criteria;
        let configuration = &health.configurations[0];
        assert!(configuration.sufficient);
        assert_eq!(
            configuration
                .criteria
                .iter()
                .map(|c| (c.id.as_str(), c.n, c.not_evaluated, c.class))
                .collect::<Vec<_>>(),
            [
                ("solid", 9, 0, Class::Saturated),
                ("rare", 2, 7, Class::InsufficientData)
            ]
        );
        assert_eq!(
            (
                configuration.model.as_deref(),
                configuration.stack["harness"].as_str()
            ),
            (Some("deepseek/flash"), "1.8.50")
        );
        assert!(health.warnings.is_empty());
        // Another model, or another stack, on the same side is another
        // configuration: ten runs of two models are not ten runs of one.
        let mut mixed = runs[..5].to_vec();
        for (i, run) in runs[5..].iter().enumerate() {
            let mut run = run.clone();
            run.attempt_id = format!("m{i}");
            if i % 2 == 0 {
                run.model = Some("openai/gpt-5".into());
            } else {
                run.stack.insert("harness".into(), "1.8.51".into());
            }
            mixed.push(run);
        }
        let health = super::super::compare(&mixed, &runs, &rule).criteria;
        let on_a = health
            .configurations
            .iter()
            .filter(|c| c.side == Side::A)
            .map(|c| c.runs.evaluated)
            .collect::<Vec<_>>();
        assert_eq!(on_a, [5, 2, 2]);
        // Runs that record no model or no stack: a warning, per side.
        let mut unknown = runs.clone();
        for run in &mut unknown[..3] {
            run.model = None;
        }
        unknown[3].stack.clear();
        assert_eq!(
            super::super::compare(&runs, &unknown, &rule)
                .criteria
                .warnings,
            [
                "B: 3 of 9 runs have no model in their results: criterion health cannot tell \
                 their configuration apart, nor the comparison whether the side mixes them.",
                "B: 1 of 9 runs have no stack in their results: criterion health cannot tell \
                 their configuration apart, nor the comparison whether the side mixes them."
            ]
        );
    }

    #[test]
    fn points_a_criterion_was_not_evaluated_for_are_one_change_and_the_changes_add_up() {
        // `extra` earns its 10 points whenever it is evaluated, but B leaves
        // it unevaluated (null, or not listed) in half the runs.
        let a = (0..4)
            .map(|i| {
                scored(
                    &format!("a{i}"),
                    &[("core", Some(10.0)), ("extra", Some(10.0))],
                )
            })
            .collect::<Vec<_>>();
        let b = (0..4)
            .map(|i| {
                let core = Some(if i == 0 { 0.0 } else { 10.0 });
                match i % 2 {
                    0 => scored(&format!("b{i}"), &[("core", core), ("extra", Some(10.0))]),
                    _ if i == 1 => scored(&format!("b{i}"), &[("core", core), ("extra", None)]),
                    _ => scored(&format!("b{i}"), &[("core", core)]),
                }
            })
            .collect::<Vec<_>>();
        let comparison = super::super::compare(&a, &b, &super::super::Rule::default());
        let changes = &comparison.criteria.changes;
        let extra = changes
            .iter()
            .find(|change| change.criteria == ["extra"])
            .unwrap();
        assert_eq!(
            (
                extra.failing.b,
                extra.not_evaluated.a,
                extra.not_evaluated.b,
                extra.delta
            ),
            (0, 0, 2, -5.0)
        );
        let total = changes.iter().map(|change| change.delta).sum::<f64>();
        assert!((total - comparison.tests[0].delta).abs() < 1e-9, "{total}");
        assert!(text(&comparison.criteria, "A", "B")
            .contains("t: extra — short in 0 of 4 runs on A, 0 of 4 on B, not evaluated in 0 on A and 2 on B: -5.0 points"));
    }

    #[test]
    fn rates_carry_a_wilson_interval_and_a_class_from_nine_runs() {
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
                    awarded: Some(5.0),
                    possible: 5.0,
                },
                super::super::Points {
                    id: "flaky".into(),
                    awarded: Some(awarded),
                    possible: 5.0,
                },
            ],
            ..Run::default()
        };
        let runs = (0..9)
            .map(|i| run(i, if i % 2 == 0 { 5.0 } else { 0.0 }))
            .collect::<Vec<_>>();
        let of = |runs: &[Run]| {
            configuration(
                Side::A,
                &("t".into(), None),
                &runs.iter().collect::<Vec<_>>(),
            )
        };
        assert!(!of(&runs[..8]).sufficient);
        let configuration = of(&runs);
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
