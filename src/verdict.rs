//! One calibrated verdict on two arms: B against A is better, no different,
//! worse, or the runs cannot tell yet. `harness-e2e compare`, the Console's
//! comparison and the Trends overview read it from here; Release Control
//! reproduces it from this description and `tests/fixtures/verdict/`.
//!
//! Reading. Every `scenarios[].runs[]` of the results documents, in order. A
//! run is keyed by (`scenario_id`, `attempt_id`, else `run_id`): a key read
//! again with the same content is skipped (a root bundle carries its groups'
//! results too), with other content it is skipped with a warning, the first
//! read kept; an attempt id under several scenarios is kept under each, with
//! a warning. A run without an id is never skipped.
//!
//! Inputs. Counted runs only: those whose `cause` counts; a run without a
//! cause, or with one this version does not know, falls back to
//! `technical == "valid"`, with a warning. A test is a (`scenario_id`,
//! `behavior_sha256`) both arms ran; any other is dropped and listed, one
//! entry per definition. Within a test the runs are ordered by `attempt_id`
//! (byte order); tests by `scenario_id`, then `behavior_sha256` (none first).
//! A score is a number (`score`); a cost is a finite `cost.subject_usd`
//! above 0. A counted run of a compared test without one leaves that figure,
//! with a warning that counts and names them. A figure reads the tests with a
//! value on both sides (cost over fewer tests than quality is a warning), and
//! there is a verdict only when some test has two scores or more on each
//! side. An arm that mixes models or stacks, sides that differ in more than
//! the Harness, and a results.json that does not parse are warnings.
//!
//! Figures. Quality is the official run score: the suite figure is the mean
//! over tests of each test's mean, Δ = (Σ mean_B − Σ mean_A) / K. Cost is
//! `cost.subject_usd`: the suite figure is the sum over tests of each test's
//! mean, shown as the change Σ mean_B / Σ mean_A − 1 and tested as the
//! difference Σ mean_B − Σ mean_A over the observed Σ mean_A. Every sum adds
//! left to right from 0.0; a mean divides that sum by the count. Completion
//! is shown, and time, processed prompt, context per turn and its peak are
//! supporting figures: none of them decides.
//!
//! Permutation test (labels exchanged within each test). Each test below
//! draws from its own SplitMix64 seeded with `seed`. In each of
//! `permutations` rounds, for each test in order, the pool is A's values then
//! B's transformed values, in run order; Fisher–Yates runs i from the last
//! index down to 1 and swaps i with `next() % (i + 1)`; the first |A| values
//! are A's. The statistic is Δ over the pools for quality; for cost, Σ mean_B
//! − Σ mean_A over the pools divided by the observed Σ mean_A (the same
//! number in every round). A round whose statistic t ≤ observed + 1e-12
//! counts low, one with t ≥ observed − 1e-12 counts high;
//! p = (count + 1) / (rounds + 1). When the observed statistic or any
//! round's is not finite, both p are 1, with a warning.
//!
//! Tests, each one-sided, α = 0.025. Quality as is: `p_fell` is low,
//! `p_improved` high. Quality with B + margin: `p_non_inferior` is high,
//! `p_inferior` low. Cost as is: `p_cheaper` low, `p_more_expensive` high.
//! Cost with B / (1 − equivalence): `p_within_low` is high; with
//! B / (1 + equivalence): `p_within_high` is low. Each compared test alone:
//! `p_fell` as is, and `p_inferior` with B + margin, Holm-adjusted over the K
//! tests (ascending, ties in test order; adjusted p_(i) = the largest over
//! j ≤ i of min(1, (K − j + 1) p_(j))).
//!
//! States. Quality: improved when `p_improved` < α / 2, else non-inferior
//! when `p_non_inferior` < α, else inferior when `p_inferior` < α, else fell
//! when `p_fell` < α / 2, else inconclusive. Cost: equivalent when both
//! `p_within_*` < α, else cheaper when `p_cheaper` < α / 2, else below the
//! bound when `p_within_high` < α, else more expensive when
//! `p_more_expensive` < α / 2, else inconclusive; no cost reads as
//! inconclusive. A test is inferior when its adjusted `p_inferior` < α and
//! its Δ < −margin, a warning that names it. Verdict: worse = quality
//! inferior, or cost more expensive unless quality improved; better = quality
//! improved and cost equivalent, below the bound or cheaper, or quality
//! non-inferior and cost cheaper; no difference = quality non-inferior and
//! cost equivalent or below the bound; anything else cannot tell. An inferior
//! test turns better or no difference into cannot tell. Cannot tell is
//! qualified `test_inferior` when a test is inferior, else `quality_fell`
//! when quality fell, else `trade_off` when quality improved and cost is more
//! expensive.
//!
//! Calibration and order. A version against itself reads better, worse or a
//! qualifier at most about 2α = 5% of the time: quality moves either way at
//! α / 2 (inferior needs a fall beyond the margin too), cost either way at
//! α / 2, and a test is inferior at α over all of them together. The table is
//! monotone, worst first: quality inferior, fell, inconclusive, non-inferior,
//! improved; cost more expensive, inconclusive, equivalent, below the bound,
//! cheaper; verdict worse, cannot tell, no difference, better. Raising a B
//! score or lowering a B cost moves each of its side's statistics against
//! the observed one the same way in every round, so no p, state or verdict
//! gets worse (cheaper implies below the bound: B divided by 1 + equivalence
//! is B lowered).
//!
//! Repetitions. When the runs cannot tell, the runs per arm N that would
//! settle what is undecided, the runs already made kept: quality when it is
//! neither improved nor non-inferior (non-inferior when M + Δ > 0, inferior
//! below); and, unless quality points to inferior, an inconclusive cost
//! (below the bound when its change is under `equivalence`, else more
//! expensive, unless quality improved). For each, with d the observed
//! distance (|M + Δ|; Σ mean_A − Σ mean_B / (1 + equivalence); Σ mean_B − Σ
//! mean_A), s² = Σ_k 2 s_k² (over K² for quality), s_k² the sample variance
//! of test k's pool as that test exchanges it (A's values, then B + M, B /
//! (1 + equivalence) or B), n₀ the fewest scores a compared test has on a
//! side, and z = 1.959964 at α or 2.241403 at α / 2: N is the fewest from
//! the larger of n₀ + 1 and f up to 30 with
//! d − 0.841621 s √(1/n₀ − 1/N) ≥ z s / √N, the estimate over N runs then
//! clearing its test's threshold with a predicted 80%, the true difference as
//! uncertain as the runs measured it (normal); f is the fewest runs per arm
//! whose smallest p, 1 / Π C(2n, n) over the tests whose pool is not
//! constant, is below the test's level. `needed` is the largest N, "more
//! than 30" when one has none. No number while a test is inferior, when
//! quality improved and the cost is or may be more expensive, or without a
//! cost.
mod criteria;

use std::collections::btree_map::Entry;
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
/// One-sided normal quantiles of the default α = 0.025, of α / 2, and of a
/// predicted 80%.
const Z_ALPHA: f64 = 1.959_964;
const Z_HALF_ALPHA: f64 = 2.241_403;
const Z_POWER: f64 = 0.841_621;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    /// Points of quality B may lose and still be non-inferior.
    pub margin: f64,
    /// Relative cost change that counts as no change.
    pub equivalence: f64,
    /// One-sided, for each quality test and the equivalence; each cost
    /// direction is tested at half of it.
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
    /// `provider/model`, from its results document.
    pub model: Option<String>,
    /// Each worker's resolved version, `iii` the engine and `lock` the stack
    /// lock digest, from its results document; empty when not recorded.
    pub stack: BTreeMap<String, String>,
}

/// What one criterion awarded in a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Points {
    pub id: String,
    /// None when the evaluator did not evaluate it.
    pub awarded: Option<f64>,
    pub possible: f64,
}

