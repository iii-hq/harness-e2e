//! One reviewed source for test-plan templates.
//! Materialization is pure: it never contacts iii or calls a model.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::artifact;
use crate::control::{scenarios_list, ScenarioDescriptor, ScenariosListRequest};
use crate::scenarios::tags::{Area, Discipline, ScenarioTags, Worker};
use crate::scenarios::{ScenarioExecutionKind, ScenarioId};

const SOURCE: &str = include_str!("../config/test-plan.json");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MasterPlan {
    pub schema: String,
    pub plan_id: String,
    pub modules: Vec<CapabilityModule>,
    pub diagnostics: Vec<String>,
    pub requirements: BTreeMap<String, Vec<String>>,
    /// Serialized under its former name: the plan's digest
    /// (`definition_sha256`, and through it every `profile_sha256`) is the
    /// cohort key Release Control groups executions by, so a rename must not
    /// move it.
    #[serde(rename = "profiles", alias = "suites")]
    pub suites: Vec<Suite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityModule {
    pub id: String,
    pub label: String,
    pub scenarios: Vec<String>,
}

/// What a campaign tests: its scenarios and how often. Where it runs (the
/// stack) and with whom (model, agent profile) are named per execution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub scenarios: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenario_groups: Vec<Vec<String>>,
    #[serde(default = "one_repetition")]
    pub repetitions: u32,
    #[serde(default)]
    pub technical_retries: u8,
    /// Internal: the admission budget, always `local-<id>`.
    #[serde(default)]
    pub lane: String,
    /// Selection by tag, for a suite a dispatch states (deploy impact, a
    /// workers PR): the `pr` suite plus every scheduled test carrying one of
    /// these tags. Names, not the vocabulary itself: one the tags do not know
    /// warns under `impact.uncovered` instead of failing the suite. Never
    /// serialized, so never in a digest or in the snapshot's schema: the
    /// snapshot records them under `impact`, and the plan may not declare them.
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub workers: Vec<String>,
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub areas: Vec<String>,
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub disciplines: Vec<String>,
}

impl Suite {
    fn tags(&self) -> TagSet {
        TagSet {
            workers: self.workers.clone(),
            areas: self.areas.clone(),
            disciplines: self.disciplines.clone(),
        }
    }
}

fn one_repetition() -> u32 {
    1
}

/// Tag names by dimension, as a dispatch states them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TagSet {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disciplines: Vec<String>,
}

impl TagSet {
    /// Each name with its dimension: `("worker", "state")`.
    fn entries(&self) -> Vec<(&'static str, &str)> {
        let workers = self.workers.iter().map(|name| ("worker", name.as_str()));
        let areas = self.areas.iter().map(|name| ("area", name.as_str()));
        let disciplines = (self.disciplines.iter()).map(|name| ("discipline", name.as_str()));
        workers.chain(areas).chain(disciplines).collect()
    }
}

/// A test's tags, each with its dimension.
fn tag_entries(tags: &ScenarioTags) -> BTreeSet<(&'static str, &'static str)> {
    let workers = tags
        .workers
        .iter()
        .map(|worker| ("worker", worker.as_str()));
    let area = tags.area.map(|area| ("area", area.as_str()));
    let disciplines =
        (tags.disciplines.iter()).map(|discipline| ("discipline", discipline.as_str()));
    workers.chain(area).chain(disciplines).collect()
}

/// Every tag a selection can name.
fn vocabulary() -> BTreeSet<(&'static str, &'static str)> {
    let workers = Worker::ALL.iter().map(|worker| ("worker", worker.as_str()));
    let areas = Area::ALL.iter().map(|area| ("area", area.as_str()));
    let disciplines =
        (Discipline::ALL.iter()).map(|discipline| ("discipline", discipline.as_str()));
    workers.chain(areas).chain(disciplines).collect()
}

/// What a suite selecting by tag asked for and what it found among the
/// scheduled tests.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Impact {
    pub requested: TagSet,
    /// Scheduled tests carrying a requested tag, run on top of `pr`.
    pub matched: Vec<String>,
    /// Tests carrying a requested tag that a selection by tag leaves out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
    /// Requested tags no selected test carries, each with why: nothing in
    /// this suite protects them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered: Vec<UncoveredTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UncoveredTag {
    /// `worker`, `area` or `discipline`.
    pub dimension: String,
    pub name: String,
    pub reason: String,
}

/// Left out of every selection by tag: the registry pair runs 17 to 34 min
/// a test, and Linkly, the onboarding tour and the memory tests need a stack
/// of their own.
// ponytail: a fixed list; a ceiling from run durations belongs where the
// durations are (Release Control's deploy check).
const IMPACT_EXCLUDED: &[ScenarioId] = &[
    ScenarioId::RegistryImplementation,
    ScenarioId::RegistryVerification,
    ScenarioId::LinklyTutorial,
    ScenarioId::OnboardingTour,
    ScenarioId::MemoryRelevantRecall,
    ScenarioId::MemorySmallTalk,
    ScenarioId::MemoryFirstPersonOffTopic,
    ScenarioId::MemoryParaphrase,
    ScenarioId::MemorySessionRepeat,
    ScenarioId::MemoryUpdatedFact,
    ScenarioId::MemoryValueProject,
    ScenarioId::MemoryValueProjectBaseline,
    ScenarioId::MemoryValuePreference,
    ScenarioId::MemoryValuePreferenceBaseline,
    ScenarioId::MemoryRuleFit,
    ScenarioId::MemoryChatterAfterAnswer,
    ScenarioId::MemoryUserFactNeeded,
    ScenarioId::MemoryAboutMe,
    ScenarioId::MemoryRewordedAcrossTurns,
    ScenarioId::MemorySiblingFacts,
    ScenarioId::MemoryRuleChangedMidSession,
    ScenarioId::MemoryBankEditsMidSession,
    ScenarioId::MemoryBankTrashedMidSession,
    ScenarioId::MemoryLearnThenRecall,
    ScenarioId::MemoryLearnRuleThenObey,
    ScenarioId::MemoryLearnCorrection,
    ScenarioId::MemoryAutomationNotLearned,
    ScenarioId::MemoryManySiblings,
    ScenarioId::MemoryAboutMeOverflow,
    ScenarioId::MemoryLinkedUpdate,
    ScenarioId::MemoryLearnFromLongMessage,
    ScenarioId::MemoryHonorsDoNotKeep,
    ScenarioId::MemoryKeepsWithoutARule,
];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileSnapshot {
    pub schema: String,
    pub plan_id: String,
    pub definition_sha256: String,
    pub profile_sha256: String,
    pub profile: Suite,
    pub scenario_ids: Vec<String>,
    pub cases: Vec<Value>,
    pub campaigns: Vec<Value>,
    pub budget: Value,
    pub interpretation: String,
    /// Present when the suite selects by tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<Impact>,
}

pub fn embedded() -> Result<MasterPlan> {
    let plan: MasterPlan = serde_json::from_str(SOURCE).context("decode master test plan")?;
    plan.validate()?;
    Ok(plan)
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

pub fn execution_kind(key: &ScenarioId) -> &'static str {
    match key.execution_kind() {
        ScenarioExecutionKind::HarnessTurn => "harness_turn",
        ScenarioExecutionKind::ScriptedDialogue => "scripted_dialogue",
        ScenarioExecutionKind::CompositeFlow => "composite_flow",
        ScenarioExecutionKind::AdaptiveFlow => "adaptive_flow",
    }
}

