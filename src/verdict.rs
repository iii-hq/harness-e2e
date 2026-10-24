//! One calibrated verdict on two arms: B against A is better, no different,
//! worse, or the runs cannot tell yet. `harness-e2e compare`, the Console's
//! comparison and the Trends overview read it from here; Release Control
//! reproduces it from this description and `tests/fixtures/verdict/`.
//!
//! Inputs. Counted runs only: those whose `cause` counts (a report older
//! than causes falls back to `technical == "valid"`, with a warning). A test
//! is a (`scenario_id`, `behavior_sha256`) both arms ran; any other is
//! dropped and listed. Within a test the runs are ordered by `attempt_id`
//! (byte order); tests by `scenario_id`, then `behavior_sha256` (none
//! first). A figure reads the tests with a value on both sides, and there is
//! a verdict only when some test has two runs or more on each.
//!
//! Figures. Quality is the official run score: the suite figure is the mean
//! over tests of each test's mean, Δ = (Σ mean_B − Σ mean_A) / K. Cost is
//! `cost.subject_usd`: the suite figure is the sum over tests of each test's
//! mean, the change Σ mean_B / Σ mean_A − 1. Every sum adds left to right
//! from 0.0; a mean divides that sum by the count. Completion is shown, and
//! time, processed prompt, context per turn and its peak are supporting
//! figures: none of them decides.
//!
//! Permutation test (labels exchanged within each test). Each test below
//! draws from its own SplitMix64 seeded with `seed`. In each of
//! `permutations` rounds, for each test in order, the pool is A's values then
//! B's transformed values, in run order; Fisher–Yates runs i from the last
//! index down to 1 and swaps i with `next() % (i + 1)`; the first |A| values
//! are A's. A round whose statistic t ≤ observed + 1e-12 counts low, one
//! with t ≥ observed − 1e-12 counts high; p = (count + 1) / (rounds + 1).
//!
//! Tests, each one-sided at α = 0.025. Quality as is: `p_fell` is low.
//! Quality with B + margin: `p_non_inferior` is high, `p_inferior` low.
//! Cost as is: `p_cheaper` low, `p_more_expensive` high. Cost with
//! B / (1 − equivalence): `p_within_low` is high; with B / (1 + equivalence):
//! `p_within_high` is low.
//!
//! States. Quality: non-inferior when `p_non_inferior` < α, else inferior
//! when `p_inferior` < α, else fell when `p_fell` < α, else inconclusive.
//! Cost: equivalent when both `p_within_*` < α, else cheaper, else more
//! expensive, else inconclusive. Verdict: better = non-inferior and cheaper;
//! no difference = non-inferior and equivalent; worse = inferior, or
//! non-inferior and more expensive; anything else cannot tell, qualified
//! `quality_fell` when quality fell by an amount the runs cannot bound.
//!
//! Repetitions. When the runs cannot tell, the repetitions per arm the margin
//! needs at the observed Δ: n = Σ_k 2σ_k² / (K² ((M + Δ) / 2.8)²), σ_k² the
//! mean of the arms' sample variances of test k (the pooled one where a test
//! has none); no number when M + Δ ≤ 0, and "more than 30" above 30.
mod criteria;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use criteria::Health;

use crate::report::RunCause;

/// Above it the repetitions needed read "more than".
pub const REPETITIONS_CAP: usize = 30;
const TIE: f64 = 1e-12;
/// z for a one-sided α of 0.025 plus z for a power of 80%.
const POWER_Z: f64 = 2.8;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    /// Points of quality B may lose and still be non-inferior.
    pub margin: f64,
    /// Relative cost change that counts as no change.
    pub equivalence: f64,
    /// One-sided, for each test.
    pub alpha: f64,
    pub permutations: usize,
    pub seed: u64,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            margin: 5.0,
            equivalence: 0.05,
            alpha: 0.025,
            permutations: 10_000,
            seed: 0,
        }
    }
}

/// One run as the verdict reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub attempt_id: String,
    pub scenario: String,
    pub behavior: Option<String>,
    /// None in a report older than causes.
    pub cause: Option<String>,
    pub counted: bool,
    pub score: Option<f64>,
    pub completed: bool,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: Option<f64>,
    pub processed_prompt_tokens: Option<f64>,
    pub turns: Option<f64>,
    pub peak_context_tokens: Option<f64>,
    /// False for a provider whose prompt does not match the Harness's count.
    pub cache_comparable: Option<bool>,
    pub criteria: Vec<Points>,
}

/// What one criterion awarded in a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Points {
    pub id: String,
    pub awarded: f64,
    pub possible: f64,
}

/// The runs of results documents (`results.json`, or a report the Console
/// projects from one): every `scenarios[].runs[]`, a run seen twice counted
/// once (a root bundle carries its groups' results too).
pub fn runs_of<'a>(documents: impl IntoIterator<Item = &'a Value>) -> Vec<Run> {
    let mut seen = BTreeSet::new();
    let mut runs = Vec::new();
    for document in documents {
        for scenario in document["scenarios"].as_array().into_iter().flatten() {
            let Some(id) = scenario["scenario_id"].as_str() else {
                continue;
            };
            for run in scenario["runs"].as_array().into_iter().flatten() {
                let attempt = run["attempt_id"]
                    .as_str()
                    .or(run["run_id"].as_str())
                    .unwrap_or_default();
                if !attempt.is_empty() && !seen.insert(attempt.to_owned()) {
                    continue;
                }
                runs.push(run_of(
                    id,
                    scenario["behavior_sha256"].as_str(),
                    attempt,
                    run,
                ));
            }
        }
    }
    runs
}

