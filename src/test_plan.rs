//! One reviewed source for profiles and compatibility campaign snapshots.
//! Materialization is pure: it never contacts iii or calls a model.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::artifact;
use crate::control::{scenarios_list, ScenarioDescriptor, ScenariosListRequest};
use crate::markdown::ScenarioKey;
use crate::scenarios::{ComplexityTier, ScenarioExecutionKind};

const SOURCE: &str = include_str!("../config/test-plan.json");
pub const PROFILE_IDS: [&str; 6] = [
    "smoke",
    "regression",
    "capability",
    "evolution",
    "resilience",
    "endurance",
];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MasterPlan {
    pub schema: String,
    pub plan_id: String,
    pub version: u32,
    pub modules: Vec<CapabilityModule>,
    pub diagnostics: Vec<String>,
    pub requirements: BTreeMap<String, Vec<String>>,
    pub profiles: Vec<Profile>,
    pub compatibility_campaigns: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityModule {
    pub id: String,
    pub label: String,
    pub scenarios: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub purpose: String,
    pub metrics: Vec<String>,
    pub modules: Vec<String>,
    pub scenarios: Vec<String>,
    pub repetitions: u32,
    pub technical_retries: u8,
    pub lane: String,
    pub fault_groups: Vec<FaultGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaultGroup {
    pub id: String,
    pub execution_kind: String,
    pub runs: u32,
    pub technical_retries: u8,
    pub difficulty_weight: u8,
    pub fault_profile: String,
    pub fault_scenario: String,
    pub soak_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanIdentity {
    pub plan_id: String,
    pub version: u32,
    pub definition_sha256: String,
    pub profile_id: String,
    pub profile_sha256: String,
    pub repetition: u32,
    pub repetitions: u32,
    pub campaign_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileSnapshot {
    pub schema: String,
    pub plan_id: String,
    pub version: u32,
    pub definition_sha256: String,
    pub profile_sha256: String,
    pub profile: Profile,
    pub scenario_ids: Vec<String>,
    pub cases: Vec<Value>,
    pub campaigns: Vec<Value>,
    pub budget: Value,
    pub interpretation: String,
    pub protected_supervisor_required: bool,
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

pub fn execution_kind(key: &ScenarioKey) -> &'static str {
    match key.execution_kind() {
        ScenarioExecutionKind::HarnessTurn => "harness_turn",
        ScenarioExecutionKind::ScriptedDialogue => "scripted_dialogue",
        ScenarioExecutionKind::CompositeFlow => "composite_flow",
        ScenarioExecutionKind::AdaptiveFlow => "adaptive_flow",
    }
}

pub(crate) fn weight(tier: ComplexityTier) -> u8 {
    match tier {
        ComplexityTier::L0Atomic | ComplexityTier::L1Sequential => 1,
        ComplexityTier::L2Stateful => 2,
        ComplexityTier::L3Concurrent => 3,
        ComplexityTier::L4Coordinated => 4,
        ComplexityTier::L5Adaptive => 5,
    }
}

fn native_catalog() -> Result<BTreeMap<String, ScenarioDescriptor>> {
    Ok(scenarios_list(ScenariosListRequest { seed: None })?
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
        ensure!(
            self.schema == "harness-e2e-master-test-plan/v1",
            "unsupported master test plan schema"
        );
        ensure!(
            safe_id(&self.plan_id) && self.version > 0,
            "invalid master plan identity"
        );
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
        let native: BTreeSet<_> = crate::markdown::all_keys()?
            .iter()
            .map(ToString::to_string)
            .collect();
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
        let mut profiles = BTreeSet::new();
        for profile in &self.profiles {
            ensure!(
                safe_id(&profile.id) && profiles.insert(profile.id.as_str()),
                "invalid or repeated profile"
            );
            ensure!(
                profile.lane == format!("local-{}", profile.id),
                "profile lane must use the supported local budget"
            );
            ensure!(
                (1..=20).contains(&profile.repetitions) && profile.technical_retries <= 1,
                "invalid profile sample or retries"
            );
            ensure!(
                !profile.metrics.is_empty() && !profile.purpose.trim().is_empty(),
                "profile must declare purpose and metrics"
            );
            let selected = self.select(profile)?;
            ensure!(!selected.is_empty(), "profile has no scenarios");
            ensure!(
                selected.iter().all(|id| native.contains(id)),
                "profile selects an unknown scenario"
            );
            let mut fault_ids = BTreeSet::new();
            for group in &profile.fault_groups {
                ensure!(
                    safe_id(&group.id) && fault_ids.insert(group.id.as_str()),
                    "invalid fault group id"
                );
                let expected = match group.fault_profile.as_str() {
                    "weekly-l2-recovery" => (2, "stateful.2"),
                    "weekly-l3-recovery" => (3, "coordination.3"),
                    "weekly-l4-recovery" => (4, "coordination.4"),
                    _ => bail!("unsupported fault profile"),
                };
                ensure!(
                    group.execution_kind == "fault_injection"
                        && group.technical_retries == 0
                        && (3..=20).contains(&group.runs)
                        && (60..=180).contains(&group.soak_minutes)
                        && group.difficulty_weight == expected.0
                        && group.fault_scenario == expected.1,
                    "invalid recovery policy for {}",
                    group.id
                );
            }
        }
        ensure!(
            profiles == PROFILE_IDS.into_iter().collect(),
            "master plan must declare the six reviewed profiles"
        );
        for (id, manifest) in &self.compatibility_campaigns {
            ensure!(
                safe_id(id) && manifest["campaign_id"] == *id,
                "invalid compatibility campaign"
            );
        }
        Ok(())
    }

    fn select(&self, profile: &Profile) -> Result<Vec<String>> {
        let mut selected = Vec::new();
        let mut seen = BTreeSet::new();
        for module_id in &profile.modules {
            let module = self
                .modules
                .iter()
                .find(|m| &m.id == module_id)
                .context("unknown profile module")?;
            for id in &module.scenarios {
                ensure!(seen.insert(id.clone()), "duplicate profile scenario {id}");
                selected.push(id.clone());
            }
        }
        for id in &profile.scenarios {
            ensure!(seen.insert(id.clone()), "duplicate profile scenario {id}");
            selected.push(id.clone());
        }
        Ok(selected)
    }

    pub fn materialize(&self, id: &str) -> Result<ProfileSnapshot> {
        self.validate()?;
        let profile = self
            .profiles
            .iter()
            .find(|p| p.id == id)
            .context("unknown test profile")?
            .clone();
        let scenario_ids = self.select(&profile)?;
        let native = native_catalog()?;
        let definition_sha256 = self.digest()?;
        let mut cases = Vec::new();
        let mut ordinary_groups = Vec::new();
        let mut subject_turns = 0_u64;
        let mut subject_token_limit = Some(0_u64);
        let mut unbounded_token_cases = Vec::new();
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
                "model": "preview", "provider": "preview", "judge_model": "preview", "judge_provider": "preview",
                "scenarios": [id], "runs": 1, "technical_retries": retries,
            }))?;
            crate::control::validate_run_request(&admission)?;
            let attempts = u64::from(profile.repetitions) * (1 + u64::from(retries));
            let envelope = &case.resource_envelope;
            subject_turns += u64::from(envelope.execution.max_turns) * attempts;
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
                "scenario_id": id, "scenario_version": case.scenario_version, "case_id": case.case_id,
                "seed": case.seed, "inputs_sha256": case.inputs_sha256, "contract_sha256": case.contract_sha256,
                "execution_kind": execution_kind(key), "difficulty_weight": weight(case.classification.tier),
                "resource_envelope": envelope, "required_capabilities": case.required_capabilities,
                "requirements": self.requirements.get(id).cloned().unwrap_or_default(),
                "module": self.modules.iter().find(|m| m.scenarios.contains(id)).map(|m| &m.id),
                "judge_required": key.built_in().is_none(),
            }));
            // Every repetition is a fresh invocation. This also obeys the
            // campaign parser's one-case, runs=1 adaptive-flow contract.
            ordinary_groups.push(json!({
                "id": format!("case-{}", id.replace('_', "-")),
                "execution_kind": execution_kind(key), "runs": 1,
                "technical_retries": retries, "difficulty_weight": weight(case.classification.tier),
                "scenarios": [id],
            }));
        }
        let profile_sha256 = artifact::sha256_value(
            &json!({"definition_sha256": definition_sha256, "profile": profile, "cases": cases}),
        )?;
        let mut campaigns = Vec::new();
        for repetition in 1..=profile.repetitions {
            let mut groups = ordinary_groups.clone();
            groups.extend(
                profile
                    .fault_groups
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
            let mut campaign = json!({
                "kind": "harness-e2e-campaign", "campaign_id": format!("{}-r{repetition:02}", profile.id),
                "lane": profile.lane, "failure_policy": "advisory", "scoring_profile": "difficulty-weighted-v1", "groups": groups,
            });
            let identity = PlanIdentity {
                plan_id: self.plan_id.clone(),
                version: self.version,
                definition_sha256: definition_sha256.clone(),
                profile_id: profile.id.clone(),
                profile_sha256: profile_sha256.clone(),
                repetition,
                repetitions: profile.repetitions,
                campaign_sha256: artifact::sha256_value(&campaign)?,
            };
            campaign["test_plan"] = serde_json::to_value(identity)?;
            campaigns.push(campaign);
        }
        let fault_runs: u64 = profile
            .fault_groups
            .iter()
            .map(|g| u64::from(g.runs))
            .sum::<u64>()
            * u64::from(profile.repetitions);
        Ok(ProfileSnapshot {
            schema: "harness-e2e-profile-snapshot/v1".into(),
            plan_id: self.plan_id.clone(),
            version: self.version,
            definition_sha256,
            profile_sha256,
            profile: profile.clone(),
            scenario_ids: scenario_ids.clone(),
            cases,
            campaigns,
            budget: json!({"scenario_runs": scenario_ids.len() as u64 * u64::from(profile.repetitions), "fault_runs": fault_runs,
                "planned_runs": scenario_ids.len() as u64 * u64::from(profile.repetitions) + fault_runs,
                "session_turn_limit_sum": subject_turns, "subject_token_limit": subject_token_limit,
                "unbounded_token_cases": unbounded_token_cases, "fault_budget_separate": fault_runs > 0,
                "max_concurrent_groups": 1, "scope": "turn sum counts per-session limits, not a whole-workflow ceiling; tokens cover subject only; setup, judge, capture and cleanup are additional"}),
            interpretation: "descriptive_only".into(),
            protected_supervisor_required: !profile.fault_groups.is_empty(),
        })
    }

    pub fn catalog(&self) -> Result<Value> {
        let mut profiles = Vec::new();
        for profile in &self.profiles {
            let snapshot = self.materialize(&profile.id)?;
            profiles.push(json!({"id": profile.id, "label": profile.label, "purpose": profile.purpose, "metrics": profile.metrics,
                "scenario_ids": snapshot.scenario_ids, "repetitions": profile.repetitions,
                "technical_retries": profile.technical_retries, "budget": snapshot.budget,
                "profile_sha256": snapshot.profile_sha256, "protected_supervisor_required": snapshot.protected_supervisor_required,
                "campaigns": snapshot.campaigns,
                "judge_required": snapshot.protected_supervisor_required || snapshot.cases.iter().any(|c| c["judge_required"] == true),
                "cases": snapshot.cases}));
        }
        Ok(
            json!({"plan_id": self.plan_id, "version": self.version, "definition_sha256": self.digest()?, "profiles": profiles}),
        )
    }

    pub fn campaign_catalog(&self) -> Result<Value> {
        let mut scenarios = BTreeMap::new();
        for (id, case) in native_catalog()? {
            scenarios.insert(id, json!({"execution_kind": execution_kind(&case.scenario_id), "difficulty_weight": weight(case.classification.tier), "markdown": case.scenario_id.built_in().is_none()}));
        }
        Ok(
            json!({"schema": "harness-e2e-campaign-catalog/v1", "definition_sha256": self.digest()?, "scenarios": scenarios}),
        )
    }

    fn profile_documentation(&self, catalog: &Value) -> String {
        let mut text = format!("# Generated Harness test profiles\n\nGenerated from [config/test-plan.json](../config/test-plan.json), revision {}.\nEdit the source, then run \u{0060}cargo run --locked -- test-plan sync\u{0060}.\nAll profiles are advisory; repetitions are independent invocations.\n\n| Profile | Cases | Repetitions | Planned slots | Execution |\n| --- | ---: | ---: | ---: | --- |\n", self.version);
        for profile in catalog["profiles"].as_array().expect("catalog profiles") {
            text.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                profile["label"].as_str().unwrap(),
                profile["scenario_ids"].as_array().unwrap().len(),
                profile["repetitions"],
                profile["budget"]["planned_runs"],
                if profile["protected_supervisor_required"] == true {
                    "Protected fault executor"
                } else {
                    "Campaign runner"
                }
            ));
        }
        for profile in &self.profiles {
            text.push_str(&format!("\n## {}\n\n{}\n\nCases: {}.\n\nMeasures: {}.\n\nRetry ceiling: {}; non-replay-safe cases always use zero.\n", profile.label, profile.purpose, self.select(profile).expect("validated profile").iter().map(|id| format!("`{id}`")).collect::<Vec<_>>().join(", "), profile.metrics.iter().map(|id| format!("`{id}`")).collect::<Vec<_>>().join(", "), profile.technical_retries));
            for fault in &profile.fault_groups {
                text.push_str(&format!(
                    "\nFault: `{}` / `{}`, {} repetitions, {} minutes soak.\n",
                    fault.fault_profile, fault.fault_scenario, fault.runs, fault.soak_minutes
                ));
            }
        }
        text.push_str("\n## Capability modules\n\n| Module | Cases |\n| --- | --- |\n");
        for module in &self.modules {
            text.push_str(&format!(
                "| {} — {} | {} |\n",
                module.id,
                module.label,
                module
                    .scenarios
                    .iter()
                    .map(|id| format!("`{id}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        text.push_str(&format!(
            "\nDiagnostic cases: {}.\n",
            self.diagnostics
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        text
    }

    /// Regenerate only owned outputs; check mode is read-only and CI uses it.
    pub fn sync(&self, root: &Path, check: bool) -> Result<()> {
        let mut outputs = BTreeMap::new();
        for (id, campaign) in &self.compatibility_campaigns {
            outputs.insert(
                root.join("config/campaigns").join(format!("{id}.json")),
                campaign.clone(),
            );
        }
        outputs.insert(
            root.join("config/test-plan-catalog.json"),
            self.campaign_catalog()?,
        );
        let catalog = self.catalog()?;
        outputs.insert(root.join("config/test-plan-profiles.json"), catalog.clone());
        let documentation = self.profile_documentation(&catalog);
        let doc_path = root.join("docs/test-profiles.generated.md");
        if check {
            ensure!(
                std::fs::read_to_string(&doc_path).ok().as_deref() == Some(documentation.as_str()),
                "generated profile documentation is stale"
            );
        } else {
            std::fs::create_dir_all(doc_path.parent().context("documentation parent")?)?;
            std::fs::write(doc_path, documentation)?;
        }
        for (path, value) in outputs {
            if check {
                let current: Value = serde_json::from_slice(
                    &std::fs::read(&path)
                        .with_context(|| format!("read generated {}", path.display()))?,
                )?;
                ensure!(
                    current == value,
                    "generated {} is stale; run harness-e2e test-plan sync",
                    path.display()
                );
            } else {
                if std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .as_ref()
                    == Some(&value)
                {
                    continue;
                }
                std::fs::create_dir_all(path.parent().context("generated output parent")?)?;
                std::fs::write(
                    &path,
                    format!("{}\n", serde_json::to_string_pretty(&value)?),
                )?;
            }
        }
        Ok(())
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
                "system_under_test": report.system_under_test, "subject": report.subject,
                "judge": report.judge, "judge_protocol": report.judge_protocol});
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
        json!({"schema": "harness-e2e-profile-measurements/v1", "interpretation": "descriptive_only",
        "cohorts": cohorts, "deferred": deferred, "input_artifacts": paths}),
    )
}

/// Compare all compatible repetitions using the native longitudinal metrics.
/// Cohort mismatches remain visible; this endpoint never emits promotion gates.
pub fn compare_measurements(
    from: &[std::path::PathBuf],
    to: &[std::path::PathBuf],
) -> Result<Value> {
    let (from, from_deferred) = measurement_cohorts(from)?;
    let (to, to_deferred) = measurement_cohorts(to)?;
    let index = |cohorts: &MeasurementCohorts| -> Result<BTreeMap<String, Vec<String>>> {
        let mut families: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (digest, (identity, _)) in cohorts {
            families
                .entry(artifact::sha256_value(&comparison_fixed_identity(
                    identity,
                ))?)
                .or_default()
                .push(digest.clone());
        }
        Ok(families)
    };
    let from_index = index(&from)?;
    let to_index = index(&to)?;
    let keys = from_index
        .keys()
        .chain(to_index.keys())
        .collect::<BTreeSet<_>>();
    let mut comparisons = Vec::new();
    let mut excluded = Vec::new();
    for key in keys {
        let left = from_index.get(key).map(Vec::as_slice).unwrap_or_default();
        let right = to_index.get(key).map(Vec::as_slice).unwrap_or_default();
        if let ([left], [right]) = (left, right) {
            let (left_identity, left_case) = &from[left];
            let (right_identity, right_case) = &to[right];
            comparisons.push(json!({"cohort_sha256": key, "scenario_id": left_case.scenario_id,
                "from_cohort_sha256": left, "to_cohort_sha256": right, "from_identity": left_identity, "to_identity": right_identity,
                "metrics": crate::longitudinal::compare_case_descriptive(left_case, right_case)}));
        } else {
            for (side, ids, cohorts) in [("baseline", left, &from), ("candidate", right, &to)] {
                for id in ids {
                    excluded.push(json!({"cohort_sha256": id, "side": side, "identity": cohorts[id].0,
                    "reason": "Case, model, runner revision or fixed execution contract has no unique compatible cohort on the other side."}));
                }
            }
        }
    }
    Ok(
        json!({"schema": "harness-e2e-profile-comparison/v1", "interpretation": "descriptive_only",
        "treatment": "Harness version and Workers source revision; all other recorded identities remain fixed. Results do not establish causality or promotion gates.",
        "comparisons": comparisons, "excluded": excluded, "baseline_deferred": from_deferred, "candidate_deferred": to_deferred}),
    )
}

fn comparison_fixed_identity(identity: &Value) -> Value {
    let mut fixed = identity.clone();
    if let Some(system) = fixed["system_under_test"].as_object_mut() {
        system.remove("harness_version");
        if let Some(stack) = system.get_mut("stack").and_then(Value::as_object_mut) {
            if stack.get("mode").and_then(Value::as_str) == Some("source") {
                stack.remove("workers_revision");
            } else if let Some(versions) = stack
                .get_mut("stack_versions")
                .and_then(Value::as_object_mut)
            {
                versions.remove("harness");
                stack.remove("stack_lock_digest");
            }
        }
    }
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evolution_only_pairs_declared_treatment_changes_and_retains_fixed_identity() {
        let identity = json!({"case": {"version": 1}, "subject": {"model": "subject"}, "judge": {"model": "judge"},
            "system_under_test": {"harness_version": "1", "engine_version": "1", "e2e_revision": "runner-1", "contract_hashes": {"send": "contract"}, "stack": {"mode": "source", "workers_repository": "workers", "workers_revision": "a"}}});
        let mut candidate = identity.clone();
        candidate["system_under_test"]["harness_version"] = json!("2");
        candidate["system_under_test"]["stack"]["workers_revision"] = json!("b");
        assert_eq!(
            comparison_fixed_identity(&identity),
            comparison_fixed_identity(&candidate)
        );
        for path in [
            vec!["subject", "model"],
            vec!["case", "version"],
            vec!["system_under_test", "e2e_revision"],
            vec!["system_under_test", "engine_version"],
            vec!["system_under_test", "contract_hashes", "send"],
        ] {
            let mut changed = candidate.clone();
            let mut field = &mut changed;
            for key in path {
                field = &mut field[key];
            }
            *field = json!("incompatible");
            assert_ne!(
                comparison_fixed_identity(&identity),
                comparison_fixed_identity(&changed)
            );
        }
    }

    #[test]
    fn profile_samples_preserve_independent_execution_and_retry_boundaries() {
        let plan = embedded().unwrap();
        for (id, cases, runs) in [
            ("smoke", 5, 5),
            ("regression", 12, 12),
            ("capability", 47, 47),
            ("evolution", 18, 90),
            ("resilience", 4, 13),
            ("endurance", 5, 5),
        ] {
            let snapshot = plan.materialize(id).unwrap();
            assert_eq!(snapshot.scenario_ids.len(), cases);
            assert_eq!(snapshot.budget["planned_runs"], runs);
            let mut rounds = BTreeSet::new();
            for campaign in snapshot.campaigns {
                assert!(rounds.insert(campaign["campaign_id"].as_str().unwrap().to_string()));
                let mut native = campaign.clone();
                let identity: PlanIdentity = serde_json::from_value(
                    native.as_object_mut().unwrap().remove("test_plan").unwrap(),
                )
                .unwrap();
                assert_eq!(
                    identity.campaign_sha256,
                    artifact::sha256_value(&native).unwrap()
                );
                let groups = campaign["groups"].as_array().unwrap();
                let mut selected = BTreeSet::new();
                for group in groups {
                    if group["execution_kind"] == "fault_injection" {
                        assert_eq!(group["technical_retries"], 0);
                        assert_eq!(group["soak_minutes"], 60);
                    } else {
                        assert_eq!(group["runs"], 1);
                        let id = group["scenarios"][0].as_str().unwrap();
                        assert!(selected.insert(id));
                        if !id
                            .parse::<ScenarioKey>()
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
    fn source_rejects_lost_coverage_overlap_invalid_samples_and_fault_policy() {
        let plan = embedded().unwrap();
        let mut changed = plan.clone();
        changed.modules[0].scenarios.pop();
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.profiles[0].scenarios.push("minimal_path".into());
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.profiles[3].repetitions = 21;
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.profiles[4].fault_groups[0].technical_retries = 1;
        assert!(changed.validate().is_err());
        let mut changed = plan.clone();
        changed.profiles[0].scenarios[0] = "local_invented".into();
        assert!(changed.validate().is_err());
    }

    #[test]
    fn materialized_scope_changes_identity_and_never_invents_budget() {
        let plan = embedded().unwrap();
        let first = plan.materialize("evolution").unwrap();
        assert_eq!(
            first.profile_sha256,
            plan.materialize("evolution").unwrap().profile_sha256
        );
        let mut changed = plan;
        changed.profiles[3].repetitions = 6;
        assert_ne!(
            first.profile_sha256,
            changed.materialize("evolution").unwrap().profile_sha256
        );
        assert!(first.budget["subject_token_limit"].is_null());
        assert!(!first.budget["unbounded_token_cases"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn generated_assets_are_reproducible_and_drift_is_rejected() {
        let plan = embedded().unwrap();
        let root = tempfile::tempdir().unwrap();
        plan.sync(root.path(), false).unwrap();
        plan.sync(root.path(), true).unwrap();
        let daily = root.path().join("config/campaigns/daily.json");
        let mut value: Value = serde_json::from_slice(&std::fs::read(&daily).unwrap()).unwrap();
        value["groups"][0]["technical_retries"] = json!(0);
        std::fs::write(daily, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(plan.sync(root.path(), true).is_err());
    }
}