fn native_catalog(seed: Option<u64>) -> Result<BTreeMap<String, ScenarioDescriptor>> {
    Ok(scenarios_list(ScenariosListRequest { seed })?
        .scenarios
        .into_iter()
        .map(|entry| (entry.scenario_id.to_string(), entry))
        .collect())
}

impl MasterPlan {
    pub fn digest(&self) -> Result<String> {
        artifact::sha256_value(self)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != "harness-e2e-master-test-plan" {
            tracing::warn!(
                schema = %self.schema,
                "master test plan carries another schema id; read as it is"
            );
        }
        ensure!(safe_id(&self.plan_id), "invalid master plan identity");
        let mut covered = BTreeSet::new();
        let mut modules = BTreeSet::new();
        for module in &self.modules {
            ensure!(
                safe_id(&module.id) && modules.insert(module.id.as_str()),
                "invalid or repeated module {}",
                module.id
            );
            ensure!(!module.scenarios.is_empty(), "empty module {}", module.id);
            for id in &module.scenarios {
                ensure!(
                    covered.insert(id.clone()),
                    "scenario {id} belongs to multiple modules"
                );
            }
        }
        for id in &self.diagnostics {
            ensure!(covered.insert(id.clone()), "duplicate diagnostic {id}");
        }
        let native: BTreeSet<_> = ScenarioId::ALL.iter().map(ToString::to_string).collect();
        ensure!(
            covered == native,
            "master plan coverage differs from native catalog: missing {:?}, unknown {:?}",
            native.difference(&covered).collect::<Vec<_>>(),
            covered.difference(&native).collect::<Vec<_>>()
        );
        ensure!(
            self.requirements.keys().all(|id| native.contains(id)),
            "requirements reference unknown scenario"
        );
        let mut suites = BTreeSet::new();
        for suite in &self.suites {
            ensure!(
                safe_id(&suite.id) && suites.insert(suite.id.as_str()),
                "invalid or repeated suite"
            );
            ensure!(
                suite.lane == format!("local-{}", suite.id),
                "suite lane must use the supported local budget"
            );
            ensure!(
                (1..=20).contains(&suite.repetitions) && suite.technical_retries <= 1,
                "invalid suite sample or retries"
            );
            ensure!(
                !suite.metrics.is_empty() && !suite.purpose.trim().is_empty(),
                "suite must declare purpose and metrics"
            );
            ensure!(
                suite.tags().entries().is_empty(),
                "suite {} selects by tag; only a dispatched suite may",
                suite.id
            );
            self.selected_scenarios(suite, &native)?;
        }
        ensure!(
            !suites.is_empty(),
            "master plan must declare at least one reviewed suite"
        );
        Ok(())
    }

    fn selected_scenarios(&self, suite: &Suite, native: &BTreeSet<String>) -> Result<Vec<String>> {
        let selected = self.select(suite)?;
        ensure!(!selected.is_empty(), "suite {} has no scenarios", suite.id);
        let unknown = selected
            .iter()
            .filter(|id| !native.contains(*id))
            .collect::<Vec<_>>();
        ensure!(
            unknown.is_empty(),
            "suite {} selects unknown scenarios {unknown:?}",
            suite.id
        );
        Ok(selected)
    }

    fn select(&self, suite: &Suite) -> Result<Vec<String>> {
        let mut selected = Vec::new();
        let mut seen = BTreeSet::new();
        for module_id in &suite.modules {
            let module = self
                .modules
                .iter()
                .find(|m| &m.id == module_id)
                .with_context(|| format!("unknown suite module {module_id}"))?;
            for id in &module.scenarios {
                ensure!(seen.insert(id.clone()), "duplicate suite scenario {id}");
                selected.push(id.clone());
            }
        }
        for id in &suite.scenarios {
            ensure!(seen.insert(id.clone()), "duplicate suite scenario {id}");
            selected.push(id.clone());
        }
        if let Some(impact) = self.impact(suite)? {
            let pr = self
                .suites
                .iter()
                .find(|candidate| candidate.id == "pr")
                .context("selection by tag needs the plan's pr suite")?;
            for id in self.select(pr)?.into_iter().chain(impact.matched) {
                if seen.insert(id.clone()) {
                    selected.push(id);
                }
            }
        }
        let mut grouped = BTreeSet::new();
        for group in &suite.scenario_groups {
            ensure!(
                group.len() > 1,
                "scenario group must contain multiple cases"
            );
            for id in group {
                ensure!(
                    seen.contains(id) && grouped.insert(id),
                    "unknown or repeated grouped scenario {id}"
                );
                ensure!(
                    id.parse::<ScenarioId>()?.execution_kind()
                        == ScenarioExecutionKind::HarnessTurn,
                    "sequential suite groups require ordinary harness turns"
                );
            }
        }
        Ok(selected)
    }

    /// One reviewed suite of the master plan, by id.
    pub fn materialize(&self, id: &str) -> Result<ProfileSnapshot> {
        self.validate()?;
        let suite = self
            .suites
            .iter()
            .find(|s| s.id == id)
            .with_context(|| format!("unknown suite {id}"))?
            .clone();
        self.materialize_scope(suite, None)
    }

    /// A suite stated in full by whoever dispatches the execution. Only what
    /// materializing needs is checked: a known scenario set and the id the
    /// campaigns are named after. Its lane is always the local budget.
    pub fn materialize_suite(&self, mut suite: Suite) -> Result<ProfileSnapshot> {
        self.validate()?;
        ensure!(safe_id(&suite.id), "suite id must be lowercase kebab-case");
        if suite.label.trim().is_empty() {
            suite.label = suite.id.clone();
        }
        suite.lane = format!("local-{}", suite.id);
        let native: BTreeSet<_> = ScenarioId::ALL.iter().map(ToString::to_string).collect();
        self.selected_scenarios(&suite, &native)?;
        self.materialize_scope(suite, None)
    }