/// One arm's runs: a results.json, or every results.json below a directory.
/// A file that does not parse is skipped, as the root bundle's copies are
/// counted once.
pub fn read_arm(path: &std::path::Path) -> anyhow::Result<Vec<Run>> {
    fn files(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        if path.is_file() {
            out.push(path.to_owned());
            return Ok(());
        }
        let mut entries = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                files(&entry, out)?;
            } else if entry.file_name().is_some_and(|name| name == "results.json") {
                out.push(entry);
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    files(path, &mut paths).with_context(|| format!("read {}", path.display()))?;
    let documents = paths
        .iter()
        .filter_map(|path| serde_json::from_slice::<Value>(&std::fs::read(path).ok()?).ok())
        .collect::<Vec<_>>();
    let runs = runs_of(&documents);
    if runs.is_empty() {
        anyhow::bail!("no run in any results.json at {}", path.display());
    }
    Ok(runs)
}

fn run_of(scenario: &str, behavior: Option<&str>, attempt: &str, run: &Value) -> Run {
    let cause = run["cause"].as_str().map(str::to_owned);
    let counted = cause
        .as_deref()
        .and_then(|cause| serde_json::from_value::<RunCause>(Value::from(cause)).ok())
        .map_or(run["technical"] == "valid", RunCause::counted);
    let criteria = run["criteria"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|criterion| {
            Some(Points {
                id: criterion["id"].as_str()?.to_owned(),
                awarded: criterion["awarded"].as_f64().unwrap_or(0.0),
                possible: criterion["possible"].as_f64()?,
            })
        })
        .collect();
    Run {
        attempt_id: attempt.to_owned(),
        scenario: scenario.to_owned(),
        behavior: behavior.map(str::to_owned),
        cause,
        counted,
        score: run["score"].as_f64(),
        completed: run["completion"] == "completed",
        cost_usd: run["cost"]["subject_usd"].as_f64(),
        wall_time_ms: run["wall_time_ms"].as_f64(),
        processed_prompt_tokens: run["efficiency"]["processed_prompt_tokens"].as_f64(),
        turns: run["metrics"]["totals"]["turns"].as_f64(),
        peak_context_tokens: run["efficiency"]["peak_context_tokens"].as_f64(),
        cache_comparable: run["efficiency"]["cache_comparable"].as_bool(),
        criteria,
    }
}

/// A side of the comparison.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    A,
    B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Sides<T> {
    pub a: T,
    pub b: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Better,
    NoDifference,
    Worse,
    CannotTell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Qualifier {
    /// Quality fell, by an amount the runs cannot bound.
    QualityFell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualityState {
    /// B loses less than the margin.
    NonInferior,
    /// B loses more than the margin.
    Inferior,
    /// B loses something; how much, the runs cannot bound.
    Fell,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostState {
    /// Within ±`equivalence`.
    Equivalent,
    Cheaper,
    MoreExpensive,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quality {
    pub state: QualityState,
    /// Mean over the tests of each test's mean score.
    pub a: f64,
    pub b: f64,
    /// B − A, in points.
    pub delta: f64,
    pub tests: usize,
    pub p_non_inferior: f64,
    pub p_inferior: f64,
    pub p_fell: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cost {
    pub state: CostState,
    /// Sum over the tests of each test's mean subject cost, in US$.
    pub a: f64,
    pub b: f64,
    /// B / A − 1.
    pub relative: f64,
    pub tests: usize,
    pub p_cheaper: f64,
    pub p_more_expensive: f64,
    /// Against B costing `equivalence` less or more; both under α is
    /// equivalent.
    pub p_within_low: f64,
    pub p_within_high: f64,
}

/// A figure shown beside the verdict, never deciding it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Figure {
    /// `completion`, `wall_time_s`, `processed_prompt_tokens`,
    /// `context_per_turn` or `peak_context_tokens`.
    pub id: String,
    pub a: Option<f64>,
    pub b: Option<f64>,
    pub delta: Option<f64>,
    pub relative: Option<f64>,
    /// False when a provider's prompt does not reconcile on a side.
    pub comparable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Repetitions {
    /// The fewest counted runs a compared test has on each side.
    pub have: Sides<usize>,
    /// Pooled within-test standard deviation of the score, per run.
    pub sigma: Option<f64>,
    /// Per arm, for the margin at the observed Δ; only when the runs cannot
    /// tell and a number exists.
    pub needed: Option<usize>,
    /// `needed` less the fewest of `have`.
    pub missing: Option<usize>,
    /// Set to the cap when more than it would be needed.
    pub more_than: Option<usize>,
    /// Why there is no number, when there is none.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestDelta {
    pub scenario: String,
    pub behavior: Option<String>,
    pub runs: Sides<usize>,
    pub a: f64,
    pub b: f64,
    pub delta: f64,
    /// This test's own permutation test of a drop.
    pub p_fell: f64,
    /// It fell more than the margin: a warning, never deciding.
    pub warning: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dropped {
    pub scenario: String,
    /// `definition_changed`, `only_in_a`, `only_in_b`, `no_counted_run_in_a`
    /// or `no_counted_run_in_b`.
    pub reason: String,
}

/// What became of one side's runs, first by cause, then as the criteria
/// health reads them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunCounts {
    pub counted: usize,
    pub excluded: usize,
    /// `earlier_rule` for a run of a report older than causes.
    pub causes: BTreeMap<String, usize>,
    pub classes: criteria::RunClasses,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Comparison {
    /// Null when there is no calibrated verdict; `unavailable` says why.
    pub verdict: Option<Verdict>,
    pub qualifier: Option<Qualifier>,
    pub unavailable: Option<String>,
    /// The rule's reading, quality first.
    pub reasons: Vec<String>,
    pub rule: Rule,
    pub quality: Option<Quality>,
    pub cost: Option<Cost>,
    /// Completion first, then the supporting figures.
    pub figures: Vec<Figure>,
    pub repetitions: Repetitions,
    pub tests: Vec<TestDelta>,
    pub dropped: Vec<Dropped>,
    pub runs: Sides<RunCounts>,
    pub warnings: Vec<String>,
    pub criteria: Health,
}

/// The verdict alone, for a reader that has no run causes or criteria (the
/// Trends overview).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub verdict: Option<Verdict>,
    pub qualifier: Option<Qualifier>,
    pub unavailable: Option<String>,
    pub reasons: Vec<String>,
    pub rule: Rule,
    pub quality: Option<Quality>,
    pub cost: Option<Cost>,
    pub repetitions: Repetitions,
}

impl Comparison {
    pub fn summary(self) -> Summary {
        Summary {
            verdict: self.verdict,
            qualifier: self.qualifier,
            unavailable: self.unavailable,
            reasons: self.reasons,
            rule: self.rule,
            quality: self.quality,
            cost: self.cost,
            repetitions: self.repetitions,
        }
    }
}

type Key = (String, Option<String>);

/// A test's values on each side, in run order.
struct Cell<'a> {
    key: &'a Key,
    a: Vec<f64>,
    b: Vec<f64>,
}

#[derive(Clone, Copy)]
enum Statistic {
    /// (Σ mean_B − Σ mean_A) / K.
    MeanDifference,
    /// Σ mean_B / Σ mean_A − 1.
    RelativeSum,
}

/// The observed statistic and its one-sided p-values.
struct Tails {
    observed: f64,
    low: f64,
    high: f64,
}

/// SplitMix64 (Steele, Lea and Flood): the whole state is the seed.
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Fisher–Yates, from the last index down.
    pub fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            values.swap(i, j);
        }
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().fold(0.0, |total, value| total + value) / values.len() as f64
}

/// The statistic over pools whose first `split` values are A's.
fn statistic(how: Statistic, pools: &[(usize, Vec<f64>)]) -> f64 {
    let (mut a, mut b) = (0.0, 0.0);
    for (split, pool) in pools {
        a += mean(&pool[..*split]);
        b += mean(&pool[*split..]);
    }
    match how {
        Statistic::MeanDifference => (b - a) / pools.len() as f64,
        Statistic::RelativeSum => b / a - 1.0,
    }
}

fn permutation(
    cells: &[Cell],
    how: Statistic,
    transform: impl Fn(f64) -> f64,
    rule: &Rule,
) -> Tails {
    let pools = cells
        .iter()
        .map(|cell| {
            let pool = cell
                .a
                .iter()
                .copied()
                .chain(cell.b.iter().map(|v| transform(*v)));
            (cell.a.len(), pool.collect::<Vec<_>>())
        })
        .collect::<Vec<_>>();
    let observed = statistic(how, &pools);
    let mut rng = SplitMix64(rule.seed);
    let mut work = pools.clone();
    let (mut low, mut high) = (0usize, 0usize);
    for _ in 0..rule.permutations {
        for ((_, pool), (_, original)) in work.iter_mut().zip(&pools) {
            pool.copy_from_slice(original);
            rng.shuffle(pool);
        }
        let t = statistic(how, &work);
        low += usize::from(t <= observed + TIE);
        high += usize::from(t >= observed - TIE);
    }
    let p = |count: usize| (count + 1) as f64 / (rule.permutations + 1) as f64;
    Tails {
        observed,
        low: p(low),
        high: p(high),
    }
}

/// Each side's counted runs by test, ordered by attempt.
fn cells_of(runs: &[Run]) -> BTreeMap<Key, Vec<&Run>> {
    let mut cells = BTreeMap::<Key, Vec<&Run>>::new();
    for run in runs.iter().filter(|run| run.counted) {
        cells
            .entry((run.scenario.clone(), run.behavior.clone()))
            .or_default()
            .push(run);
    }
    for runs in cells.values_mut() {
        runs.sort_by(|left, right| left.attempt_id.cmp(&right.attempt_id));
    }
    cells
}

/// The tests both sides have a value of `of` for.
fn values<'a>(
    shared: &'a [Key],
    a: &BTreeMap<Key, Vec<&Run>>,
    b: &BTreeMap<Key, Vec<&Run>>,
    of: impl Fn(&Run) -> Option<f64>,
) -> Vec<Cell<'a>> {
    shared
        .iter()
        .map(|key| Cell {
            key,
            a: a[key].iter().filter_map(|run| of(run)).collect(),
            b: b[key].iter().filter_map(|run| of(run)).collect(),
        })
        .filter(|cell| !cell.a.is_empty() && !cell.b.is_empty())
        .collect()
}

fn variance(values: &[f64]) -> Option<f64> {
    (values.len() > 1).then(|| {
        let m = mean(values);
        values.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (values.len() - 1) as f64
    })
}

/// B against A over their counted runs.
pub fn compare(a: &[Run], b: &[Run], rule: &Rule) -> Comparison {
    let (cells_a, cells_b) = (cells_of(a), cells_of(b));
    let shared = cells_a
        .keys()
        .filter(|key| cells_b.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let scores = values(&shared, &cells_a, &cells_b, |run| run.score);
    let costs = values(&shared, &cells_a, &cells_b, |run| run.cost_usd);
    let unavailable = if scores.is_empty() {
        Some("No test ran on both sides under the same definition.".to_owned())
    } else if !scores
        .iter()
        .any(|cell| cell.a.len() > 1 && cell.b.len() > 1)
    {
        Some("One run per test on a side: no noise to read the difference against.".to_owned())
    } else {
        None
    };

    let quality = (unavailable.is_none()).then(|| {
        let fell = permutation(&scores, Statistic::MeanDifference, |v| v, rule);
        let shifted = permutation(
            &scores,
            Statistic::MeanDifference,
            |v| v + rule.margin,
            rule,
        );
        let k = scores.len() as f64;
        let (p_non_inferior, p_inferior, p_fell) = (shifted.high, shifted.low, fell.low);
        Quality {
            state: if p_non_inferior < rule.alpha {
                QualityState::NonInferior
            } else if p_inferior < rule.alpha {
                QualityState::Inferior
            } else if p_fell < rule.alpha {
                QualityState::Fell
            } else {
                QualityState::Inconclusive
            },
            a: scores.iter().map(|cell| mean(&cell.a)).sum::<f64>() / k,
            b: scores.iter().map(|cell| mean(&cell.b)).sum::<f64>() / k,
            delta: fell.observed,
            tests: scores.len(),
            p_non_inferior,
            p_inferior,
            p_fell,
        }
    });
    let cost_a = costs.iter().map(|cell| mean(&cell.a)).sum::<f64>();
    let cost = (unavailable.is_none() && cost_a > 0.0).then(|| {
        let plain = permutation(&costs, Statistic::RelativeSum, |v| v, rule);
        let low = permutation(
            &costs,
            Statistic::RelativeSum,
            |v| v / (1.0 - rule.equivalence),
            rule,
        );
        let high = permutation(
            &costs,
            Statistic::RelativeSum,
            |v| v / (1.0 + rule.equivalence),
            rule,
        );
        let (p_within_low, p_within_high) = (low.high, high.low);
        Cost {
            state: if p_within_low < rule.alpha && p_within_high < rule.alpha {
                CostState::Equivalent
            } else if plain.low < rule.alpha {
                CostState::Cheaper
            } else if plain.high < rule.alpha {
                CostState::MoreExpensive
            } else {
                CostState::Inconclusive
            },
            a: cost_a,
            b: costs.iter().map(|cell| mean(&cell.b)).sum(),
            relative: plain.observed,
            tests: costs.len(),
            p_cheaper: plain.low,
            p_more_expensive: plain.high,
            p_within_low,
            p_within_high,
        }
    });
    let verdict = quality.as_ref().map(|quality| {
        match (quality.state, cost.as_ref().map(|cost| cost.state)) {
            (QualityState::Inferior, _) => Verdict::Worse,
            (QualityState::NonInferior, Some(CostState::Cheaper)) => Verdict::Better,
            (QualityState::NonInferior, Some(CostState::Equivalent)) => Verdict::NoDifference,
            (QualityState::NonInferior, Some(CostState::MoreExpensive)) => Verdict::Worse,
            _ => Verdict::CannotTell,
        }
    });
    let qualifier = quality
        .as_ref()
        .filter(|quality| quality.state == QualityState::Fell)
        .map(|_| Qualifier::QualityFell);

    let tests = scores
        .iter()
        .map(|cell| {
            let own = permutation(
                std::slice::from_ref(cell),
                Statistic::MeanDifference,
                |v| v,
                rule,
            );
            TestDelta {
                scenario: cell.key.0.clone(),
                behavior: cell.key.1.clone(),
                runs: Sides {
                    a: cell.a.len(),
                    b: cell.b.len(),
                },
                a: mean(&cell.a),
                b: mean(&cell.b),
                delta: own.observed,
                p_fell: own.low,
                warning: own.observed < -rule.margin,
            }
        })
        .collect::<Vec<_>>();
    let repetitions = repetitions(&scores, quality.as_ref(), verdict, rule);
    let figures = figures(&shared, &cells_a, &cells_b);
    let earlier = a.iter().chain(b).filter(|run| run.cause.is_none()).count();
    let mut warnings = Vec::new();
    if earlier > 0 {
        warnings.push(format!(
            "{earlier} runs come from reports older than causes and were classified by the \
             earlier rule: every subject error left the mean, and a failure of the evaluator \
             itself could count as a valid zero."
        ));
    }
    warnings.extend(tests.iter().filter(|test| test.warning).map(|test| {
        format!(
            "{} fell {:.1} points, more than the {}-point margin (its own p = {:.3}).",
            test.scenario, -test.delta, rule.margin, test.p_fell
        )
    }));
    Comparison {
        reasons: reasons(quality.as_ref(), cost.as_ref(), rule),
        verdict,
        qualifier,
        unavailable,
        rule: *rule,
        quality,
        cost,
        figures,
        repetitions,
        tests,
        dropped: dropped(a, b, &cells_a, &cells_b),
        runs: Sides {
            a: counts(a),
            b: counts(b),
        },
        warnings,
        criteria: criteria::health(a, b, &shared),
    }
}

fn repetitions(
    scores: &[Cell],
    quality: Option<&Quality>,
    verdict: Option<Verdict>,
    rule: &Rule,
) -> Repetitions {
    let have = |pick: fn(&Cell) -> usize| scores.iter().map(pick).min().unwrap_or(0);
    let have = Sides {
        a: have(|cell| cell.a.len()),
        b: have(|cell| cell.b.len()),
    };
    // Pooled over every (test, side) with two runs or more.
    let (mut squares, mut freedom) = (0.0, 0usize);
    for values in scores.iter().flat_map(|cell| [&cell.a, &cell.b]) {
        if let Some(variance) = variance(values) {
            squares += variance * (values.len() - 1) as f64;
            freedom += values.len() - 1;
        }
    }
    let pooled = (freedom > 0).then(|| squares / freedom as f64);
    let mut repetitions = Repetitions {
        have,
        sigma: pooled.map(f64::sqrt),
        needed: None,
        missing: None,
        more_than: None,
        note: None,
    };
    let (Some(quality), Some(Verdict::CannotTell), Some(pooled)) = (quality, verdict, pooled)
    else {
        return repetitions;
    };
    if quality.state == QualityState::NonInferior {
        repetitions.note = Some("Quality is already non-inferior: the cost is undecided.".into());
        return repetitions;
    }
    let room = rule.margin + quality.delta;
    if room <= 0.0 {
        repetitions.note = Some(format!(
            "No number: the observed drop already reaches the {}-point margin.",
            rule.margin
        ));
        return repetitions;
    }
    let k = scores.len() as f64;
    let spread = scores
        .iter()
        .map(|cell| {
            let own = [variance(&cell.a), variance(&cell.b)];
            let own = own.iter().flatten().collect::<Vec<_>>();
            if own.is_empty() {
                pooled
            } else {
                own.iter().copied().sum::<f64>() / own.len() as f64
            }
        })
        .map(|variance| 2.0 * variance)
        .sum::<f64>();
    let n = spread / (k * k * (room / POWER_Z).powi(2));
    let needed = n.ceil().max(1.0) as usize;
    if needed > REPETITIONS_CAP {
        repetitions.more_than = Some(REPETITIONS_CAP);
    } else {
        repetitions.needed = Some(needed);
        repetitions.missing = Some(needed.saturating_sub(have.a.min(have.b)));
    }
    repetitions
}

/// Completion, then the supporting figures, over the tests compared.
fn figures(
    shared: &[Key],
    a: &BTreeMap<Key, Vec<&Run>>,
    b: &BTreeMap<Key, Vec<&Run>>,
) -> Vec<Figure> {
    let comparable = shared
        .iter()
        .flat_map(|key| a[key].iter().chain(&b[key]))
        .all(|run| run.cache_comparable != Some(false));
    type Of = fn(&Run) -> Option<f64>;
    let rows: [(&str, Of, bool, bool); 5] = [
        (
            "completion",
            |run| Some(f64::from(u8::from(run.completed))),
            false,
            true,
        ),
        (
            "wall_time_s",
            |run| run.wall_time_ms.map(|ms| ms / 1_000.0),
            true,
            true,
        ),
        (
            "processed_prompt_tokens",
            |run| run.processed_prompt_tokens,
            true,
            comparable,
        ),
        (
            "context_per_turn",
            |run| Some(run.processed_prompt_tokens? / run.turns.filter(|turns| *turns > 0.0)?),
            false,
            comparable,
        ),
        (
            "peak_context_tokens",
            |run| run.peak_context_tokens,
            false,
            comparable,
        ),
    ];
    rows.into_iter()
        .map(|(id, of, summed, comparable)| {
            // A test without the figure on a side leaves the figure out.
            let side = |cells: &BTreeMap<Key, Vec<&Run>>| {
                let means = shared
                    .iter()
                    .map(|key| {
                        let values = cells[key]
                            .iter()
                            .filter_map(|run| of(run))
                            .collect::<Vec<_>>();
                        (!values.is_empty()).then(|| mean(&values))
                    })
                    .collect::<Option<Vec<_>>>()
                    .filter(|means| !means.is_empty())?;
                let total = means.iter().sum::<f64>();
                Some(if summed {
                    total
                } else {
                    total / means.len() as f64
                })
            };
            let (a, b) = (side(a), side(b));
            Figure {
                id: id.to_owned(),
                a,
                b,
                delta: a.zip(b).map(|(a, b)| b - a),
                relative: a
                    .zip(b)
                    .filter(|(a, _)| *a != 0.0)
                    .map(|(a, b)| b / a - 1.0),
                comparable,
            }
        })
        .collect()
}

fn dropped(
    a: &[Run],
    b: &[Run],
    cells_a: &BTreeMap<Key, Vec<&Run>>,
    cells_b: &BTreeMap<Key, Vec<&Run>>,
) -> Vec<Dropped> {
    fn definitions(runs: &[Run]) -> BTreeMap<&str, BTreeSet<Option<&str>>> {
        let mut by = BTreeMap::<&str, BTreeSet<Option<&str>>>::new();
        for run in runs {
            by.entry(run.scenario.as_str())
                .or_default()
                .insert(run.behavior.as_deref());
        }
        by
    }
    let (in_a, in_b) = (definitions(a), definitions(b));
    let compared = |scenario: &str| {
        cells_a
            .keys()
            .any(|key| key.0 == scenario && cells_b.contains_key(key))
    };
    in_a.keys()
        .chain(in_b.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|scenario| !compared(scenario))
        .map(|scenario| {
            let reason = match (in_a.get(scenario), in_b.get(scenario)) {
                (Some(left), Some(right)) if left.is_disjoint(right) => "definition_changed",
                (Some(_), None) => "only_in_a",
                (None, Some(_)) => "only_in_b",
                _ if !cells_a.keys().any(|key| key.0 == **scenario) => "no_counted_run_in_a",
                _ => "no_counted_run_in_b",
            };
            Dropped {
                scenario: (*scenario).to_owned(),
                reason: reason.to_owned(),
            }
        })
        .collect()
}

fn counts(runs: &[Run]) -> RunCounts {
    let mut counts = RunCounts::default();
    for run in runs {
        if run.counted {
            counts.counted += 1;
        } else {
            counts.excluded += 1;
        }
        let cause = run.cause.as_deref().unwrap_or("earlier_rule");
        *counts.causes.entry(cause.to_owned()).or_default() += 1;
        counts.classes.add(criteria::class_of(run));
    }
    counts
}

/// A p-value as the reasons print it.
fn p(value: f64) -> String {
    if value < 0.001 {
        "p < 0.001".into()
    } else {
        format!("p = {value:.3}")
    }
}

fn reasons(quality: Option<&Quality>, cost: Option<&Cost>, rule: &Rule) -> Vec<String> {
    let mut reasons = Vec::new();
    if let Some(q) = quality {
        let (delta, margin) = (q.delta, rule.margin);
        let (within, below) = (p(q.p_non_inferior), p(q.p_inferior));
        reasons.push(match q.state {
            QualityState::NonInferior => {
                format!("Quality {delta:+.1} points: within the {margin}-point margin ({within}).")
            }
            QualityState::Inferior => {
                format!("Quality {delta:+.1} points: below the {margin}-point margin ({below}).")
            }
            QualityState::Fell => format!(
                "Quality {delta:+.1} points: it fell ({}), by an amount the runs cannot bound: \
                 neither within the {margin}-point margin ({within}) nor below it ({below}).",
                p(q.p_fell)
            ),
            QualityState::Inconclusive => format!(
                "Quality {delta:+.1} points: neither within the {margin}-point margin \
                 ({within}) nor below it ({below})."
            ),
        });
    }
    if let Some(c) = cost {
        let (change, within) = (c.relative * 100.0, rule.equivalence * 100.0);
        reasons.push(match c.state {
            CostState::Equivalent => format!(
                "Cost {change:+.1}%: within ±{within}% ({} and {}).",
                p(c.p_within_low),
                p(c.p_within_high)
            ),
            CostState::Cheaper => format!("Cost {change:+.1}%: cheaper ({}).", p(c.p_cheaper)),
            CostState::MoreExpensive => format!(
                "Cost {change:+.1}%: more expensive ({}).",
                p(c.p_more_expensive)
            ),
            CostState::Inconclusive => format!(
                "Cost {change:+.1}%: neither cheaper ({}), more expensive ({}) nor within \
                 ±{within}%.",
                p(c.p_cheaper),
                p(c.p_more_expensive)
            ),
        });
    } else if quality.is_some() {
        reasons.push("Cost: no subject cost recorded on side A.".into());
    }
    reasons
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Self::Better => "better",
            Self::NoDifference => "no difference",
            Self::Worse => "worse",
            Self::CannotTell => "cannot tell",
        }
    }
}