/// The runs of results documents (`results.json`, or a report the Console
/// projects from one): every `scenarios[].runs[]`, a run of a test seen
/// twice counted once (a root bundle carries its groups' results too), with
/// a warning for a repeat whose content differs and for an attempt id that
/// more than one test uses.
pub fn runs_of<'a>(documents: impl IntoIterator<Item = &'a Value>) -> (Vec<Run>, Vec<String>) {
    let mut seen = BTreeMap::<(&str, &str), &Value>::new();
    let mut tests_of = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut conflicts = Vec::new();
    let mut runs = Vec::new();
    for document in documents {
        let subject = &document["subject"];
        let model = subject["provider"]
            .as_str()
            .zip(subject["model"].as_str())
            .map(|(provider, model)| format!("{provider}/{model}"));
        let system = &document["system_under_test"];
        let mut stack = system["stack"]["stack_versions"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(name, version)| Some((name.clone(), version.as_str()?.to_owned())))
            .collect::<BTreeMap<_, _>>();
        for (name, value) in [
            ("iii", &system["engine_version"]),
            ("lock", &system["stack"]["stack_lock_digest"]),
        ] {
            if let Some(value) = value.as_str() {
                stack.insert(name.to_owned(), value.to_owned());
            }
        }
        for scenario in document["scenarios"].as_array().into_iter().flatten() {
            let Some(id) = scenario["scenario_id"].as_str() else {
                continue;
            };
            for run in scenario["runs"].as_array().into_iter().flatten() {
                let attempt = run["attempt_id"]
                    .as_str()
                    .or(run["run_id"].as_str())
                    .unwrap_or_default();
                if !attempt.is_empty() {
                    match seen.entry((id, attempt)) {
                        Entry::Occupied(first) => {
                            if *first.get() != run {
                                conflicts.push(format!("{id} {attempt}"));
                            }
                            continue;
                        }
                        Entry::Vacant(slot) => {
                            slot.insert(run);
                        }
                    }
                    tests_of.entry(attempt).or_default().insert(id);
                }
                runs.push(Run {
                    model: model.clone(),
                    stack: stack.clone(),
                    ..run_of(id, scenario["behavior_sha256"].as_str(), attempt, run)
                });
            }
        }
    }
    let mut warnings = Vec::new();
    if !conflicts.is_empty() {
        warnings.push(format!(
            "{} runs repeat an attempt already read for the same test, with other content; the \
             first read is kept: {}.",
            conflicts.len(),
            listed(&conflicts)
        ));
    }
    let shared = tests_of
        .iter()
        .filter(|(_, tests)| tests.len() > 1)
        .map(|(attempt, tests)| {
            format!(
                "{attempt} ({})",
                tests.iter().copied().collect::<Vec<_>>().join(", ")
            )
        })
        .collect::<Vec<_>>();
    if !shared.is_empty() {
        warnings.push(format!(
            "{} attempt ids appear under more than one test, kept under each: {}.",
            shared.len(),
            listed(&shared)
        ));
    }
    (runs, warnings)
}

/// The first five, then how many more.
fn listed(items: &[String]) -> String {
    let mut text = items.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 5 {
        text += &format!(" and {} more", items.len() - 5);
    }
    text
}

/// One arm's runs: a results.json, or every results.json below a directory,
/// and a warning for each file that could not be read or parsed (skipped)
/// and each run left out or kept with a doubt.
pub fn read_arm(path: &std::path::Path) -> anyhow::Result<(Vec<Run>, Vec<String>)> {
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
    let (mut documents, mut skipped) = (Vec::new(), Vec::new());
    for file in &paths {
        match std::fs::read(file)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<Value>(&bytes)?))
        {
            Ok(document) => documents.push(document),
            Err(error) => skipped.push(format!("Skipped {}: {error}.", file.display())),
        }
    }
    let (runs, warnings) = runs_of(&documents);
    skipped.extend(warnings);
    if runs.is_empty() {
        anyhow::bail!(
            "no run in any readable results.json at {} ({} skipped)",
            path.display(),
            skipped.len()
        );
    }
    Ok((runs, skipped))
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
                awarded: criterion["awarded"].as_f64(),
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
        ..Run::default()
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
    /// A test fell beyond the margin on its own, whatever the mean says.
    TestInferior,
    /// Quality fell, by an amount the runs cannot bound.
    QualityFell,
    /// Quality improved and the cost rose: the rule does not weigh one
    /// against the other.
    TradeOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualityState {
    /// B scores higher than A.
    Improved,
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
    /// Below +`equivalence`, but neither within −`equivalence` nor cheaper.
    BelowBound,
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
    pub p_improved: f64,
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
    /// B / A − 1, shown; the tests read B − A.
    pub relative: f64,
    pub tests: usize,
    /// Each against α / 2.
    pub p_cheaper: f64,
    pub p_more_expensive: f64,
    /// Against B costing `equivalence` less or more; both under α is
    /// equivalent, the high one alone below the bound.
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
    /// Pooled within-test standard deviation of the score, per run: the
    /// noise, shown; `needed` reads the spread each test exchanges.
    pub sigma: Option<f64>,
    /// Runs per arm, those made kept, that would settle what is undecided
    /// with a predicted 80%; only when the runs cannot tell and a number
    /// exists.
    pub needed: Option<usize>,
    /// `needed` less the fewest of `have`, at least 1.
    pub missing: Option<usize>,
    /// Set to the cap when more than it would be needed.
    pub more_than: Option<usize>,
    /// What `needed` or `more_than` settles: `quality` (the margin) and
    /// `cost` (below the bound, or more expensive).
    pub settles: Vec<String>,
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
    /// This test's own permutation test of a drop beyond the margin, and it
    /// Holm-adjusted over the tests compared.
    pub p_inferior: f64,
    pub p_inferior_adjusted: f64,
    /// It fell beyond the margin on its own (adjusted p under α): a warning,
    /// and the verdict cannot read better or no difference.
    pub warning: bool,
}

/// A definition of a test left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dropped {
    pub scenario: String,
    pub behavior: Option<String>,
    /// `definition_changed` (the other side ran the scenario under another
    /// definition), `only_in_a`, `only_in_b`, `no_counted_run_in_a` or
    /// `no_counted_run_in_b`.
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
    /// The comparison's warnings, a test that fell beyond the margin among
    /// them.
    pub warnings: Vec<String>,
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
            warnings: self.warnings,
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
    /// (Σ mean_B − Σ mean_A) / A, A the observed Σ mean_A: one number in
    /// every round and none of B's, so lowering a B value lowers the
    /// statistic against the observed one in every round, which a ratio of
    /// the permuted sums does not, and the tie tolerance is relative.
    RelativeDifference,
}