    pub(crate) fn materialize_scope(
        &self,
        profile: Suite,
        seed: Option<u64>,
    ) -> Result<ProfileSnapshot> {
        let scenario_ids = self.select(&profile)?;
        let native = native_catalog(seed)?;
        let definition_sha256 = self.digest()?;
        let mut cases = Vec::new();
        let mut ordinary_groups = Vec::new();
        let mut subject_turns = 0_u64;
        let mut subject_token_limit = Some(0_u64);
        let mut unbounded_token_cases = Vec::new();
        let mut unbounded_turn_cases = Vec::new();
        for id in &scenario_ids {
            let case = &native[id];
            let key = &case.scenario_id;
            let retries = if key.execution_kind().replay_safe() {
                profile.technical_retries
            } else {
                0
            };
            let admission: crate::control::RunRequest = serde_json::from_value(json!({
                "idempotency_key": format!("plan-preview:{}:{id}", profile.id), "lane": profile.lane,
                "model": "preview", "provider": "preview",
                "scenarios": [id], "runs": 1, "seed": seed, "technical_retries": retries,
            }))?;
            crate::control::validate_run_request(&admission)?;
            let attempts = u64::from(profile.repetitions) * (1 + u64::from(retries));
            let envelope = &case.resource_envelope;
            match envelope.execution.max_turns {
                Some(max_turns) => subject_turns += u64::from(max_turns) * attempts,
                None => unbounded_turn_cases.push(id.clone()),
            }
            // A session ceiling cannot stand in for an unbounded workflow
            // containing several sessions. Keep that whole-case limit unknown.
            let tokens = match &envelope.workflow {
                Some(workflow) => workflow.max_total_tokens,
                None => envelope.execution.max_total_tokens,
            };
            if tokens.is_none() {
                unbounded_token_cases.push(id.clone());
            }
            subject_token_limit = subject_token_limit
                .zip(tokens)
                .and_then(|(sum, cap)| cap.checked_mul(attempts).and_then(|n| sum.checked_add(n)));
            cases.push(json!({
                "scenario_id": id, "behavior_sha256": case.behavior_sha256, "case_id": case.case_id,
                "seed": case.seed, "inputs_sha256": case.inputs_sha256, "contract_sha256": case.contract_sha256,
                "execution_kind": execution_kind(key),
                "resource_envelope": envelope, "required_capabilities": case.required_capabilities,
                "requirements": self.requirements.get(id).cloned().unwrap_or_default(),
                "module": self.modules.iter().find(|m| m.scenarios.contains(id)).map(|m| &m.id),
            }));
            // Every repetition is a fresh invocation. This also obeys the
            // campaign parser's one-case, runs=1 adaptive-flow contract.
            let grouped = profile
                .scenario_groups
                .iter()
                .find(|group| group.contains(id));
            if grouped.is_some_and(|group| &group[0] != id) {
                continue;
            }
            let group = grouped.cloned().unwrap_or_else(|| vec![id.clone()]);
            let group_retries = if group
                .iter()
                .all(|id| native[id].scenario_id.execution_kind().replay_safe())
            {
                profile.technical_retries
            } else {
                0
            };
            ordinary_groups.push(json!({
                "id": format!("case-{}", id.replace('_', "-")),
                "execution_kind": execution_kind(key), "runs": 1,
                "technical_retries": group_retries,
                "scenarios": group,
            }));
        }
        let profile_sha256 = artifact::sha256_value(
            &json!({"definition_sha256": definition_sha256, "profile": profile, "cases": cases}),
        )?;
        let mut campaigns = Vec::new();
        for repetition in 1..=profile.repetitions {
            let campaign = json!({
                "kind": "harness-e2e-campaign", "campaign_id": format!("{}-r{repetition:02}", profile.id),
                "lane": profile.lane, "failure_policy": "advisory", "groups": ordinary_groups,
            });
            campaigns.push(campaign);
        }
        Ok(ProfileSnapshot {
            schema: "harness-e2e-profile-snapshot".into(),
            plan_id: self.plan_id.clone(),
            definition_sha256,
            profile_sha256,
            profile: profile.clone(),
            scenario_ids: scenario_ids.clone(),
            cases,
            campaigns,
            budget: json!({"scenario_runs": scenario_ids.len() as u64 * u64::from(profile.repetitions),
                "planned_runs": scenario_ids.len() as u64 * u64::from(profile.repetitions),
                "session_turn_limit_sum": subject_turns, "subject_token_limit": subject_token_limit,
                "unbounded_token_cases": unbounded_token_cases,
                "unbounded_turn_cases": unbounded_turn_cases,
                "max_concurrent_groups": 1, "scope": "turn sum counts per-session limits, not a whole-workflow ceiling; tokens cover subject only; setup, capture and cleanup are additional"}),
            interpretation: "descriptive_only".into(),
            impact: self.impact(&profile)?,
        })
    }

    /// The scheduled tests a suite's tags match; `None` when it names none.
    fn impact(&self, suite: &Suite) -> Result<Option<Impact>> {
        let requested = suite.tags();
        let wanted = requested.entries();
        if wanted.is_empty() {
            return Ok(None);
        }
        let (mut matched, mut excluded) = (Vec::new(), Vec::new());
        let (mut covered, mut left_out) = (BTreeSet::new(), BTreeSet::new());
        for id in self.scheduled()? {
            let scenario = id.parse::<ScenarioId>()?;
            let carries = tag_entries(&scenario.tags());
            let carried = (wanted.iter().copied())
                .filter(|tag| carries.contains(tag))
                .collect::<Vec<_>>();
            if carried.is_empty() {
                continue;
            }
            if IMPACT_EXCLUDED.contains(&scenario) {
                left_out.extend(carried);
                excluded.push(id);
            } else {
                covered.extend(carried);
                matched.push(id);
            }
        }
        let known = vocabulary();
        let uncovered = (wanted.iter())
            .filter(|tag| !covered.contains(*tag))
            .map(|&(dimension, name)| UncoveredTag {
                dimension: dimension.into(),
                name: name.into(),
                reason: if !known.contains(&(dimension, name)) {
                    "not in the tag vocabulary"
                } else if left_out.contains(&(dimension, name)) {
                    "only tests a selection by tag leaves out carry it"
                } else {
                    "no scheduled test carries it"
                }
                .into(),
            })
            .collect();
        Ok(Some(Impact {
            requested,
            matched,
            excluded,
            uncovered,
        }))
    }