/// What `harness-e2e compare` prints.
pub fn text(comparison: &Comparison, label_a: &str, label_b: &str) -> String {
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    let rule = &comparison.rule;
    match (comparison.verdict, &comparison.unavailable) {
        (Some(verdict), _) => line(format!(
            "Verdict, {label_b} against {label_a}: {}{}",
            verdict.label(),
            comparison.qualifier.map_or("", |_| {
                " (quality fell, by an amount the runs cannot bound)"
            })
        )),
        (None, Some(why)) => line(format!("No calibrated verdict: {why}")),
        (None, None) => {}
    }
    for reason in &comparison.reasons {
        line(format!("  {reason}"));
    }
    if let Some(q) = &comparison.quality {
        line(format!(
            "Quality, official score, mean over {} tests: {label_a} {:.1}, {label_b} {:.1}",
            q.tests, q.a, q.b
        ));
    }
    if let Some(c) = &comparison.cost {
        line(format!(
            "Cost, subject US$ summed over {} tests: {label_a} {:.4}, {label_b} {:.4}",
            c.tests, c.a, c.b
        ));
    }
    let figures = comparison
        .figures
        .iter()
        .map(|figure| {
            let value = match (figure.id.as_str(), figure.delta, figure.relative) {
                ("completion", Some(delta), _) => format!("{:+.0} pp", delta * 100.0),
                (_, _, Some(relative)) => format!("{:+.1}%", relative * 100.0),
                _ => "—".into(),
            };
            let note = if figure.comparable {
                ""
            } else {
                " (not comparable)"
            };
            let label = match figure.id.as_str() {
                "wall_time_s" => "time",
                "processed_prompt_tokens" => "processed prompt",
                "context_per_turn" => "context per turn",
                "peak_context_tokens" => "peak context",
                other => other,
            };
            format!("{label} {value}{note}")
        })
        .collect::<Vec<_>>();
    line(format!("Beside it, never deciding: {}", figures.join(", ")));
    let r = &comparison.repetitions;
    let mut repetitions = format!(
        "Repetitions: at least {} per test on {label_a}, {} on {label_b}",
        r.have.a, r.have.b
    );
    if let Some(sigma) = r.sigma {
        repetitions += &format!("; σ per run {sigma:.1} points");
    }
    match (r.needed, r.missing, r.more_than, &r.note) {
        (Some(needed), Some(missing), _, _) => {
            repetitions += &format!(
                "; about {needed} per arm would bound the {}-point margin at the observed Δ: {missing} more",
                rule.margin
            )
        }
        (_, _, Some(cap), _) => {
            repetitions += &format!("; more than {cap} per arm would be needed at the observed Δ")
        }
        (_, _, _, Some(note)) => repetitions += &format!(". {note}"),
        _ => {}
    }
    line(repetitions);
    for test in &comparison.tests {
        line(format!(
            "  {:<32} {}/{}  {:6.1} → {:6.1}  {:+6.1}{}",
            test.scenario,
            test.runs.a,
            test.runs.b,
            test.a,
            test.b,
            test.delta,
            if test.warning {
                "  fell more than the margin"
            } else {
                ""
            }
        ));
    }
    for dropped in &comparison.dropped {
        line(format!(
            "  {} left out: {}",
            dropped.scenario,
            dropped.reason.replace('_', " ")
        ));
    }
    let causes = |counts: &RunCounts| {
        counts
            .causes
            .iter()
            .map(|(cause, n)| format!("{cause} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    line(format!(
        "Runs by cause: {label_a} {}; {label_b} {}",
        causes(&comparison.runs.a),
        causes(&comparison.runs.b)
    ));
    for warning in &comparison.warnings {
        line(format!("Warning: {warning}"));
    }
    out.push_str(&criteria::text(&comparison.criteria, label_a, label_b));
    out.push_str(&format!(
        "Rule: permutation tests within each test, one-sided α {} each, margin {} points, cost \
         within ±{}%, {} permutations, seed {}.\n",
        rule.alpha,
        rule.margin,
        rule.equivalence * 100.0,
        rule.permutations,
        rule.seed
    ));
    out
}

#[cfg(test)]
mod tests_support {
    use super::{runs_of, Run};
    use std::path::Path;

    /// One arm of the golden pair in `tests/fixtures/verdict/`.
    pub(super) fn fixture(name: &str) -> Vec<Run> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/verdict")
            .join(name);
        let document = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        runs_of([&document])
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::fixture;
    use super::*;

    fn expected() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/verdict/expected.json")).unwrap()
    }

    #[test]
    fn splitmix64_and_its_shuffle_match_the_shared_vectors() {
        let vectors = &expected()["prng"];
        let mut rng = SplitMix64(vectors["seed"].as_u64().unwrap());
        let first = (0..3)
            .map(|_| rng.next_u64().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            first,
            serde_json::from_value::<Vec<String>>(vectors["first"].clone()).unwrap()
        );
        let mut rng = SplitMix64(vectors["seed"].as_u64().unwrap());
        let mut values = (0..10).collect::<Vec<u32>>();
        rng.shuffle(&mut values);
        assert_eq!(
            values,
            serde_json::from_value::<Vec<u32>>(vectors["shuffle"].clone()).unwrap()
        );
    }

    #[test]
    fn the_pair_cannot_tell_and_software_engineering_is_cheaper() {
        let rule = Rule::default();
        for (suite, expected) in expected()["pairs"].as_object().unwrap() {
            let a = fixture(expected["a"].as_str().unwrap());
            let b = fixture(expected["b"].as_str().unwrap());
            let comparison = compare(&a, &b, &rule);
            let state = |value: Value| value.as_str().unwrap().to_owned();
            let verdict = state(serde_json::to_value(comparison.verdict).unwrap());
            let quality = &comparison.quality.as_ref().unwrap();
            assert_eq!(verdict, state(expected["verdict"].clone()), "{suite}");
            assert_eq!(
                state(serde_json::to_value(quality.state).unwrap()),
                state(expected["quality"].clone()),
                "{suite}"
            );
            assert_eq!(
                state(serde_json::to_value(comparison.cost.as_ref().unwrap().state).unwrap()),
                state(expected["cost"].clone()),
                "{suite}"
            );
            assert_eq!(comparison.qualifier, None, "{suite}");
            assert!(comparison
                .warnings
                .iter()
                .all(|w| !w.contains("older than causes")));
            // How many more repetitions, or why there is no number.
            let r = &comparison.repetitions;
            assert!(
                r.needed.is_some() || r.more_than.is_some() || r.note.is_some(),
                "{suite}"
            );
        }
        // The port collision is the evaluator's machine: out of the mean.
        let b = fixture("se-1.8.50.json");
        let se = compare(&fixture("se-1.8.44.json"), &b, &rule);
        assert_eq!(se.runs.b.causes.get("evaluation_environment"), Some(&1));
        assert_eq!(se.runs.b.counted, 29);
        let quality = se.quality.as_ref().unwrap();
        assert!((quality.delta + 3.86).abs() < 0.01, "{}", quality.delta);
        assert!((se.cost.as_ref().unwrap().relative + 0.198).abs() < 0.001);
        assert!(se
            .tests
            .iter()
            .any(|test| test.scenario == "kanban_c3_board" && test.warning));
    }

    #[test]
    fn counted_as_a_valid_zero_the_port_collision_pulls_quality_down() {
        let mut b = fixture("se-1.8.50.json");
        for run in &mut b {
            if run.cause.as_deref() == Some("evaluation_environment") {
                run.cause = Some("evaluated".into());
                run.counted = true;
            }
        }
        let comparison = compare(&fixture("se-1.8.44.json"), &b, &Rule::default());
        let quality = comparison.quality.unwrap();
        assert!((quality.delta + 6.99).abs() < 0.01, "{}", quality.delta);
        assert_eq!(comparison.verdict, Some(Verdict::CannotTell));
    }

    #[test]
    fn halves_of_one_version_rarely_give_a_false_signal() {
        let calibration = &expected()["calibration"];
        let splits = calibration["splits_per_arm"].as_u64().unwrap() as usize;
        let rule = Rule {
            permutations: calibration["permutations"].as_u64().unwrap() as usize,
            ..Rule::default()
        };
        let mut rng = SplitMix64(calibration["seed"].as_u64().unwrap());
        let (mut trials, mut false_signals) = (0, 0);
        for arm in [
            "se-1.8.44.json",
            "se-1.8.50.json",
            "regression-1.8.44.json",
            "regression-1.8.50.json",
        ] {
            let runs = fixture(arm);
            let mut by = BTreeMap::<&str, Vec<&Run>>::new();
            for run in runs.iter().filter(|run| run.counted) {
                by.entry(&run.scenario).or_default().push(run);
            }
            for _ in 0..splits {
                let (mut one, mut two) = (Vec::new(), Vec::new());
                for runs in by.values() {
                    let mut runs = runs.clone();
                    rng.shuffle(&mut runs);
                    let half = runs.len() / 2;
                    one.extend(runs[..half].iter().map(|run| (*run).clone()));
                    two.extend(runs[half..].iter().map(|run| (*run).clone()));
                }
                let comparison = compare(&one, &two, &rule);
                trials += 1;
                false_signals += usize::from(
                    matches!(comparison.verdict, Some(Verdict::Better | Verdict::Worse))
                        || comparison.qualifier.is_some(),
                );
            }
        }
        let rate = false_signals as f64 / trials as f64;
        let ceiling = calibration["max_false_signal_rate"].as_f64().unwrap();
        assert!(
            rate < ceiling,
            "{false_signals} of {trials} splits gave a false signal"
        );
    }

    #[test]
    fn one_run_per_test_has_no_calibrated_verdict_and_dropped_tests_say_why() {
        let run = |attempt: &str, scenario: &str, behavior: &str, score: f64| Run {
            attempt_id: attempt.into(),
            scenario: scenario.into(),
            behavior: Some(behavior.into()),
            cause: Some("evaluated".into()),
            counted: true,
            score: Some(score),
            completed: true,
            cost_usd: Some(1.0),
            ..Run::default()
        };
        let a = [
            run("a1", "t1", "s1", 80.0),
            run("a2", "t2", "s2", 90.0),
            run("a3", "t3", "s3", 70.0),
        ];
        let mut infra = run("b4", "t4", "s4", 0.0);
        infra.counted = false;
        infra.cause = Some("provider_infrastructure".into());
        let b = [
            run("b1", "t1", "s1", 60.0),
            run("b2", "t2", "other", 90.0),
            infra,
        ];
        let comparison = compare(&a, &b, &Rule::default());
        assert_eq!(comparison.verdict, None);
        assert!(comparison
            .unavailable
            .unwrap()
            .starts_with("One run per test"));
        assert_eq!(
            comparison
                .dropped
                .iter()
                .map(|d| (d.scenario.as_str(), d.reason.as_str()))
                .collect::<Vec<_>>(),
            [
                ("t2", "definition_changed"),
                ("t3", "only_in_a"),
                ("t4", "only_in_b")
            ]
        );
        assert_eq!(comparison.runs.b.excluded, 1);
        assert_eq!(comparison.tests[0].delta, -20.0);
        // Reports older than causes fall back to `technical`, and say so.
        let older = runs_of([
            &serde_json::json!({"scenarios": [{"scenario_id": "t1", "runs": [
                {"attempt_id": "x", "technical": "valid", "score": 50},
                {"attempt_id": "y", "technical": "technical_invalid", "score": 0},
            ]}]}),
        ]);
        assert_eq!(
            older.iter().map(|run| run.counted).collect::<Vec<_>>(),
            [true, false]
        );
        assert!(compare(&older, &older, &Rule::default()).warnings[0].contains("older than causes"));
    }

    #[test]
    fn clear_changes_read_as_better_no_difference_and_worse() {
        let arm = |prefix: &str, scores: [f64; 5], cost: f64| {
            (0..3)
                .flat_map(|test| {
                    scores.iter().enumerate().map(move |(i, score)| Run {
                        attempt_id: format!("{prefix}{test}{i}"),
                        scenario: format!("t{test}"),
                        behavior: Some(format!("s{test}")),
                        cause: Some("evaluated".into()),
                        counted: true,
                        score: Some(*score),
                        completed: true,
                        cost_usd: Some(cost * (1.0 + i as f64 * 0.001)),
                        ..Run::default()
                    })
                })
                .collect::<Vec<_>>()
        };
        let base = arm("a", [90.0, 91.0, 92.0, 90.0, 91.0], 1.0);
        let rule = Rule::default();
        let verdict = |b: Vec<Run>| compare(&base, &b, &rule).verdict;
        assert_eq!(
            verdict(arm("b", [90.0, 91.0, 92.0, 90.0, 91.0], 0.5)),
            Some(Verdict::Better)
        );
        assert_eq!(
            verdict(arm("b", [91.0, 90.0, 92.0, 91.0, 90.0], 1.0)),
            Some(Verdict::NoDifference)
        );
        assert_eq!(
            verdict(arm("b", [91.0, 90.0, 92.0, 91.0, 90.0], 2.0)),
            Some(Verdict::Worse)
        );
        assert_eq!(
            verdict(arm("b", [60.0, 61.0, 62.0, 60.0, 61.0], 0.5)),
            Some(Verdict::Worse)
        );
        // Five points down, every run: it fell, by about the margin itself.
        let fell = compare(&base, &arm("b", [85.0, 86.0, 87.0, 85.0, 86.0], 0.5), &rule);
        assert_eq!(fell.verdict, Some(Verdict::CannotTell));
        assert_eq!(fell.qualifier, Some(Qualifier::QualityFell));
        assert!(fell.repetitions.note.is_some());
        let unsure = compare(
            &base,
            &arm("b", [70.0, 100.0, 80.0, 95.0, 85.0], 1.0),
            &rule,
        );
        assert_eq!(unsure.verdict, Some(Verdict::CannotTell));
        assert!(unsure.repetitions.missing.is_some() || unsure.repetitions.more_than.is_some());
    }
}
