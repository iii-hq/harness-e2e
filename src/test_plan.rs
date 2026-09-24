//! One reviewed source for test-plan templates.
//! Materialization is pure: it never contacts iii or calls a model.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::artifact;
use crate::control::{scenarios_list, ScenarioDescriptor, ScenariosListRequest};
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
}

fn one_repetition() -> u32 {
    1
}

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
        })
    }

    pub fn catalog(&self) -> Result<Value> {
        let mut profiles = Vec::new();
        for profile in &self.suites {
            let snapshot = self.materialize(&profile.id)?;
            profiles.push(json!({"id": profile.id, "label": profile.label, "purpose": profile.purpose, "metrics": profile.metrics,
                "scenario_ids": snapshot.scenario_ids, "repetitions": profile.repetitions,
                "technical_retries": profile.technical_retries, "budget": snapshot.budget,
                "profile_sha256": snapshot.profile_sha256,
                "cases": snapshot.cases}));
        }
        Ok(
            json!({"plan_id": self.plan_id, "definition_sha256": self.digest()?, "profiles": profiles}),
        )
    }

    pub fn campaign_catalog(&self) -> Result<Value> {
        let mut scenarios = BTreeMap::new();
        for (id, case) in native_catalog(None)? {
            scenarios.insert(
                id,
                json!({"execution_kind": execution_kind(&case.scenario_id)}),
            );
        }
        Ok(
            json!({"schema": "harness-e2e-campaign-catalog", "definition_sha256": self.digest()?, "scenarios": scenarios}),
        )
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
        assert_eq!(plan.suites.len(), 4);
        for (id, cases, runs) in [
            ("regression", 9, 9),
            ("software-engineering", 15, 15),
            ("pr", 4, 4),
            ("after-release", 5, 5),
        ] {
            let snapshot = plan.materialize(id).unwrap();
            assert_eq!(snapshot.scenario_ids.len(), cases);
            assert_eq!(snapshot.budget["planned_runs"], runs);
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
    fn software_engineering_profile_includes_trending_topics_and_linkly_independently() {
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
                "linkly_tutorial",
                "alertmanager_route_match",
                "chess_engine_build",
                "form_flow_build",
                "state_machine_canvas_build",
            ])
            .collect::<Vec<_>>();
        assert_eq!(snapshot.scenario_ids, expected);
        let groups = snapshot.campaigns[0]["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 14);
        let linkly = groups
            .iter()
            .find(|g| g["id"] == "case-linkly-tutorial")
            .unwrap();
        assert_eq!(linkly["scenarios"], json!(["linkly_tutorial"]));
        assert_eq!(linkly["execution_kind"], "scripted_dialogue");
        assert_eq!(linkly["runs"], 1);
        assert_eq!(linkly["technical_retries"], 0);
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
    fn software_engineering_orders_registry_delivery_and_verification_in_one_group() {
        let plan = embedded().unwrap();
        let snapshot = plan.materialize("software-engineering").unwrap();
        let groups = snapshot.campaigns[0]["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 14);
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
        assert_eq!(snapshot.cases.len(), 15);
        assert_eq!(snapshot.budget["planned_runs"], 15);

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