    /// Every test some suite runs: the base of selection by tag.
    pub fn scheduled(&self) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for suite in &self.suites {
            ids.extend(self.select(suite)?);
        }
        Ok(ids)
    }

    /// Which scheduled tests protect each worker, area and discipline. Every
    /// value of the vocabularies is listed, so an uncovered one reads empty.
    pub fn coverage(&self) -> Result<Value> {
        let scheduled = self
            .scheduled()?
            .into_iter()
            .map(|id| Ok((id.parse::<ScenarioId>()?.tags(), id)))
            .collect::<Result<Vec<_>>>()?;
        let tests = |protects: &dyn Fn(&ScenarioTags) -> bool| {
            scheduled
                .iter()
                .filter(|(tags, _)| protects(tags))
                .map(|(_, id)| id.as_str())
                .collect::<Vec<_>>()
        };
        let workers = Worker::ALL
            .iter()
            .map(|worker| {
                (
                    worker.as_str(),
                    tests(&|tags| tags.workers.contains(worker)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let areas = Area::ALL
            .iter()
            .map(|area| (area.as_str(), tests(&|tags| tags.area == Some(*area))))
            .collect::<BTreeMap<_, _>>();
        let disciplines = Discipline::ALL
            .iter()
            .map(|discipline| {
                let protects = |tags: &ScenarioTags| tags.disciplines.contains(discipline);
                (discipline.as_str(), tests(&protects))
            })
            .collect::<BTreeMap<_, _>>();
        Ok(json!({
            "scheduled": scheduled.len(),
            "harness_only": tests(&|tags| tags.workers.is_empty()),
            "workers": workers,
            "areas": areas,
            "disciplines": disciplines,
        }))
    }

    pub fn catalog(&self) -> Result<Value> {
        let mut suites = Vec::new();
        for suite in &self.suites {
            let snapshot = self.materialize(&suite.id)?;
            suites.push(json!({"id": suite.id, "label": suite.label, "purpose": suite.purpose, "metrics": suite.metrics,
                "scenario_ids": snapshot.scenario_ids, "repetitions": suite.repetitions,
                "technical_retries": suite.technical_retries, "budget": snapshot.budget,
                "sha256": snapshot.profile_sha256,
                "cases": snapshot.cases}));
        }
        Ok(json!({"plan_id": self.plan_id, "definition_sha256": self.digest()?, "suites": suites}))
    }

    pub fn campaign_catalog(&self) -> Result<Value> {
        let mut scenarios = BTreeMap::new();
        for (id, case) in native_catalog(None)? {
            scenarios.insert(
                id,
                json!({"execution_kind": execution_kind(&case.scenario_id)}),
            );
        }
        // The finalizer runs the checkout's scripts with this runner: the
        // contract it writes Results under is stated here, not assumed.
        Ok(json!({
            "schema": "harness-e2e-campaign-catalog", "definition_sha256": self.digest()?,
            "result_contract_sha256": crate::report::RESULT_CONTRACT_SHA256,
            "scenarios": scenarios,
        }))
    }
}

/// Aggregate a profile's independent invocations using the existing Results
/// contract. Different cases, models or stack identities are never pooled.
type MeasurementCohorts = BTreeMap<String, (Value, crate::report::E2eScenarioReport)>;

fn measurement_cohorts(paths: &[std::path::PathBuf]) -> Result<(MeasurementCohorts, Vec<Value>)> {
    use crate::report::{E2eReport, E2eScenarioReport};
    ensure!(
        !paths.is_empty(),
        "measurement requires at least one Results artifact"
    );
    let mut cohorts: BTreeMap<String, (Value, E2eScenarioReport)> = BTreeMap::new();
    let mut observations = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut deferred = Vec::new();
    for path in paths {
        ensure!(
            files.insert(std::fs::canonicalize(path)?),
            "duplicate Results input"
        );
        let (report, _) = E2eReport::read_from(path)?;
        for scenario in report.scenarios {
            let Some(case) = scenario.case.as_ref() else {
                deferred.push(json!({"scenario_id": scenario.scenario_id, "planned_runs": scenario.aggregate.planned_runs, "reason": scenario.deferral_reason}));
                continue;
            };
            for run in &scenario.runs {
                ensure!(
                    observations.insert((run.run_id.clone(), run.attempt_id.clone())),
                    "duplicate run/attempt in profile evidence"
                );
                for retry in &run.retry_attempts {
                    ensure!(
                        observations.insert((retry.run_id.clone(), retry.attempt_id.clone())),
                        "duplicate retry attempt in profile evidence"
                    );
                }
            }
            let identity = json!({"case": case, "execution_policy": scenario.execution_policy,
                "system_under_test": report.system_under_test, "subject": report.subject});
            let digest = artifact::sha256_value(&identity)?;
            if let Some((_, accumulated)) = cohorts.get_mut(&digest) {
                let planned = accumulated
                    .aggregate
                    .planned_runs
                    .checked_add(scenario.aggregate.planned_runs)
                    .context("planned sample overflow")?;
                let mut runs = std::mem::take(&mut accumulated.runs);
                runs.extend(scenario.runs);
                *accumulated = E2eScenarioReport::aggregate_case_with_planned(
                    case.clone(),
                    scenario.execution_policy,
                    planned,
                    runs,
                );
            } else {
                cohorts.insert(digest, (identity, scenario));
            }
        }
    }
    Ok((cohorts, deferred))
}

pub fn measure(paths: &[std::path::PathBuf]) -> Result<Value> {
    let (cohorts, deferred) = measurement_cohorts(paths)?;
    let cohorts: Vec<_> = cohorts.into_iter().map(|(digest, (identity, scenario))| {
        json!({"cohort_sha256": digest, "identity": identity, "scenario_id": scenario.scenario_id,
            "aggregate": scenario.aggregate, "consumption": crate::longitudinal::consumption_metrics(&scenario.runs),
            "run_ids": scenario.runs.iter().map(|r| &r.run_id).collect::<Vec<_>>()})
    }).collect();
    Ok(
        json!({"schema": "harness-e2e-profile-measurements", "interpretation": "descriptive_only",
        "cohorts": cohorts, "deferred": deferred, "input_artifacts": paths}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_samples_preserve_independent_execution_and_retry_boundaries() {
        let plan = embedded().unwrap();
        assert_eq!(plan.suites.len(), 14);
        for (id, cases, runs) in [
            ("regression", 9, 99),
            ("null-canary", 10, 10),
            ("software-engineering", 14, 14),
            ("software-engineering-x3", 14, 42),
            ("software-engineering-x9", 14, 126),
            ("regression-x11", 9, 99),
            ("context-capacity", 4, 4),
            ("deepwork", 3, 3),
            ("linkly", 1, 1),
            ("onboarding", 1, 1),
            ("pr", 4, 4),
            ("after-release", 5, 5),
            ("memory", 22, 66),
            ("memory-learning", 7, 21),
        ] {
            let snapshot = plan.materialize(id).unwrap();
            assert_eq!(snapshot.scenario_ids.len(), cases);
            assert_eq!(snapshot.budget["planned_runs"], runs);
            assert_eq!(snapshot.campaigns.len() * cases, runs);
            let mut rounds = BTreeSet::new();
            for campaign in snapshot.campaigns {
                assert!(rounds.insert(campaign["campaign_id"].as_str().unwrap().to_string()));
                let groups = campaign["groups"].as_array().unwrap();
                let mut selected = BTreeSet::new();
                for group in groups {
                    assert_eq!(group["runs"], 1);
                    for id in group["scenarios"].as_array().unwrap() {
                        let id = id.as_str().unwrap();
                        assert!(selected.insert(id));
                        if !id
                            .parse::<ScenarioId>()
                            .unwrap()
                            .execution_kind()
                            .replay_safe()
                        {
                            assert_eq!(group["technical_retries"], 0);
                        }
                    }
                }
                assert_eq!(selected.len(), cases);
            }
        }
    }

    #[test]
    fn software_engineering_profile_includes_trending_topics_independently() {
        let snapshot = embedded()
            .unwrap()
            .materialize("software-engineering")
            .unwrap();
        let expected = crate::scenarios::kanban::IDS
            .into_iter()
            .chain([
                "registry_implementation",
                "registry_verification",
                "trending_topics_build",
                "alertmanager_route_match",
                "chess_engine_build",
                "form_flow_build",
                "state_machine_canvas_build",
            ])
            .collect::<Vec<_>>();
        assert_eq!(snapshot.scenario_ids, expected);
        let groups = snapshot.campaigns[0]["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 13);
        let build = groups
            .iter()
            .find(|g| g["id"] == "case-trending-topics-build")
            .unwrap();
        assert_eq!(build["scenarios"], json!(["trending_topics_build"]));
        assert_eq!(build["runs"], 1);
        assert_eq!(build["technical_retries"], 0);
        let alertmanager = groups
            .iter()
            .find(|g| g["id"] == "case-alertmanager-route-match")
            .unwrap();
        assert_eq!(
            alertmanager["scenarios"],
            json!(["alertmanager_route_match"])
        );
        assert_eq!(alertmanager["technical_retries"], 0);
        let chess = groups
            .iter()
            .find(|g| g["id"] == "case-chess-engine-build")
            .unwrap();
        assert_eq!(chess["scenarios"], json!(["chess_engine_build"]));
        assert_eq!(chess["technical_retries"], 0);
        for (group_id, scenario_id) in [
            ("case-form-flow-build", "form_flow_build"),
            (
                "case-state-machine-canvas-build",
                "state_machine_canvas_build",
            ),
        ] {
            let group = groups.iter().find(|group| group["id"] == group_id).unwrap();
            assert_eq!(group["scenarios"], json!([scenario_id]));
            assert_eq!(group["technical_retries"], 0);
        }
        assert!(snapshot.budget["unbounded_turn_cases"]
            .as_array()
            .unwrap()
            .contains(&json!("alertmanager_route_match")));
        let delivery = groups
            .iter()
            .find(|g| g["id"] == "case-registry-implementation")
            .unwrap();
        assert_eq!(
            delivery["scenarios"],
            json!(["registry_implementation", "registry_verification"])
        );
        assert_eq!(snapshot.profile.repetitions, 1);
        assert_eq!(snapshot.profile.technical_retries, 0);
    }

    #[test]
    fn a_dispatched_suite_selects_pr_plus_the_scheduled_tests_with_its_tags() {
        let plan = embedded().unwrap();
        let suite = |value: Value| plan.materialize_suite(serde_json::from_value(value).unwrap());
        let pr = [
            "minimal_path",
            "persistent_state",
            "tool_contract_recovery",
            "shell_coder_sandbox",
        ];

        let state = suite(json!({"id": "impact", "workers": ["state"]})).unwrap();
        assert_eq!(
            state.scenario_ids,
            [&pr[..], &["contention_ledger", "timer_wake"]].concat()
        );
        let impact = state.impact.as_ref().unwrap();
        assert_eq!(
            impact.matched,
            [
                "contention_ledger",
                "minimal_path",
                "persistent_state",
                "timer_wake"
            ]
        );
        // Linkly and the onboarding tour carry state but need a stack of
        // their own.
        assert_eq!(impact.excluded, ["linkly_tutorial", "onboarding_tour"]);
        assert!(impact.uncovered.is_empty());
        // The tags stay out of the profile, and so out of every digest.
        let profile = serde_json::to_value(&state.profile).unwrap();
        assert!(profile.get("workers").is_none());

        let thin =
            suite(json!({"id": "impact", "workers": ["cron"], "areas": ["coding"]})).unwrap();
        assert_eq!(
            thin.scenario_ids,
            [&pr[..], &["alertmanager_route_match"]].concat()
        );
        let impact = thin.impact.unwrap();
        assert_eq!(impact.excluded, ["registry_implementation"]);
        let gap = |dimension: &str, name: &str, reason: &str| UncoveredTag {
            dimension: dimension.into(),
            name: name.into(),
            reason: reason.into(),
        };
        assert_eq!(
            impact.uncovered,
            [gap("worker", "cron", "no scheduled test carries it")]
        );

        // A name the tags do not know warns instead of failing the suite,
        // and a tag only an excluded test carries says so.
        let unknown = suite(json!({
            "id": "impact",
            "workers": ["provider-anthropic", "queue"],
            "areas": ["harness"],
        }))
        .unwrap();
        assert_eq!(unknown.scenario_ids, pr);
        assert_eq!(
            unknown.impact.unwrap().uncovered,
            [
                gap("worker", "provider-anthropic", "not in the tag vocabulary"),
                gap(
                    "worker",
                    "queue",
                    "only tests a selection by tag leaves out carry it"
                ),
                gap("area", "harness", "not in the tag vocabulary"),
            ]
        );

        // A suite naming no tag has no impact.
        assert!(plan.materialize("pr").unwrap().impact.is_none());
    }

    /// Tags never reach the plan's digest: the cohort key Release Control
    /// groups every execution by. A reviewed change to the plan updates this,
    /// and gives every suite a new `profile_sha256` (README, Test plans).
    #[test]
    fn the_plan_digest_is_pinned() {
        assert_eq!(
            embedded().unwrap().digest().unwrap(),
            "sha256:fdbc6f9343baaace98c193444ac2562024a749ae739f9ad916afe1b1068e676d"
        );
    }

    #[test]
    fn the_plan_never_selects_by_tag() {
        let mut document: Value = serde_json::from_str(SOURCE).unwrap();
        document["suites"][0]["workers"] = json!(["state"]);
        let plan: MasterPlan = serde_json::from_value(document).unwrap();
        assert!(plan
            .validate()
            .unwrap_err()
            .to_string()
            .contains("selects by tag"));
    }

    #[test]
    fn coverage_lists_every_tag_with_the_tests_behind_it() {
        let plan = embedded().unwrap();
        let coverage = plan.coverage().unwrap();
        assert_eq!(
            coverage["scheduled"],
            json!(plan.scheduled().unwrap().len())
        );
        assert_eq!(
            coverage["workers"].as_object().unwrap().len(),
            Worker::ALL.len()
        );
        assert_eq!(coverage["workers"]["browser"], json!([]));
        assert_eq!(
            coverage["workers"]["state"],
            json!([
                "contention_ledger",
                "linkly_tutorial",
                "minimal_path",
                "onboarding_tour",
                "persistent_state",
                "timer_wake"
            ])
        );
        assert_eq!(
            coverage["areas"]["verification"],
            json!(["registry_verification"])
        );
        assert_eq!(
            coverage["disciplines"]["data"],
            json!([
                "database_migration_recovery",
                "kanban_c2_persistence",
                "kanban_c4_ticket_flow",
                "kanban_c6_discussion",
                "linkly_tutorial",
                "onboarding_tour"
            ])
        );
    }

    /// Every test a scheduled suite runs reads as a card: a title, what it
    /// protects (and what breaks when it fails), and the task in plain words.
    #[test]
    fn every_scheduled_test_states_its_card() {
        let plan = embedded().unwrap();
        for suite in [
            "regression",
            "null-canary",
            "pr",
            "software-engineering",
            "software-engineering-x3",
            "linkly",
            "memory",
            "memory-learning",
        ] {
            for id in plan.materialize(suite).unwrap().scenario_ids {
                let scenario = id.parse::<ScenarioId>().unwrap();
                for (field, text) in [
                    ("title", scenario.title()),
                    ("summary", scenario.summary()),
                    ("protects", scenario.protects()),
                ] {
                    assert!(
                        text.is_some_and(|text| !text.trim().is_empty()),
                        "{suite}: {id} states no {field}"
                    );
                }
                assert!(
                    scenario.protects().unwrap().contains(". If it fails, "),
                    "{suite}: {id} protects must say what breaks when it fails"
                );
                // The card lists each criterion by its label, a short
                // statement; the full question stays one step away.
                for criterion in scenario.spec("card").criteria {
                    let words = criterion
                        .label
                        .map_or(0, |label| label.split_whitespace().count());
                    assert!(
                        (3..=8).contains(&words),
                        "{suite}: {id} criterion {} needs a label of 3 to 8 words, has {:?}",
                        criterion.id,
                        criterion.label
                    );
                }
            }
        }
    }

    #[test]
    fn deepwork_reuses_native_build_contracts_that_allow_delegation() {
        let plan = embedded().unwrap();
        let snapshot = plan.materialize("deepwork").unwrap();
        assert_eq!(
            snapshot.scenario_ids,
            [
                "chess_engine_build",
                "form_flow_build",
                "state_machine_canvas_build"
            ]
        );
        for id in snapshot.scenario_ids {
            let scenario = id.parse::<ScenarioId>().unwrap();
            let case = scenario.materialize("attempt", 4404).unwrap();
            assert!(case.spec.filesystem_root.is_some());
            assert!(case.module.allowed_functions("attempt").is_none());
            assert!(!case.spec.denied_functions.contains(&"harness::spawn"));
        }
    }

    #[test]
    fn linkly_runs_alone_in_its_own_suite() {
        let snapshot = embedded().unwrap().materialize("linkly").unwrap();
        assert_eq!(snapshot.scenario_ids, ["linkly_tutorial"]);
        assert_eq!(
            snapshot.campaigns[0]["groups"],
            json!([{
                "id": "case-linkly-tutorial", "execution_kind": "scripted_dialogue",
                "runs": 1, "technical_retries": 0, "scenarios": ["linkly_tutorial"],
            }])
        );
    }

    /// Every memory case is its own group, so a retry reruns one case and a
    /// case never shares a stack with its baseline. The learning cases run in
    /// a suite of their own, after the deterministic ones; their baselines
    /// stay in `memory`.
    #[test]
    fn memory_runs_each_case_alone_three_times_with_one_retry() {
        let plan = embedded().unwrap();
        let mut every_case = Vec::new();
        for suite in ["memory", "memory-learning"] {
            let snapshot = plan.materialize(suite).unwrap();
            assert_eq!(snapshot.campaigns.len(), 3);
            for campaign in &snapshot.campaigns {
                let groups = campaign["groups"].as_array().unwrap();
                assert_eq!(groups.len(), snapshot.scenario_ids.len());
                for (group, id) in groups.iter().zip(&snapshot.scenario_ids) {
                    assert_eq!(group["scenarios"], json!([id]));
                    assert_eq!(group["execution_kind"], "harness_turn");
                    assert_eq!(group["technical_retries"], 1);
                }
            }
            every_case.extend(snapshot.scenario_ids);
        }
        assert_eq!(every_case, crate::scenarios::memory::IDS);
        // Exactly the cases whose outcome rests on memory's extraction.
        use crate::scenarios::memory::cases::{audits_bank_after, CASES};
        let learning: Vec<&str> = CASES
            .iter()
            .filter(|case| !case.teacher.is_empty() || audits_bank_after(case))
            .map(|case| case.id)
            .collect();
        assert_eq!(
            plan.materialize("memory-learning").unwrap().scenario_ids,
            learning
        );
    }

    /// The memory cases need the memory stack, so a selection by tag leaves
    /// them out instead of running them where they would fail in setup.
    #[test]
    fn a_selection_by_the_memory_worker_leaves_the_memory_cases_out() {
        let plan = embedded().unwrap();
        let impact = plan
            .materialize_suite(
                serde_json::from_value(json!({"id": "impact", "workers": ["memory"]})).unwrap(),
            )
            .unwrap();
        assert!(!impact
            .scenario_ids
            .iter()
            .any(|id| id.starts_with("memory_")));
        let impact = impact.impact.unwrap();
        assert!(impact.matched.is_empty());
        let mut every_case = crate::scenarios::memory::IDS;
        every_case.sort_unstable();
        assert_eq!(impact.excluded, every_case);
        assert_eq!(
            impact.uncovered[0].reason,
            "only tests a selection by tag leaves out carry it"
        );
    }

    /// The weekly variance reading: the daily suite's groups, three
    /// independent rounds, each round its own campaign and so its own jobs.
    #[test]
    fn the_weekly_x3_suite_repeats_software_engineering_three_times() {
        let plan = embedded().unwrap();
        let daily = plan.materialize("software-engineering").unwrap();
        let weekly = plan.materialize("software-engineering-x3").unwrap();
        assert_eq!(daily.profile.repetitions, 1);
        assert_eq!(weekly.scenario_ids, daily.scenario_ids);
        let rounds: Vec<_> = weekly
            .campaigns
            .iter()
            .map(|campaign| campaign["campaign_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            rounds,
            [
                "software-engineering-x3-r01",
                "software-engineering-x3-r02",
                "software-engineering-x3-r03"
            ]
        );
        for campaign in &weekly.campaigns {
            assert_eq!(campaign["groups"], daily.campaigns[0]["groups"]);
        }
        assert_eq!(plan.materialize("regression").unwrap().campaigns.len(), 11);
    }

    /// The version-comparison suites are the daily ones with more rounds,
    /// and the capacity lane is four tests that already exist.
    #[test]
    fn the_comparison_and_capacity_suites_reuse_existing_tests() {
        let plan = embedded().unwrap();
        for (comparison, base, rounds) in [
            ("software-engineering-x9", "software-engineering", 9),
            ("regression-x11", "regression", 11),
        ] {
            let (comparison, base) = (
                plan.materialize(comparison).unwrap(),
                plan.materialize(base).unwrap(),
            );
            assert_eq!(comparison.scenario_ids, base.scenario_ids);
            assert_eq!(comparison.campaigns.len(), rounds);
            assert_eq!(
                comparison.campaigns[0]["groups"],
                base.campaigns[0]["groups"]
            );
        }
        assert_eq!(
            plan.materialize("context-capacity").unwrap().scenario_ids,
            [
                "kanban_c1_foundation",
                "kanban_c4_ticket_flow",
                "kanban_c7_live",
                "context_pressure"
            ]
        );
    }

    const NULL_AGENT: &str = include_str!("../config/agents/null-agent.md");

    /// The canary is the regression plus the minimal path (the one test the
    /// `pr` and `after-release` suites add), run by an agent that does nothing.
    #[test]
    fn the_null_canary_runs_the_regression_as_an_agent_that_does_nothing() {
        let plan = embedded().unwrap();
        let mut expected = plan.materialize("regression").unwrap().scenario_ids;
        expected.insert(0, ScenarioId::MinimalPath.to_string());
        assert_eq!(
            plan.materialize("null-canary").unwrap().scenario_ids,
            expected
        );
        for suite in ["pr", "after-release"] {
            for id in plan.materialize(suite).unwrap().scenario_ids {
                assert!(expected.contains(&id), "{suite}: {id} escapes the canary");
            }
        }
        // A parent, skills, preloaded functions or a model of its own would
        // bring in the calls the null agent exists never to make.
        let (frontmatter, body) = NULL_AGENT
            .strip_prefix("---\n")
            .and_then(|text| text.split_once("\n---\n"))
            .unwrap();
        for key in ["extends:", "skills:", "functions:", "model:"] {
            assert!(!frontmatter.contains(key), "null-agent declares {key}");
        }
        assert_eq!(body.trim().lines().last(), Some("done"));
    }

    /// Scheduled tests whose evaluator scores what a live stack left behind:
    /// why, and the deterministic controls CI runs for them instead. An
    /// empty list says "no CI control" in its reason, with what runs by hand.
    const LIVE_CONTROLS: &[(ScenarioId, &str, &[&str])] = &[
        (
            ScenarioId::RegistryImplementation,
            "capture builds the delivery in Docker and probes it; validate.py and the \
             feature probe's failure regressions are scored in CI",
            &[
                "tests/python/test_registry_validation.py",
                "tests/python/test_registry_feature_probe.py",
            ],
        ),
        (
            ScenarioId::RegistryVerification,
            "capture replays the report against the delivery in Docker; the empty, the \
             claim-only, the one-check-executed and the all-passing report are scored in CI",
            &["tests/python/test_registry_validation.py"],
        ),
        (
            ScenarioId::TrendingTopicsBuild,
            "capture builds the pushed site and runs Playwright in Docker; CI runs the \
             evaluator, lifecycle and focus controls, while the reference app in \
             validate-controls.mjs needs the private e2e-fixture app and runs by hand",
            &[
                "tests/fixtures/trending-topics-build/evaluator.test.mjs",
                "tests/fixtures/trending-topics-build/test_lifecycle.py",
                "tests/python/test_trending_focus_eval_regressions.py",
            ],
        ),
        (
            ScenarioId::ChessEngineBuild,
            "capture plays the Compose-started Worker in the Console; CI runs the browser \
             controls of tests/fixtures/chess-eval/negative-control.mjs",
            &["tests/python/test_chess_eval_regressions.py"],
        ),
        (ScenarioId::FormFlowBuild, VISUAL_LIVE, VISUAL_CI),
        (ScenarioId::StateMachineCanvasBuild, VISUAL_LIVE, VISUAL_CI),
        (
            ScenarioId::LinklyTutorial,
            "capture probes the running linkly-agentic scaffold, chapter by chapter; \
             no CI control",
            &[],
        ),
        (
            ScenarioId::OnboardingTour,
            "capture reads what the tour left in the onboarding stack's engine; no CI control",
            &[],
        ),
        (ScenarioId::KanbanC1Foundation, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::KanbanC2Persistence, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::KanbanC3Board, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::KanbanC4TicketFlow, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::KanbanC5EditMove, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::KanbanC6Discussion, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::KanbanC7Live, KANBAN_LIVE, KANBAN_CI),
        (ScenarioId::MemoryRelevantRecall, MEMORY_LIVE, &[]),
        (ScenarioId::MemorySmallTalk, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryFirstPersonOffTopic, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryParaphrase, MEMORY_LIVE, &[]),
        (ScenarioId::MemorySessionRepeat, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryUpdatedFact, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryValueProject, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryValueProjectBaseline, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryValuePreference, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryValuePreferenceBaseline, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryRuleFit, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryChatterAfterAnswer, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryUserFactNeeded, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryAboutMe, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryRewordedAcrossTurns, MEMORY_LIVE, &[]),
        (ScenarioId::MemorySiblingFacts, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryRuleChangedMidSession, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryBankEditsMidSession, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryBankTrashedMidSession, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryManySiblings, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryAboutMeOverflow, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryLinkedUpdate, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryLearnThenRecall, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryLearnRuleThenObey, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryLearnCorrection, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryAutomationNotLearned, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryLearnFromLongMessage, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryHonorsDoNotKeep, MEMORY_LIVE, &[]),
        (ScenarioId::MemoryKeepsWithoutARule, MEMORY_LIVE, &[]),
    ];
    const MEMORY_LIVE: &str =
        "capture runs live sessions against the memory worker and audits what it injected \
         into each turn; the subject is the memory worker, not the agent, so a null agent is \
         no control of it, and memory/score.rs fails each gate offline; no CI control";
    const VISUAL_LIVE: &str = "capture drives the Compose-started Worker in the Console; \
         CI runs the browser controls of tests/fixtures/visual-worker-eval/regressions.mjs";
    const VISUAL_CI: &[&str] = &["tests/python/test_visual_worker_eval_regressions.py"];
    const KANBAN_LIVE: &str =
        "capture runs the delivered application under Compose in an isolated \
         Docker pair and probes it with Playwright; CI runs the browser controls and scoring \
         regressions, while the base and reference controls of scripts/kanban_eval/run.py \
         need Docker and the fixture and run by hand";
    const KANBAN_CI: &[&str] = &[
        "tests/python/test_kanban_browser_flows.py",
        "tests/python/test_kanban_scoring_regressions.py",
    ];
    const CI: &str = include_str!("../.github/workflows/ci.yml");

    /// Every control the table cites exists and CI runs it: named in the
    /// workflow, or a `tests/python/test_*.py` its discovery runs.
    #[test]
    fn the_live_controls_cited_run_in_ci() {
        let discovers = CI.contains("unittest discover -s tests/python -p 'test_*.py'");
        for (id, reason, controls) in LIVE_CONTROLS {
            assert_eq!(
                controls.is_empty(),
                reason.contains("no CI control"),
                "{id}: say \"no CI control\" exactly when none is cited"
            );
            for control in *controls {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(control);
                assert!(path.is_file(), "{id}: {control} does not exist");
                let discovered = discovers
                    && control.starts_with("tests/python/test_")
                    && control.ends_with(".py");
                assert!(
                    discovered || CI.contains(control),
                    "{id}: CI does not run {control}"
                );
            }
        }
    }

    /// The null and the reference delivery of a scheduled test whose
    /// evaluator scores offline, in that order.
    async fn offline_controls(
        id: ScenarioId,
    ) -> Option<[crate::scenarios::ObjectiveEvaluation; 2]> {
        use crate::scenarios::*;
        Some(match id {
            ScenarioId::MinimalPath => minimal_path::tests::controls().await,
            ScenarioId::PersistentState => persistent_state::tests::controls().await,
            ScenarioId::ToolContractRecovery => tool_contract_recovery::tests::controls(),
            ScenarioId::TimerWake => timer_wake::tests::controls(),
            ScenarioId::ShellCoderSandbox => shell_coder_sandbox::tests::controls().await,
            ScenarioId::DatabaseMigrationRecovery => database_migration_recovery::tests::controls(),
            ScenarioId::ContentionLedger => contention_ledger::tests::controls(),
            ScenarioId::ValidationSelfRepair => validation_self_repair::tests::controls(),
            ScenarioId::ContextPressure => context_pressure::tests::controls(),
            ScenarioId::PromptInjectionResilience => prompt_injection_resilience::tests::controls(),
            ScenarioId::AlertmanagerRouteMatch => alertmanager_route_match::tests::controls().await,
            _ => return None,
        })
    }

    /// The null agent without a model, and its counterpart. Every scheduled
    /// test is scored as the runner scores it against two deliveries: the
    /// null one (no call, one turn, a reply of `done` or one that only
    /// claims the result) earns 0 and stays `task_incomplete`; the reference
    /// one completes with 100. Both run ten times: a result that differs
    /// between rounds is named as drift.
    #[tokio::test]
    async fn every_scheduled_test_pays_nothing_for_nothing_and_everything_for_the_task() {
        use crate::report::CompletionState;
        let plan = embedded().unwrap();
        // A suite schedules its own scenarios and those of the modules it names.
        let scheduled: BTreeSet<&str> = plan
            .suites
            .iter()
            .flat_map(|suite| {
                let modules = plan
                    .modules
                    .iter()
                    .filter(|module| suite.modules.contains(&module.id))
                    .flat_map(|module| module.scenarios.iter());
                suite.scenarios.iter().chain(modules).map(String::as_str)
            })
            .collect();
        for (id, _, _) in LIVE_CONTROLS {
            assert!(scheduled.contains(id.as_str()), "{id} is not scheduled");
        }
        let mut holes = Vec::new();
        for id in scheduled {
            let id: ScenarioId = id.parse().unwrap();
            let live = LIVE_CONTROLS.iter().any(|(live, _, _)| *live == id);
            let spec = id.spec("control");
            let score = |evaluation: crate::scenarios::ObjectiveEvaluation| {
                let criteria = crate::suite::criterion_reports(&spec, evaluation.awards);
                let completion = evaluation
                    .completion
                    .unwrap_or_else(|| crate::report::gate_completion(&criteria));
                let points: u16 = criteria
                    .iter()
                    .filter_map(|criterion| criterion.awarded.map(u16::from))
                    .sum();
                (points, completion)
            };
            let mut rounds = Vec::new();
            for _ in 0..10 {
                let Some([null, reference]) = offline_controls(id).await else {
                    break;
                };
                rounds.push((score(null), score(reference)));
            }
            assert_eq!(
                rounds.is_empty(),
                live,
                "{id}: offline controls xor a live reason"
            );
            for (delivery, expected, results) in [
                (
                    "null",
                    (0, CompletionState::TaskIncomplete),
                    rounds.iter().map(|round| round.0).collect::<Vec<_>>(),
                ),
                (
                    "reference",
                    (100, CompletionState::Completed),
                    rounds.iter().map(|round| round.1).collect(),
                ),
            ] {
                if results.iter().any(|result| *result != results[0]) {
                    holes.push(format!("{id} {delivery} drifts: {results:?}"));
                } else if results.first().is_some_and(|result| *result != expected) {
                    holes.push(format!("{id} {delivery}: {:?}", results[0]));
                }
            }
        }
        assert!(holes.is_empty(), "{holes:#?}");
    }

    #[test]
    fn software_engineering_orders_registry_delivery_and_verification_in_one_group() {
        let plan = embedded().unwrap();
        let snapshot = plan.materialize("software-engineering").unwrap();
        let groups = snapshot.campaigns[0]["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 13);
        let build = groups
            .iter()
            .find(|g| g["id"] == "case-trending-topics-build")
            .unwrap();
        assert_eq!(build["scenarios"], json!(["trending_topics_build"]));
        let delivery = groups
            .iter()
            .find(|g| g["id"] == "case-registry-implementation")
            .unwrap();
        assert_eq!(
            delivery["scenarios"],
            json!(["registry_implementation", "registry_verification"])
        );
        assert_eq!(snapshot.cases.len(), 14);
        assert_eq!(snapshot.budget["planned_runs"], 14);

        let mut profile = snapshot.profile;
        profile.scenario_groups[0].push("registry_verification".into());
        assert!(plan.materialize_scope(profile, None).is_err());
    }

    #[test]
    fn source_rejects_lost_coverage_overlap_and_invalid_samples() {
        let plan = embedded().unwrap();
        let mut changed = plan.clone();
        changed.modules[0].scenarios.pop();
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.suites[0].scenarios.push("persistent_state".into());
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.suites[0].repetitions = 21;
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.suites[0].scenarios[0] = "local_invented".into();
        assert!(changed.validate().is_err());
    }

    #[test]
    fn materialized_scope_changes_identity_and_never_invents_budget() {
        let plan = embedded().unwrap();
        let first = plan.materialize("regression").unwrap();
        assert_eq!(
            first.profile_sha256,
            plan.materialize("regression").unwrap().profile_sha256
        );
        let mut changed = plan;
        changed.suites[0].repetitions = 6;
        assert_ne!(
            first.profile_sha256,
            changed.materialize("regression").unwrap().profile_sha256
        );
        assert!(first.budget["subject_token_limit"].is_null());
        assert!(!first.budget["unbounded_token_cases"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn renaming_profiles_to_suites_keeps_every_digest_release_control_keys_on() {
        // Release Control groups executions by `profile_sha256`, which folds in
        // `definition_sha256`: the digest of the plan document as the `main`
        // before the rename hashed it, with the list under `profiles`.
        let mut document: Value = serde_json::from_str(SOURCE).unwrap();
        let object = document.as_object_mut().unwrap();
        let suites = object.remove("suites").unwrap();
        object.insert("profiles".into(), suites);
        let plan = embedded().unwrap();
        assert_eq!(
            plan.digest().unwrap(),
            artifact::sha256_value(&document).unwrap()
        );
        // Either name reads the same plan.
        let older: MasterPlan = serde_json::from_value(document).unwrap();
        assert_eq!(older.digest().unwrap(), plan.digest().unwrap());
        // A suite serializes as a profile did: the same fields, nothing more.
        for suite in &plan.suites {
            let keys = serde_json::to_value(suite).unwrap();
            let mut keys = keys
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            keys.retain(|key| key != "scenario_groups");
            keys.sort();
            assert_eq!(
                keys,
                [
                    "id",
                    "label",
                    "lane",
                    "metrics",
                    "modules",
                    "purpose",
                    "repetitions",
                    "scenarios",
                    "technical_retries"
                ]
            );
        }
    }

    #[test]
    fn a_dispatched_suite_materializes_like_a_reviewed_one() {
        let plan = embedded().unwrap();
        let suite: Suite = serde_json::from_value(json!({
            "id": "smoke", "scenarios": ["minimal_path", "persistent_state"]
        }))
        .unwrap();
        let snapshot = plan.materialize_suite(suite).unwrap();
        assert_eq!(snapshot.scenario_ids, ["minimal_path", "persistent_state"]);
        assert_eq!(snapshot.profile.lane, "local-smoke");
        assert_eq!(snapshot.profile.label, "smoke");
        assert_eq!(snapshot.profile.repetitions, 1);
        assert_eq!(snapshot.campaigns[0]["campaign_id"], "smoke-r01");

        let reviewed = plan.materialize("pr").unwrap();
        let mut copy = reviewed.profile.clone();
        copy.lane = String::new();
        assert_eq!(
            plan.materialize_suite(copy).unwrap().profile_sha256,
            reviewed.profile_sha256
        );

        for invalid in [
            json!({"id": "smoke", "scenarios": ["local_invented"]}),
            json!({"id": "smoke"}),
            json!({"id": "Not Kebab", "scenarios": ["minimal_path"]}),
        ] {
            let suite: Suite = serde_json::from_value(invalid).unwrap();
            assert!(plan.materialize_suite(suite).is_err());
        }
    }

    #[test]
    fn published_profile_snapshot_schema_matches_the_worker_contract() {
        let schema = serde_json::to_value(schemars::schema_for!(ProfileSnapshot)).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schemas/e2e-profile-snapshot.json");
        if std::env::var_os("UPDATE_PROFILE_SNAPSHOT_SCHEMA").is_some() {
            std::fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&schema).unwrap()),
            )
            .unwrap();
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            schema
        );
    }
}