/// The observed statistic and its one-sided p-values, both 1 when a
/// statistic was not finite.
struct Tails {
    observed: f64,
    low: f64,
    high: f64,
    finite: bool,
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

/// Σ mean_B − Σ mean_A over pools whose first `split` values are A's,
/// divided by `per`.
fn statistic(pools: &[(usize, Vec<f64>)], per: f64) -> f64 {
    let (mut a, mut b) = (0.0, 0.0);
    for (split, pool) in pools {
        a += mean(&pool[..*split]);
        b += mean(&pool[*split..]);
    }
    (b - a) / per
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
    let per = match how {
        Statistic::MeanDifference => cells.len() as f64,
        Statistic::RelativeDifference => cells.iter().map(|cell| mean(&cell.a)).sum::<f64>(),
    };
    let observed = statistic(&pools, per);
    let mut finite = observed.is_finite();
    let mut rng = SplitMix64(rule.seed);
    let mut work = pools.clone();
    let (mut low, mut high) = (0usize, 0usize);
    for _ in 0..rule.permutations {
        if !finite {
            break;
        }
        for ((_, pool), (_, original)) in work.iter_mut().zip(&pools) {
            pool.copy_from_slice(original);
            rng.shuffle(pool);
        }
        let t = statistic(&work, per);
        finite = t.is_finite();
        low += usize::from(t <= observed + TIE);
        high += usize::from(t >= observed - TIE);
    }
    let p = |count: usize| {
        if finite {
            (count + 1) as f64 / (rule.permutations + 1) as f64
        } else {
            1.0
        }
    };
    Tails {
        observed,
        low: p(low),
        high: p(high),
        finite,
    }
}

/// A score the verdict reads.
fn score_of(run: &Run) -> Option<f64> {
    run.score.filter(|score| score.is_finite())
}

/// A cost the verdict reads: finite and above 0.
fn cost_of(run: &Run) -> Option<f64> {
    run.cost_usd.filter(|cost| cost.is_finite() && *cost > 0.0)
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
    let scores = values(&shared, &cells_a, &cells_b, score_of);
    let costs = values(&shared, &cells_a, &cells_b, cost_of);
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
    let (alpha, half) = (rule.alpha, rule.alpha / 2.0);
    let mut not_finite = Vec::new();

    let quality = if unavailable.is_none() {
        let fell = permutation(&scores, Statistic::MeanDifference, |v| v, rule);
        let shifted = permutation(
            &scores,
            Statistic::MeanDifference,
            |v| v + rule.margin,
            rule,
        );
        if !(fell.finite && shifted.finite) {
            not_finite.push("quality");
        }
        let k = scores.len() as f64;
        let (p_improved, p_non_inferior, p_inferior, p_fell) =
            (fell.high, shifted.high, shifted.low, fell.low);
        Some(Quality {
            state: if p_improved < half {
                QualityState::Improved
            } else if p_non_inferior < alpha {
                QualityState::NonInferior
            } else if p_inferior < alpha {
                QualityState::Inferior
            } else if p_fell < half {
                QualityState::Fell
            } else {
                QualityState::Inconclusive
            },
            a: scores.iter().map(|cell| mean(&cell.a)).sum::<f64>() / k,
            b: scores.iter().map(|cell| mean(&cell.b)).sum::<f64>() / k,
            delta: fell.observed,
            tests: scores.len(),
            p_improved,
            p_non_inferior,
            p_inferior,
            p_fell,
        })
    } else {
        None
    };
    let cost = if unavailable.is_none() && !costs.is_empty() {
        let plain = permutation(&costs, Statistic::RelativeDifference, |v| v, rule);
        let low = permutation(
            &costs,
            Statistic::RelativeDifference,
            |v| v / (1.0 - rule.equivalence),
            rule,
        );
        let high = permutation(
            &costs,
            Statistic::RelativeDifference,
            |v| v / (1.0 + rule.equivalence),
            rule,
        );
        if !(plain.finite && low.finite && high.finite) {
            not_finite.push("cost");
        }
        let (p_within_low, p_within_high) = (low.high, high.low);
        let (cost_a, cost_b) = (
            costs.iter().map(|cell| mean(&cell.a)).sum::<f64>(),
            costs.iter().map(|cell| mean(&cell.b)).sum::<f64>(),
        );
        Some(Cost {
            state: if p_within_low < alpha && p_within_high < alpha {
                CostState::Equivalent
            } else if plain.low < half {
                CostState::Cheaper
            } else if p_within_high < alpha {
                CostState::BelowBound
            } else if plain.high < half {
                CostState::MoreExpensive
            } else {
                CostState::Inconclusive
            },
            a: cost_a,
            b: cost_b,
            relative: cost_b / cost_a - 1.0,
            tests: costs.len(),
            p_cheaper: plain.low,
            p_more_expensive: plain.high,
            p_within_low,
            p_within_high,
        })
    } else {
        None
    };

    let tests = tests(&scores, rule);
    let blocked = tests.iter().any(|test| test.warning);
    let states = quality
        .as_ref()
        .map(|quality| (quality.state, cost.as_ref().map(|cost| cost.state)));
    let verdict = states.map(|states| verdict_of(states.0, states.1, blocked));
    let qualifier = match (verdict, states) {
        (Some(Verdict::CannotTell), _) if blocked => Some(Qualifier::TestInferior),
        (Some(Verdict::CannotTell), Some((QualityState::Fell, _))) => Some(Qualifier::QualityFell),
        (
            Some(Verdict::CannotTell),
            Some((QualityState::Improved, Some(CostState::MoreExpensive))),
        ) => Some(Qualifier::TradeOff),
        _ => None,
    };
    let repetitions = repetitions(
        &scores,
        &costs,
        quality.as_ref(),
        cost.as_ref(),
        verdict,
        blocked,
        rule,
    );
    let figures = figures(&shared, &cells_a, &cells_b);
    let earlier = a.iter().chain(b).filter(|run| run.cause.is_none()).count();
    let mut warnings = Vec::new();
    if earlier > 0 {
        warnings.push(format!(
            "{earlier} runs carry no cause (from a report older than causes, or read where \
             causes are not kept) and were classified by the earlier rule: every subject error \
             left the mean, and a failure of the evaluator itself could count as a valid zero."
        ));
    }
    warnings.extend(input_warnings(a, b, &shared));
    if quality.is_some() && cost.is_some() && costs.len() < scores.len() {
        let priced = costs.iter().map(|cell| cell.key).collect::<BTreeSet<_>>();
        let unpriced = scores
            .iter()
            .filter(|cell| !priced.contains(cell.key))
            .map(|cell| cell.key.0.clone())
            .collect::<Vec<_>>();
        warnings.push(format!(
            "The cost is read over {} of the {} tests, without {}: no subject cost above 0 on a \
             side.",
            costs.len(),
            scores.len(),
            listed(&unpriced)
        ));
    }
    for figure in not_finite {
        warnings.push(format!(
            "The {figure} does not add up to a finite number: each of its tests reads p = 1."
        ));
    }
    warnings.extend(configuration_warnings(a, b));
    warnings.extend(tests.iter().filter(|test| test.warning).map(|test| {
        format!(
            "{} fell {:.1} points, beyond the {}-point margin on its own ({} Holm-adjusted over \
             {} tests): the verdict cannot read better or no difference.",
            test.scenario,
            -test.delta,
            rule.margin,
            p(test.p_inferior_adjusted),
            tests.len()
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

/// Each compared test on its own: its fall, and its fall beyond the margin
/// Holm-adjusted over the tests.
fn tests(scores: &[Cell], rule: &Rule) -> Vec<TestDelta> {
    let mut tests = scores
        .iter()
        .map(|cell| {
            let own = |transform: fn(f64, f64) -> f64| {
                permutation(
                    std::slice::from_ref(cell),
                    Statistic::MeanDifference,
                    |v| transform(v, rule.margin),
                    rule,
                )
            };
            let (fell, shifted) = (own(|v, _| v), own(|v, margin| v + margin));
            TestDelta {
                scenario: cell.key.0.clone(),
                behavior: cell.key.1.clone(),
                runs: Sides {
                    a: cell.a.len(),
                    b: cell.b.len(),
                },
                a: mean(&cell.a),
                b: mean(&cell.b),
                delta: fell.observed,
                p_fell: fell.low,
                p_inferior: shifted.low,
                p_inferior_adjusted: 1.0,
                warning: false,
            }
        })
        .collect::<Vec<_>>();
    let adjusted = holm(&tests.iter().map(|test| test.p_inferior).collect::<Vec<_>>());
    for (test, adjusted) in tests.iter_mut().zip(adjusted) {
        test.p_inferior_adjusted = adjusted;
        test.warning = adjusted < rule.alpha && test.delta < -rule.margin;
    }
    tests
}

/// Holm's step-down adjustment: ascending, ties in input order, each the
/// largest over those before it of min(1, (K − j + 1) p_(j)).
fn holm(p: &[f64]) -> Vec<f64> {
    let mut order = (0..p.len()).collect::<Vec<_>>();
    order.sort_by(|&left, &right| p[left].total_cmp(&p[right]));
    let mut adjusted = vec![1.0; p.len()];
    let mut running = 0.0_f64;
    for (rank, &index) in order.iter().enumerate() {
        running = running.max(((p.len() - rank) as f64 * p[index]).min(1.0));
        adjusted[index] = running;
    }
    adjusted
}

/// "t0 ×3, t2 ×1": how many of `runs` each test has, and their number.
fn tally<'a>(runs: impl IntoIterator<Item = &'a Run>) -> (usize, String) {
    let mut by = BTreeMap::<&str, usize>::new();
    for run in runs {
        *by.entry(run.scenario.as_str()).or_default() += 1;
    }
    let names = by
        .iter()
        .map(|(scenario, n)| format!("{scenario} ×{n}"))
        .collect::<Vec<_>>();
    (by.values().sum(), listed(&names))
}

/// Each run of a compared test a figure leaves out, and each run whose
/// cause this version does not know.
fn input_warnings(a: &[Run], b: &[Run], shared: &[Key]) -> Vec<String> {
    let shared = shared.iter().collect::<BTreeSet<_>>();
    let mut warnings = Vec::new();
    for (side, runs) in [("A", a), ("B", b)] {
        let compared = || {
            runs.iter().filter(|run| {
                run.counted && shared.contains(&(run.scenario.clone(), run.behavior.clone()))
            })
        };
        type Leaves = fn(&Run) -> bool;
        let rows: [(&str, &str, Leaves); 3] = [
            ("no numeric score", "quality", |run| score_of(run).is_none()),
            ("no subject cost", "cost", |run| run.cost_usd.is_none()),
            ("a subject cost at or below 0", "cost", |run| {
                run.cost_usd.is_some() && cost_of(run).is_none()
            }),
        ];
        for (what, figure, leaves) in rows {
            let (n, names) = tally(compared().filter(|run| leaves(run)));
            if n > 0 {
                warnings.push(format!(
                    "{side}: {n} counted runs have {what} and leave the {figure}: {names}."
                ));
            }
        }
        let unknown = runs
            .iter()
            .filter(|run| {
                run.cause.as_deref().is_some_and(|cause| {
                    serde_json::from_value::<RunCause>(Value::from(cause)).is_err()
                })
            })
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            let causes = unknown
                .iter()
                .filter_map(|run| run.cause.as_deref())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            let (n, names) = tally(unknown);
            warnings.push(format!(
                "{side}: {n} runs carry a cause this version does not know ({causes}) and were \
                 classified by `technical`: {names}."
            ));
        }
    }
    warnings
}

/// The verdict table; no cost reads like an inconclusive one, and an
/// inferior test holds back better and no difference.
fn verdict_of(quality: QualityState, cost: Option<CostState>, blocked: bool) -> Verdict {
    use CostState::{BelowBound, Cheaper, Equivalent, MoreExpensive};
    use QualityState::{Improved, Inferior, NonInferior};
    let verdict = match (quality, cost) {
        (Inferior, _) => Verdict::Worse,
        (Improved, Some(MoreExpensive)) => Verdict::CannotTell,
        (_, Some(MoreExpensive)) => Verdict::Worse,
        (Improved, Some(Equivalent | BelowBound | Cheaper)) => Verdict::Better,
        (NonInferior, Some(Cheaper)) => Verdict::Better,
        (NonInferior, Some(Equivalent | BelowBound)) => Verdict::NoDifference,
        _ => Verdict::CannotTell,
    };
    match verdict {
        Verdict::Better | Verdict::NoDifference if blocked => Verdict::CannotTell,
        verdict => verdict,
    }
}

/// The fewest runs per arm, up to one above the cap, whose smallest
/// permutation p over `tests` tests that vary, 1 / Π C(2n, n), is below
/// `level`.
fn fewest_runs(tests: usize, level: f64) -> Option<usize> {
    // ln C(2n, n).
    let ln_ways = |n: usize| {
        (1..=n)
            .map(|i| ((n + i) as f64 / i as f64).ln())
            .sum::<f64>()
    };
    (1..=REPETITIONS_CAP + 1).find(|n| tests as f64 * ln_ways(*n) > (1.0 / level).ln())
}

/// Each test's pool as a test exchanges it: A's values, then B's transformed.
fn pools(cells: &[Cell], transform: impl Fn(f64) -> f64) -> Vec<Vec<f64>> {
    cells
        .iter()
        .map(|cell| {
            let pool = cell.a.iter().copied();
            pool.chain(cell.b.iter().map(|v| transform(*v))).collect()
        })
        .collect()
}

/// The fewest runs per arm N, from `n0` + 1 to the cap, the runs made kept,
/// at which a test at `distance` from its threshold would settle with a
/// predicted 80%: d − 0.841621 s √(1/n₀ − 1/N) ≥ z s / √N, s² the pools'
/// Σ 2 s_k² over `scale`²; at least the fewest runs whose permutation p can
/// reach `level`. None above the cap.
fn plan(
    distance: f64,
    pools: &[Vec<f64>],
    scale: f64,
    n0: usize,
    z: f64,
    level: f64,
) -> Option<usize> {
    let spread = pools
        .iter()
        .map(|pool| 2.0 * variance(pool).unwrap_or(0.0))
        .sum::<f64>();
    let s = spread.sqrt() / scale;
    let varying = pools
        .iter()
        .filter(|pool| pool.iter().any(|v| *v != pool[0]))
        .count();
    let floor = fewest_runs(varying, level)?;
    let n0 = n0 as f64;
    (floor.max(n0 as usize + 1)..=REPETITIONS_CAP).find(|n| {
        let n = *n as f64;
        distance - Z_POWER * s * (1.0 / n0 - 1.0 / n).sqrt() >= z * s / n.sqrt()
    })
}

fn repetitions(
    scores: &[Cell],
    costs: &[Cell],
    quality: Option<&Quality>,
    cost: Option<&Cost>,
    verdict: Option<Verdict>,
    blocked: bool,
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
        settles: Vec::new(),
        note: None,
    };
    let (Some(quality), Some(Verdict::CannotTell)) = (quality, verdict) else {
        return repetitions;
    };
    let mut note = |text: &str| {
        repetitions.note = Some(text.to_owned());
    };
    if blocked {
        note("A test fell beyond the margin on its own: more runs do not lift a clear fall.");
        return repetitions;
    }
    let n0 = have.a.min(have.b);
    let mut plans = Vec::<(&str, Option<usize>)>::new();
    let improved = quality.state == QualityState::Improved;
    let mut inferior = false;
    if matches!(
        quality.state,
        QualityState::Fell | QualityState::Inconclusive
    ) {
        let distance = rule.margin + quality.delta;
        inferior = distance < 0.0;
        let pools = pools(scores, |v| v + rule.margin);
        let k = scores.len() as f64;
        plans.push((
            "quality",
            plan(distance.abs(), &pools, k, n0, Z_ALPHA, rule.alpha),
        ));
    }
    // Quality pointing to inferior reads worse whatever the cost.
    if !inferior {
        let Some(cost) = cost else {
            note("No subject cost on both sides: the cost cannot settle the verdict.");
            return repetitions;
        };
        let trade_off = "Quality improved and the cost rose or may rise: more runs do not settle \
                         a trade-off.";
        match cost.state {
            CostState::MoreExpensive if improved => {
                note(trade_off);
                return repetitions;
            }
            CostState::Inconclusive if cost.relative < rule.equivalence => {
                let scale = 1.0 + rule.equivalence;
                plans.push((
                    "cost",
                    plan(
                        cost.a - cost.b / scale,
                        &pools(costs, |v| v / scale),
                        1.0,
                        n0,
                        Z_ALPHA,
                        rule.alpha,
                    ),
                ));
            }
            CostState::Inconclusive if improved => {
                note(trade_off);
                return repetitions;
            }
            CostState::Inconclusive => plans.push((
                "cost",
                plan(
                    cost.b - cost.a,
                    &pools(costs, |v| v),
                    1.0,
                    n0,
                    Z_HALF_ALPHA,
                    rule.alpha / 2.0,
                ),
            )),
            _ => {}
        }
    }
    repetitions.settles = plans.iter().map(|(what, _)| (*what).to_owned()).collect();
    match plans.iter().map(|(_, n)| *n).collect::<Option<Vec<_>>>() {
        Some(needed) => {
            if let Some(needed) = needed.into_iter().max() {
                repetitions.needed = Some(needed);
                repetitions.missing = Some(needed - n0);
            }
        }
        None => repetitions.more_than = Some(REPETITIONS_CAP),
    }
    repetitions
}

/// An arm that mixes models or stacks, and sides that differ in more than
/// the Harness.
fn configuration_warnings(a: &[Run], b: &[Run]) -> Vec<String> {
    fn models(runs: &[Run]) -> BTreeSet<&str> {
        runs.iter().filter_map(|run| run.model.as_deref()).collect()
    }
    fn stacks(runs: &[Run]) -> Vec<&BTreeMap<String, String>> {
        let set = runs
            .iter()
            .map(|run| &run.stack)
            .filter(|stack| !stack.is_empty())
            .collect::<BTreeSet<_>>();
        set.into_iter().collect()
    }
    /// Each entry the stacks do not agree on, with its values in order.
    fn differences(stacks: &[&BTreeMap<String, String>], skip: &[&str]) -> Vec<String> {
        let names = stacks
            .iter()
            .flat_map(|stack| stack.keys())
            .filter(|name| !skip.contains(&name.as_str()))
            .collect::<BTreeSet<_>>();
        names
            .into_iter()
            .filter_map(|name| {
                let mut values = Vec::new();
                for stack in stacks {
                    let value = stack.get(name).map_or("none", String::as_str);
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
                (values.len() > 1).then(|| format!("{name} {}", values.join(" / ")))
            })
            .collect()
    }
    let mut warnings = Vec::new();
    for (side, runs) in [("A", a), ("B", b)] {
        let models = models(runs);
        if models.len() > 1 {
            warnings.push(format!(
                "{side} mixes models: {}.",
                models.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        let stacks = stacks(runs);
        if stacks.len() > 1 {
            warnings.push(format!(
                "{side} mixes {} stacks: {}.",
                stacks.len(),
                differences(&stacks, &[]).join(", ")
            ));
        }
    }
    let (models_a, models_b) = (models(a), models(b));
    if models_a.len() == 1 && models_b.len() == 1 && models_a != models_b {
        warnings.push(format!(
            "The sides ran different models (A / B): {} / {}.",
            models_a.first().unwrap(),
            models_b.first().unwrap()
        ));
    }
    if let ([stack_a], [stack_b]) = (stacks(a).as_slice(), stacks(b).as_slice()) {
        let differ = differences(&[stack_a, stack_b], &["harness", "lock"]);
        if !differ.is_empty() {
            warnings.push(format!(
                "The sides' stacks differ in more than the Harness (A / B): {}.",
                differ.join(", ")
            ));
        }
    }
    warnings
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
    let keys = in_a
        .iter()
        .chain(&in_b)
        .flat_map(|(scenario, behaviors)| {
            behaviors.iter().map(move |behavior| (*scenario, *behavior))
        })
        .collect::<BTreeSet<_>>();
    let ran = |side: &BTreeMap<&str, BTreeSet<Option<&str>>>,
               (scenario, behavior): (&str, Option<&str>)| {
        side.get(scenario)
            .is_some_and(|behaviors| behaviors.contains(&behavior))
    };
    keys.into_iter()
        .filter_map(|key| {
            let owned = (key.0.to_owned(), key.1.map(str::to_owned));
            let reason = match (ran(&in_a, key), ran(&in_b, key)) {
                (true, true) if !cells_a.contains_key(&owned) => "no_counted_run_in_a",
                (true, true) if !cells_b.contains_key(&owned) => "no_counted_run_in_b",
                (true, true) => return None,
                (true, false) if in_b.contains_key(key.0) => "definition_changed",
                (false, true) if in_a.contains_key(key.0) => "definition_changed",
                (true, false) => "only_in_a",
                _ => "only_in_b",
            };
            Some(Dropped {
                scenario: owned.0,
                behavior: owned.1,
                reason: reason.to_owned(),
            })
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
            QualityState::Improved => {
                format!(
                    "Quality {delta:+.1} points: improved ({}).",
                    p(q.p_improved)
                )
            }
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
            CostState::BelowBound => format!(
                "Cost {change:+.1}%: below the +{within}% bound ({}), but neither within \
                 −{within}% ({}) nor cheaper ({}).",
                p(c.p_within_high),
                p(c.p_within_low),
                p(c.p_cheaper)
            ),
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
        reasons.push("Cost: no test has a subject cost above 0 on both sides.".into());
    }
    reasons
}

/// "the quality at the 5-point margin and the cost at the +5% bound".
fn settles_text(settles: &[String], rule: &Rule) -> String {
    let parts = settles
        .iter()
        .map(|what| match what.as_str() {
            "quality" => format!("the quality at the {}-point margin", rule.margin),
            _ => "the cost".to_owned(),
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "it".to_owned()
    } else {
        parts.join(" and ")
    }
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
            match comparison.qualifier {
                Some(Qualifier::TestInferior) => " (a test fell beyond the margin on its own)",
                Some(Qualifier::QualityFell) =>
                    " (quality fell, by an amount the runs cannot bound)",
                Some(Qualifier::TradeOff) => {
                    " (quality improved and the cost rose: a trade-off the rule does not weigh)"
                }
                None => "",
            }
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
    let settles = settles_text(&r.settles, rule);
    match (r.needed, r.missing, r.more_than, &r.note) {
        (Some(needed), Some(missing), _, _) => {
            repetitions += &format!(
                "; about {needed} per arm, those made kept, would settle {settles} with a \
                 predicted 80%: {missing} more"
            )
        }
        (_, _, Some(cap), _) => {
            repetitions += &format!("; more than {cap} per arm would be needed to settle {settles}")
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
                "  fell beyond the margin on its own"
            } else {
                ""
            }
        ));
    }
    for dropped in &comparison.dropped {
        line(format!(
            "  {}{} left out: {}",
            dropped.scenario,
            dropped
                .behavior
                .as_deref()
                .map_or(String::new(), |behavior| format!(
                    " ({})",
                    behavior.get(..15).unwrap_or(behavior)
                )),
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
        "Rule: permutation tests within each test, one-sided α {} each (improved, fell, cheaper \
         and more expensive at α / 2; each test's fall beyond the margin Holm-adjusted), margin \
         {} points, cost within ±{}%, {} permutations, seed {}.\n",
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
        runs_of([&document]).0
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::fixture;
    use super::*;

    fn expected() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/verdict/expected.json")).unwrap()
    }

    /// A counted, evaluated run of `scenario` (its own definition).
    fn run(attempt: &str, scenario: &str, score: f64, cost: f64) -> Run {
        Run {
            attempt_id: attempt.into(),
            scenario: scenario.into(),
            behavior: Some(format!("sha256:{scenario}")),
            cause: Some("evaluated".into()),
            counted: true,
            score: Some(score),
            completed: true,
            cost_usd: Some(cost),
            ..Run::default()
        }
    }

    // Worst first, as the header orders them.
    fn verdict_rank(comparison: &Comparison) -> u8 {
        match comparison.verdict.unwrap() {
            Verdict::Worse => 0,
            Verdict::CannotTell => 1,
            Verdict::NoDifference => 2,
            Verdict::Better => 3,
        }
    }

    fn quality_rank(comparison: &Comparison) -> u8 {
        match comparison.quality.as_ref().unwrap().state {
            QualityState::Inferior => 0,
            QualityState::Fell => 1,
            QualityState::Inconclusive => 2,
            QualityState::NonInferior => 3,
            QualityState::Improved => 4,
        }
    }

    fn cost_rank(comparison: &Comparison) -> u8 {
        match comparison.cost.as_ref().unwrap().state {
            CostState::MoreExpensive => 0,
            CostState::Inconclusive => 1,
            CostState::Equivalent => 2,
            CostState::BelowBound => 3,
            CostState::Cheaper => 4,
        }
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
            assert_eq!(
                serde_json::to_value(comparison.qualifier).unwrap(),
                expected["qualifier"],
                "{suite}"
            );
            // One model, one stack per arm, only the Harness differs, and no
            // run is left out or doubted.
            assert!(
                comparison.warnings.is_empty(),
                "{suite}: {:?}",
                comparison.warnings
            );
            // How many more repetitions, never none while it cannot tell.
            let r = &comparison.repetitions;
            assert_eq!(
                serde_json::json!({"needed": r.needed, "missing": r.missing,
                    "more_than": r.more_than, "settles": r.settles}),
                expected["repetitions"],
                "{suite}"
            );
        }
        // The run the subject's own stack broke is a counted zero.
        let b = fixture("se-1.8.50.json");
        let se = compare(&fixture("se-1.8.44.json"), &b, &rule);
        assert_eq!(se.runs.b.causes.get("evaluated"), Some(&30));
        assert_eq!(se.runs.b.counted, 30);
        let quality = se.quality.as_ref().unwrap();
        assert!((quality.delta + 6.99).abs() < 0.01, "{}", quality.delta);
        assert!(se.cost.as_ref().unwrap().relative < -0.05);
        // The Kanban board fell 16 points, more than the margin, but five runs
        // a side do not show it beyond the margin once the tests are many.
        let board = se
            .tests
            .iter()
            .find(|test| test.scenario == "kanban_c3_board")
            .unwrap();
        assert!(board.delta < -rule.margin && !board.warning);
        assert!(board.p_inferior_adjusted >= board.p_inferior);
    }

    /// Both arms drawn with replacement from one arm's counted runs, test by
    /// test, `n` runs each: the same version against itself.
    fn same_version(tests: &[Vec<Run>], n: usize, rng: &mut SplitMix64) -> [Vec<Run>; 2] {
        ["a", "b"].map(|side| {
            tests
                .iter()
                .flat_map(|runs| {
                    (0..n)
                        .map(|i| Run {
                            attempt_id: format!("{side}{i:02}"),
                            ..runs[(rng.next_u64() % runs.len() as u64) as usize].clone()
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        })
    }

    /// Each arm of the pair's counted runs, by test.
    fn arms_by_test() -> [Vec<Vec<Run>>; 4] {
        [
            "se-1.8.44.json",
            "se-1.8.50.json",
            "regression-1.8.44.json",
            "regression-1.8.50.json",
        ]
        .map(|arm| {
            let mut by = BTreeMap::<String, Vec<Run>>::new();
            for run in fixture(arm).into_iter().filter(|run| run.counted) {
                by.entry(run.scenario.clone()).or_default().push(run);
            }
            by.into_values().collect::<Vec<_>>()
        })
    }

    #[test]
    fn a_version_against_itself_gives_a_false_signal_in_fewer_than_one_comparison_in_twenty() {
        let calibration = &expected()["calibration"];
        let number = |key: &str| calibration[key].as_u64().unwrap() as usize;
        let arms = arms_by_test();
        let mut rng = SplitMix64(number("seed") as u64);
        let ceiling = calibration["max_false_signal_rate"].as_f64().unwrap();
        for n in calibration["runs_per_arm"].as_array().unwrap() {
            let n = n.as_u64().unwrap() as usize;
            let trials = number("trials_per_size");
            let mut false_signals = 0;
            for trial in 0..trials {
                let [a, b] = same_version(&arms[trial % arms.len()], n, &mut rng);
                let rule = Rule {
                    permutations: number("permutations"),
                    seed: trial as u64,
                    ..Rule::default()
                };
                let comparison = compare(&a, &b, &rule);
                false_signals += usize::from(
                    matches!(comparison.verdict, Some(Verdict::Better | Verdict::Worse))
                        || comparison.qualifier.is_some(),
                );
            }
            let rate = false_signals as f64 / trials as f64;
            assert!(
                rate < ceiling,
                "{n} runs per arm: {false_signals} of {trials} gave a false signal"
            );
        }
    }

    #[test]
    fn a_test_warns_only_when_its_own_fall_beyond_the_margin_survives_holm() {
        assert_eq!(holm(&[0.01, 0.04, 0.03, 0.005]), [0.03, 0.06, 0.06, 0.02]);
        // Ties keep their order and the adjustment never passes 1.
        assert_eq!(holm(&[0.6, 0.6, 0.125]), [1.0, 1.0, 0.375]);
        // The same version at nine runs: the observed fall of some test
        // passes the margin in most comparisons, its own test almost never.
        let arms = arms_by_test();
        let mut rng = SplitMix64(9);
        let rule = Rule {
            permutations: 2_000,
            ..Rule::default()
        };
        let (mut beyond, mut warned) = (0, 0);
        for trial in 0..24 {
            let [a, b] = same_version(&arms[trial % arms.len()], 9, &mut rng);
            let comparison = compare(&a, &b, &rule);
            beyond += usize::from(
                comparison
                    .tests
                    .iter()
                    .any(|test| test.delta < -rule.margin),
            );
            warned += usize::from(comparison.tests.iter().any(|test| test.warning));
        }
        assert!(beyond >= 8, "{beyond}");
        assert!(warned <= 1, "{warned}");
        // A clear fall of one test among many does warn, by name.
        let mut a = Vec::new();
        let mut b = Vec::new();
        for test in 0..14 {
            for i in 0..9 {
                let score = 80.0 + (i % 3) as f64;
                let fallen = if test == 3 { score - 30.0 } else { score };
                a.push(run(
                    &format!("a{test}-{i}"),
                    &format!("t{test:02}"),
                    score,
                    1.0,
                ));
                b.push(run(
                    &format!("b{test}-{i}"),
                    &format!("t{test:02}"),
                    fallen,
                    1.0,
                ));
            }
        }
        let comparison = compare(&a, &b, &rule);
        let warned = comparison
            .tests
            .iter()
            .filter(|test| test.warning)
            .map(|test| test.scenario.as_str())
            .collect::<Vec<_>>();
        assert_eq!(warned, ["t03"]);
        assert!(comparison
            .warnings
            .iter()
            .any(|w| w.starts_with("t03 fell 30.0 points, beyond the 5-point margin on its own")));
    }

    #[test]
    fn one_run_per_test_has_no_calibrated_verdict_and_dropped_definitions_say_why() {
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
            // Compared under s1, t1 also ran under another definition on B.
            run("b5", "t1", "s1b", 50.0),
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
                .map(|d| (
                    d.scenario.as_str(),
                    d.behavior.as_deref().unwrap(),
                    d.reason.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("t1", "s1b", "definition_changed"),
                ("t2", "other", "definition_changed"),
                ("t2", "s2", "definition_changed"),
                ("t3", "s3", "only_in_a"),
                ("t4", "s4", "only_in_b")
            ]
        );
        assert_eq!(comparison.runs.b.excluded, 1);
        assert_eq!(comparison.tests[0].delta, -20.0);
        // One run cannot show a test beyond the margin.
        assert!(!comparison.tests[0].warning);
        // Reports older than causes fall back to `technical`, and say so.
        let (older, _) = runs_of([
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

    /// Three tests, each with these scores and a cost a hair apart per run.
    fn arm(prefix: &str, scores: [f64; 5], cost: f64) -> Vec<Run> {
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
            .collect()
    }

    #[test]
    fn clear_changes_read_as_better_no_difference_and_worse() {
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
        // Clearly higher quality at the same cost is better.
        let higher = compare(&base, &arm("b", [96.0, 97.0, 98.0, 96.0, 97.0], 1.0), &rule);
        assert_eq!(
            (higher.quality.as_ref().unwrap().state, higher.verdict),
            (QualityState::Improved, Some(Verdict::Better))
        );
        // Five points down, every run: it fell, by about the margin itself,
        // which more runs cannot settle.
        let fell = compare(&base, &arm("b", [85.0, 86.0, 87.0, 85.0, 86.0], 0.5), &rule);
        assert_eq!(fell.verdict, Some(Verdict::CannotTell));
        assert_eq!(fell.qualifier, Some(Qualifier::QualityFell));
        assert_eq!(fell.repetitions.more_than, Some(REPETITIONS_CAP));
        assert_eq!(fell.repetitions.settles, ["quality"]);
        // Fallen, or undecided, and more expensive is worse.
        let fell = compare(&base, &arm("b", [85.0, 86.0, 87.0, 85.0, 86.0], 2.0), &rule);
        assert_eq!(fell.quality.unwrap().state, QualityState::Fell);
        assert_eq!((fell.verdict, fell.qualifier), (Some(Verdict::Worse), None));
        let noisy = [70.0, 100.0, 80.0, 95.0, 85.0];
        let unsure = compare(&base, &arm("b", noisy, 1.0), &rule);
        assert_eq!(unsure.verdict, Some(Verdict::CannotTell));
        assert!(unsure.repetitions.missing.is_some() || unsure.repetitions.more_than.is_some());
        assert_eq!(
            compare(&base, &arm("b", noisy, 2.0), &rule).verdict,
            Some(Verdict::Worse)
        );
        // Clearly higher quality for a higher cost is a trade-off, not worse.
        let dearer = compare(&base, &arm("b", [96.0, 97.0, 98.0, 96.0, 97.0], 2.0), &rule);
        assert_eq!(
            dearer.quality.as_ref().unwrap().state,
            QualityState::Improved
        );
        assert_eq!(
            (dearer.verdict, dearer.qualifier),
            (Some(Verdict::CannotTell), Some(Qualifier::TradeOff))
        );
        assert!(dearer.repetitions.note.unwrap().contains("trade-off"));
        assert!(dearer.reasons[0].contains("improved"));
    }

    #[test]
    fn a_test_beyond_the_margin_holds_back_better_however_many_steady_tests_dilute_it() {
        // One test 22 points down and 29% cheaper is worse; ten tests at 100
        // on both sides bring the mean within the margin, not to better.
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..9 {
            let wobble = (i % 3) as f64;
            a.push(run(
                &format!("a{i}"),
                "core",
                94.0 + wobble,
                1.0 + wobble / 100.0,
            ));
            b.push(run(
                &format!("b{i}"),
                "core",
                72.0 + wobble,
                0.71 + wobble / 100.0,
            ));
        }
        let rule = Rule::default();
        let alone = compare(&a, &b, &rule);
        assert_eq!(alone.verdict, Some(Verdict::Worse));
        for test in 0..10 {
            for i in 0..9 {
                let scenario = format!("steady{test:02}");
                a.push(run(&format!("a{test}-{i}"), &scenario, 100.0, 0.1));
                b.push(run(&format!("b{test}-{i}"), &scenario, 100.0, 0.1));
            }
        }
        let diluted = compare(&a, &b, &rule);
        let quality = diluted.quality.as_ref().unwrap();
        assert_eq!(
            quality.state,
            QualityState::NonInferior,
            "{}",
            quality.delta
        );
        assert_eq!(diluted.cost.as_ref().unwrap().state, CostState::Cheaper);
        assert_eq!(
            (diluted.verdict, diluted.qualifier),
            (Some(Verdict::CannotTell), Some(Qualifier::TestInferior))
        );
        assert!(diluted.repetitions.note.is_some());
        assert!(diluted
            .warnings
            .iter()
            .any(|w| w.starts_with("core fell 22.0 points, beyond the 5-point margin")));
    }

    /// Up to four tests with uneven runs per side, scores and costs drawn.
    fn uneven(rng: &mut SplitMix64, dearer: f64) -> (Vec<Run>, Vec<Run>) {
        let mut draw = |low: u64, span: u64| (low + rng.next_u64() % span) as f64;
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for test in 0..1 + draw(0, 4) as usize {
            let scenario = format!("t{test}");
            let base = draw(5, 95) / 100.0;
            for (side, runs, shift, price) in [("a", &mut a, 0.0, 1.0), ("b", &mut b, 4.0, dearer)]
            {
                for i in 0..draw(3, 8) as usize {
                    let cost = base * price * draw(40, 120) / 100.0;
                    runs.push(run(
                        &format!("{side}{test}{i}"),
                        &scenario,
                        draw(78, 15) - shift,
                        cost,
                    ));
                }
            }
        }
        (a, b)
    }

    #[test]
    fn raising_a_b_score_or_lowering_a_b_cost_never_makes_a_p_a_state_or_the_verdict_worse() {
        let rule = Rule {
            permutations: 300,
            ..Rule::default()
        };
        let mut rng = SplitMix64(11);
        let mut moved = 0;
        for case in 0..40 {
            let dearer = [1.0, 1.3, 1.6][case % 3];
            let (a, mut b) = uneven(&mut rng, dearer);
            let mut before = compare(&a, &b, &rule);
            for step in 0..16 {
                let at = (rng.next_u64() % b.len() as u64) as usize;
                let raise = step % 2 == 0;
                if raise {
                    *b[at].score.as_mut().unwrap() += (1 + rng.next_u64() % 12) as f64;
                } else {
                    *b[at].cost_usd.as_mut().unwrap() *= [0.95, 0.7, 0.4][step % 3];
                }
                let after = compare(&a, &b, &rule);
                let what = format!(
                    "case {case} step {step}: {:?} then {:?}",
                    before.reasons, after.reasons
                );
                assert!(verdict_rank(&after) >= verdict_rank(&before), "{what}");
                let (q0, q1) = (
                    before.quality.as_ref().unwrap(),
                    after.quality.as_ref().unwrap(),
                );
                let (c0, c1) = (before.cost.as_ref().unwrap(), after.cost.as_ref().unwrap());
                if raise {
                    assert!(quality_rank(&after) >= quality_rank(&before), "{what}");
                    assert!(
                        q1.p_improved <= q0.p_improved && q1.p_non_inferior <= q0.p_non_inferior
                    );
                    assert!(q1.p_inferior >= q0.p_inferior && q1.p_fell >= q0.p_fell);
                    assert_eq!(
                        (c0.p_cheaper, c0.p_more_expensive),
                        (c1.p_cheaper, c1.p_more_expensive)
                    );
                } else {
                    assert!(cost_rank(&after) >= cost_rank(&before), "{what}");
                    assert!(
                        c1.p_cheaper <= c0.p_cheaper && c1.p_within_high <= c0.p_within_high,
                        "{what}"
                    );
                    assert!(
                        c1.p_more_expensive >= c0.p_more_expensive
                            && c1.p_within_low >= c0.p_within_low,
                        "{what}"
                    );
                    assert_eq!(q0, q1);
                }
                moved += usize::from(after.verdict != before.verdict);
                before = after;
            }
        }
        // The steps did move verdicts, so the property was put to the test.
        assert!(moved >= 10, "{moved}");
    }

    #[test]
    fn the_cost_reads_the_same_in_any_unit() {
        let mut rng = SplitMix64(5);
        let rule = Rule {
            permutations: 500,
            ..Rule::default()
        };
        for _ in 0..6 {
            let (a, b) = uneven(&mut rng, 1.05);
            let base = compare(&a, &b, &rule).cost.unwrap();
            for unit in [1e-9, 3.7, 1e9] {
                let scaled = |runs: &[Run]| {
                    runs.iter()
                        .map(|run| Run {
                            cost_usd: run.cost_usd.map(|cost| cost * unit),
                            ..run.clone()
                        })
                        .collect::<Vec<_>>()
                };
                let cost = compare(&scaled(&a), &scaled(&b), &rule).cost.unwrap();
                assert_eq!(
                    (cost.state, cost.p_cheaper, cost.p_more_expensive),
                    (base.state, base.p_cheaper, base.p_more_expensive),
                    "×{unit}"
                );
                assert_eq!(
                    (cost.p_within_low, cost.p_within_high),
                    (base.p_within_low, base.p_within_high),
                    "×{unit}"
                );
            }
        }
    }

    #[test]
    fn the_cost_state_and_the_verdict_move_one_way_as_every_b_cost_falls() {
        // Ten tests, nine runs a side, the same scores and a cost noise of
        // about 15%: B's costs scaled from 30% dearer to 30% cheaper.
        let mut rng = SplitMix64(3);
        let mut noise = || 0.85 + (rng.next_u64() % 300) as f64 / 1_000.0;
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for test in 0..10 {
            let scenario = format!("t{test}");
            for i in 0..9 {
                let score = 80.0 + (i % 4) as f64;
                a.push(run(&format!("a{test}{i}"), &scenario, score, 0.1 * noise()));
                b.push(run(&format!("b{test}{i}"), &scenario, score, 0.1 * noise()));
            }
        }
        let rule = Rule {
            permutations: 400,
            ..Rule::default()
        };
        let mut seen = BTreeSet::new();
        let mut last = (0, 0);
        for step in 0..=60 {
            let scale = 1.3 - step as f64 * 0.01;
            let scaled = b
                .iter()
                .map(|run| Run {
                    cost_usd: run.cost_usd.map(|cost| cost * scale),
                    ..run.clone()
                })
                .collect::<Vec<_>>();
            let comparison = compare(&a, &scaled, &rule);
            let now = (cost_rank(&comparison), verdict_rank(&comparison));
            assert!(
                now.0 >= last.0 && now.1 >= last.1,
                "at ×{scale:.2}: {:?}",
                comparison.reasons
            );
            seen.insert(now.0);
            last = now;
        }
        // From more expensive to cheaper, through equivalent and the bound.
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn each_run_left_out_or_doubted_is_counted_and_named() {
        let document = |runs: Value, extra: Value| {
            serde_json::json!({"subject": {"provider": "deepseek", "model": "flash"},
                "system_under_test": {"stack": {"stack_versions": {"harness": "1.8.50"}}},
                "scenarios": [{"scenario_id": "t0", "behavior_sha256": "sha256:t0", "runs": runs},
                              {"scenario_id": "t1", "behavior_sha256": "sha256:t1", "runs": extra}]})
        };
        let runs = |side: &str, scores: [Value; 4], costs: [Value; 4]| {
            Value::Array(
                scores
                    .into_iter()
                    .zip(costs)
                    .enumerate()
                    .map(|(i, (score, cost))| {
                        serde_json::json!({"attempt_id": format!("{side}{i}"), "cause": "evaluated",
                            "score": score, "cost": {"subject_usd": cost}})
                    })
                    .collect(),
            )
        };
        let a = document(
            runs(
                "a",
                [80.into(), 82.into(), 81.into(), 83.into()],
                [0.10.into(), 0.11.into(), 0.12.into(), 0.10.into()],
            ),
            // t1 reuses t0's attempt ids: both tests stay.
            runs(
                "a",
                [70.into(), 71.into(), 72.into(), 70.into()],
                [0.2.into(), 0.21.into(), 0.2.into(), 0.22.into()],
            ),
        );
        let mut conflict = a.clone();
        conflict["scenarios"][0]["runs"][0]["score"] = 0.into();
        let (a, doubts) = runs_of([&a, &conflict]);
        assert_eq!(a.len(), 8);
        assert_eq!(a[0].score, Some(80.0));
        assert_eq!(
            doubts,
            [
                "1 runs repeat an attempt already read for the same test, with other content; \
                 the first read is kept: t0 a0.",
                "4 attempt ids appear under more than one test, kept under each: a0 (t0, t1), \
                 a1 (t0, t1), a2 (t0, t1), a3 (t0, t1)."
            ]
        );
        let (mut b, doubts) = runs_of([&document(
            runs(
                "b",
                [Value::Null, "81".into(), 80.into(), 82.into()],
                [0.1.into(), Value::Null, (-0.5).into(), 0.0.into()],
            ),
            runs(
                "c",
                [70.into(), 72.into(), 71.into(), 70.into()],
                [Value::Null, Value::Null, Value::Null, Value::Null],
            ),
        )]);
        assert!(doubts.is_empty());
        b[4].cause = Some("subject_crashed_new_kind".into());
        let comparison = compare(
            &a,
            &b,
            &Rule {
                permutations: 200,
                ..Rule::default()
            },
        );
        for warning in [
            "B: 2 counted runs have no numeric score and leave the quality: t0 ×2.",
            "B: 5 counted runs have no subject cost and leave the cost: t0 ×1, t1 ×4.",
            "B: 2 counted runs have a subject cost at or below 0 and leave the cost: t0 ×2.",
            "B: 1 runs carry a cause this version does not know (subject_crashed_new_kind) and \
             were classified by `technical`: t1 ×1.",
            "The cost is read over 1 of the 2 tests, without t1: no subject cost above 0 on a side.",
        ] {
            assert!(comparison.warnings.contains(&warning.to_owned()), "{warning}\n{:#?}", comparison.warnings);
        }
        // The cost reads t0's one run left with a cost above 0.
        let cost = comparison.cost.as_ref().unwrap();
        assert_eq!((cost.tests, cost.b), (1, 0.1));
        // Sums too large to add read p = 1, never equivalent.
        let huge = |prefix: &str| {
            (0..4)
                .map(|i| run(&format!("{prefix}{i}"), "t", 80.0 + i as f64, 1.5e308))
                .collect::<Vec<_>>()
        };
        let comparison = compare(
            &huge("a"),
            &huge("b"),
            &Rule {
                permutations: 200,
                ..Rule::default()
            },
        );
        let cost = comparison.cost.as_ref().unwrap();
        assert_eq!(cost.state, CostState::Inconclusive);
        assert_eq!(
            (cost.p_within_low, cost.p_within_high, cost.p_cheaper),
            (1.0, 1.0, 1.0)
        );
        assert!(comparison.warnings.contains(
            &"The cost does not add up to a finite number: each of its tests reads p = 1."
                .to_owned()
        ));
    }

    #[test]
    fn the_repetitions_settle_what_is_undecided_with_a_predicted_80_percent() {
        // Nine tests at 100 on both sides and one that falls 100 → 60, cost
        // −20%: no noise within an arm. At five runs its own test cannot show
        // the fall beyond the margin over ten tests, and the mean cannot tell
        // within 30 runs; from ten, the test is clearly inferior.
        let ten = |prefix: &str, n: usize, fallen: f64, cost: f64| {
            (0..10)
                .flat_map(|test| (0..n).map(move |i| (test, i)))
                .map(|(test, i)| {
                    let score = if test == 9 { fallen } else { 100.0 };
                    run(
                        &format!("{prefix}{test}{i:02}"),
                        &format!("t{test}"),
                        score,
                        cost * (1.0 + i as f64 * 0.001),
                    )
                })
                .collect::<Vec<_>>()
        };
        let rule = Rule {
            permutations: 2_000,
            ..Rule::default()
        };
        let five = compare(&ten("a", 5, 100.0, 1.0), &ten("b", 5, 60.0, 0.8), &rule);
        assert_eq!(five.verdict, Some(Verdict::CannotTell));
        let r = &five.repetitions;
        assert_eq!((r.needed, r.missing, r.more_than), (None, None, Some(30)));
        for n in [10, 20] {
            let comparison = compare(&ten("a", n, 100.0, 1.0), &ten("b", n, 60.0, 0.8), &rule);
            assert_eq!(comparison.qualifier, Some(Qualifier::TestInferior), "{n}");
            assert!(comparison.repetitions.note.is_some(), "{n}");
        }
        // Beyond the margin, the runs that would show it inferior.
        let a = [85.0, 95.0, 90.0, 82.0, 98.0];
        let below = compare(
            &arm("a", a, 1.0),
            &arm("b", [87.0, 77.0, 90.0, 82.0, 74.0], 1.0),
            &Rule::default(),
        );
        let quality = below.quality.as_ref().unwrap();
        assert!(quality.delta < -5.0, "{}", quality.delta);
        assert_eq!(below.verdict, Some(Verdict::CannotTell));
        let r = &below.repetitions;
        assert!(r.note.is_none(), "{:?}", r.note);
        assert!(r.missing.is_some_and(|missing| missing > 0) || r.more_than.is_some());
        // Quality within the margin and a cost that could be 10% dearer: the
        // runs that would show it below the +5% bound.
        let wobble = |prefix: &str, price: f64| {
            (0..3)
                .flat_map(|test| (0..9).map(move |i| (test, i)))
                .map(|(test, i)| {
                    let cost = price * [0.85, 1.0, 1.15][i % 3];
                    run(
                        &format!("{prefix}{test}{i}"),
                        &format!("t{test}"),
                        90.0 + (i % 2) as f64,
                        cost,
                    )
                })
                .collect::<Vec<_>>()
        };
        let undecided = compare(&wobble("a", 1.0), &wobble("b", 0.98), &Rule::default());
        assert_eq!(
            (
                undecided.quality.as_ref().unwrap().state,
                undecided.cost.as_ref().unwrap().state
            ),
            (QualityState::NonInferior, CostState::Inconclusive)
        );
        let r = &undecided.repetitions;
        assert_eq!(r.settles, ["cost"]);
        let needed = r.needed.unwrap();
        assert!(needed > 9 && needed <= REPETITIONS_CAP && r.missing == Some(needed - 9));
        // The plan at that many runs: the bound is shown.
        let settled = compare(
            &(0..needed.div_ceil(9))
                .flat_map(|copy| wobble(&format!("a{copy}-"), 1.0))
                .collect::<Vec<_>>(),
            &(0..needed.div_ceil(9))
                .flat_map(|copy| wobble(&format!("b{copy}-"), 0.98))
                .collect::<Vec<_>>(),
            &Rule::default(),
        );
        assert_ne!(
            settled.verdict,
            Some(Verdict::CannotTell),
            "{:?}",
            settled.reasons
        );
        // No permutation p reaches α = 0.025 below 4 runs per arm with one
        // test, 3 with two, 2 with three.
        assert_eq!(
            [1, 2, 3].map(|tests| fewest_runs(tests, 0.025)),
            [Some(4), Some(3), Some(2)]
        );
    }

    #[test]
    fn an_arm_that_mixes_models_or_stacks_and_sides_that_differ_beyond_the_harness_warn() {
        let document = |model: &str, harness: &str, router: &str| {
            serde_json::json!({
                "subject": {"provider": "deepseek", "model": model},
                "system_under_test": {"engine_version": "0.24.6", "stack": {
                    "stack_versions": {"harness": harness, "llm-router": router},
                    "stack_lock_digest": format!("sha256:{harness}{router}")}},
                "scenarios": [{"scenario_id": "t", "behavior_sha256": "sha256:t", "runs": [
                    {"attempt_id": format!("{model}{harness}{router}"), "cause": "evaluated",
                     "score": 90, "cost": {"subject_usd": 1.0}}]}]
            })
        };
        let a = runs_of([&document("flash", "1.8.44", "1.4.36")]).0;
        let b = runs_of([&document("flash", "1.8.50", "1.4.36")]).0;
        assert_eq!(a[0].model.as_deref(), Some("deepseek/flash"));
        assert_eq!(a[0].stack["iii"], "0.24.6");
        // Only the Harness differs: nothing to say.
        assert!(compare(&a, &b, &Rule::default()).warnings.is_empty());
        let mixed = runs_of([
            &document("flash", "1.8.50", "1.4.36"),
            &document("pro", "1.8.50", "1.4.37"),
        ])
        .0;
        assert_eq!(
            compare(&a, &mixed, &Rule::default()).warnings,
            [
                "B mixes models: deepseek/flash, deepseek/pro.",
                "B mixes 2 stacks: llm-router 1.4.36 / 1.4.37, lock sha256:1.8.501.4.36 / \
                 sha256:1.8.501.4.37."
            ]
        );
        let other = runs_of([&document("pro", "1.8.50", "1.4.37")]).0;
        assert_eq!(
            compare(&a, &other, &Rule::default()).warnings,
            [
                "The sides ran different models (A / B): deepseek/flash / deepseek/pro.",
                "The sides' stacks differ in more than the Harness (A / B): llm-router 1.4.36 / \
                 1.4.37."
            ]
        );
    }
}
