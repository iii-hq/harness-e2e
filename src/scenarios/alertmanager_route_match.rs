//! Refactor Alertmanager route matching onto an iii function.
//!
//! The subject clones a pinned `prometheus/alertmanager` bundle and moves
//! `dispatch.Route.Match` onto `route::match`. The runner scores that function
//! by calling it on the iii stack with the frozen route trees and label sets
//! from `TestRouteMatch` and `conf.good.yml`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::artifact::sha256_bytes;
use crate::context::E2eContext;
use crate::report::EvaluationDimension;

use super::assessment::{self, AssessmentSpec};
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedInvariant,
    DeliverableContract, ExecutionPolicy, ExecutionRealism, HumanHorizon, InvariantSpec,
    ObjectiveEvaluation, ProvenanceEvidence, Scenario, ScenarioCase, ScenarioCharacterization,
    ScenarioObservation, ScenarioSpec, ShadowMode,
};

pub const ID: &str = "alertmanager_route_match";
pub const CANONICAL_SEED: u64 = 0x616c_7274_0001;
pub const SUMMARY: &str = "Clone the pinned prometheus/alertmanager revision and refactor \
route matching so the iii function route::match decides receivers and group-by labels. \
The runner calls that function on the iii stack and scores the receivers it returns \
against a frozen oracle from TestRouteMatch and conf.good.yml.";

const UPSTREAM_REPOSITORY: &str = "prometheus/alertmanager";
const UPSTREAM_URL: &str = "https://github.com/prometheus/alertmanager";
const UPSTREAM_TAG: &str = "v0.34.1";
const PINNED_REVISION: &str = "73c6bfe7393929211294c1954f30d8ed78e4d0ad";
const PINNED_TREE: &str = "285e2549c75984bd4b0a6fddebf8a045dd6a76b6";
const FUNCTION_ID: &str = "route::match";
const DELIVERABLE_ID: &str = "route_match_audit";
const BUNDLE_RELATIVE_PATH: &str = "input/repository.bundle";
const MANIFEST_RELATIVE_PATH: &str = "input/case.json";
const CHECKOUT_RELATIVE_PATH: &str = "checkout";
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const PROTECTED_PATHS: &[&str] = &["notify", "api", "config/testdata"];

const PUBLIC_MANIFEST: &str =
    include_str!("../../tests/fixtures/alertmanager-route-match/manifest.json");
const ORACLE_JSON: &str = include_str!("../../tests/fixtures/alertmanager-route-match/oracle.json");
const BUNDLE_BYTES: &[u8] =
    include_bytes!("../../tests/fixtures/alertmanager-route-match/repository.bundle");

const REVISION_PINNED: AssessmentSpec = AssessmentSpec::scored(
    "revision_pinned",
    10,
    "The checkout is the pinned bundle revision or a descendant of it.",
);
const MATCH_EQUIVALENT: AssessmentSpec = AssessmentSpec::scored_in(
    "match_equivalent",
    80,
    "Each live call to route::match returns the frozen receivers and group-by labels.",
    EvaluationDimension::Deliverable,
);
const SCOPE_EXACT: AssessmentSpec = AssessmentSpec::scored(
    "scope_exact",
    10,
    "notify/, api/, and config/testdata/ stay identical to the pinned tree.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[REVISION_PINNED, MATCH_EQUIVALENT, SCOPE_EXACT];

#[derive(Debug, Deserialize)]
struct Oracle {
    trees: OracleTrees,
    cases: Vec<OracleCase>,
}

#[derive(Debug, Deserialize)]
struct OracleTrees {
    route_test: String,
    conf_good: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OracleCase {
    id: String,
    tree: String,
    labels: BTreeMap<String, String>,
    receivers: Vec<String>,
    group_by: Vec<Vec<String>>,
    group_by_all: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchResult {
    receivers: Vec<String>,
    group_by: Vec<Vec<String>>,
    group_by_all: Vec<bool>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    checkout_is_repository: bool,
    head: Option<String>,
    pinned_reachable: bool,
    protected_unchanged: bool,
    function_registered: bool,
    match_cases: Vec<(String, bool, String)>,
}

impl Snapshot {
    fn revision_pinned(&self) -> bool {
        self.checkout_is_repository && self.pinned_reachable
    }

    fn match_equivalent(&self) -> bool {
        self.function_registered
            && !self.match_cases.is_empty()
            && self.match_cases.iter().all(|(_, passed, _)| *passed)
    }

    /// The task is done once `route::match` answers with a match payload.
    /// Exact agreement with the oracle stays in the score.
    fn task_completed(&self) -> bool {
        self.function_registered
            && self
                .match_cases
                .iter()
                .any(|(_, passed, reason)| call_answered(*passed, reason))
    }

    fn match_awarded(&self) -> u8 {
        let total = self.match_cases.len();
        if !self.function_registered || total == 0 {
            return 0;
        }
        let passed = self
            .match_cases
            .iter()
            .filter(|(_, passed, _)| *passed)
            .count();
        u8::try_from(passed * usize::from(MATCH_EQUIVALENT.weight()) / total).unwrap_or(0)
    }

    fn scope_exact(&self) -> bool {
        self.checkout_is_repository && self.protected_unchanged
    }
}

pub struct AlertmanagerRouteMatch;

#[async_trait]
impl Scenario for AlertmanagerRouteMatch {
    fn id(&self) -> &'static str {
        ID
    }

    fn summary(&self) -> Option<&'static str> {
        Some(SUMMARY)
    }

    fn canonical_seed(&self) -> u64 {
        CANONICAL_SEED
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn characterization(&self) -> Result<ScenarioCharacterization> {
        ScenarioCharacterization::new(
            HumanHorizon::author_estimate(180, 360)?,
            ExecutionRealism::FrozenRealArtifact,
            ShadowMode::None,
        )
    }

    fn case(&self, _seed: u64) -> Result<ScenarioCase> {
        let oracle = oracle()?;
        ScenarioCase::new(
            ID,
            CANONICAL_SEED,
            json!({
                "fixture_repository": UPSTREAM_REPOSITORY,
                "fixture_url": UPSTREAM_URL,
                "fixture_tag": UPSTREAM_TAG,
                "fixture_revision": PINNED_REVISION,
                "fixture_tree": PINNED_TREE,
                "bundle_sha256": sha256_bytes(BUNDLE_BYTES),
                "bundle_size_bytes": BUNDLE_BYTES.len() as u64,
                "oracle_sha256": sha256_bytes(ORACLE_JSON.as_bytes()),
                "oracle_cases": oracle.cases.len(),
                "function_id": FUNCTION_ID,
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiShell,
                Capability::GitOfflineBundle,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let root = workspace_root(run_id);
        let bundle = root.join(BUNDLE_RELATIVE_PATH);
        let checkout = root.join(CHECKOUT_RELATIVE_PATH);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                r#"Download prometheus/alertmanager at the pinned revision by cloning the local
bundle `{bundle}` into `{checkout}`. The upstream URL {url} is provenance; do not fetch another
remote or another commit.

Refactor route matching so the iii function `{function}` decides, for one
alert label set and the existing YAML route tree, the receivers and the
group-by labels. Call that function from `dispatch.Route.Match`. Keep the YAML
schema and the Go method signatures.

`{function}` must accept JSON `{{"route": "<yaml>", "labels": {{"<name>": "<value>"}}}}`
and return JSON `{{"receivers": ["<name>"], "group_by": [["<label>"]], "group_by_all": [false]}}`
in match order, including every route that `continue` keeps.

Leave grouping timers, silences, notification, webhook payloads, and the HTTP
API unchanged. Do not edit `config/testdata/`, `notify/`, or `api/`.

The runner scores this by calling `{function}` on the iii stack. For each
case it sends the route YAML and one label set, then compares the receivers
and group-by labels in the JSON you return. It does not start Alertmanager."#,
                bundle = bundle.display(),
                checkout = checkout.display(),
                url = UPSTREAM_URL,
                function = FUNCTION_ID,
            ),
            filesystem_root: Some(root),
            execution: ExecutionPolicy {
                max_turns: 48,
                max_output_tokens: Some(16_384),
                max_total_tokens: Some(800_000),
                stuck_timeout_seconds: 1_800,
                max_validation_retries: None,
            },
            denied_functions: &["web::*", "scrapling::*", "http::*"],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, _context: &E2eContext, run_id: &str) -> Result<()> {
        prepare_workspace(&workspace_root(run_id)).await
    }

    async fn capture(
        &self,
        context: &E2eContext,
        _observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let snapshot = collect_snapshot(context, &workspace_root(run_id)).await?;
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "alertmanager_route_match_audit".to_string(),
            content: audit_content(&snapshot).into(),
            invariants: vec![
                CapturedInvariant {
                    id: "revision_pinned".to_string(),
                    passed: snapshot.revision_pinned(),
                    reason: revision_reason(&snapshot),
                },
                CapturedInvariant {
                    id: "match_equivalent".to_string(),
                    passed: snapshot.match_equivalent(),
                    reason: match_reason(&snapshot),
                },
                CapturedInvariant {
                    id: "scope_exact".to_string(),
                    passed: snapshot.scope_exact(),
                    reason: scope_reason(&snapshot),
                },
            ],
            provenance: vec![ProvenanceEvidence {
                kind: "git_repository".to_string(),
                source_id: format!("{UPSTREAM_REPOSITORY}@{PINNED_REVISION}"),
                relation: "materialized_from_verified_bundle".to_string(),
            }],
        }])
    }

    async fn evaluate(
        &self,
        context: &E2eContext,
        _observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        let snapshot = collect_snapshot(context, &workspace_root(run_id)).await?;
        Ok(assessment::build_evaluation(
            if snapshot.task_completed() {
                crate::report::CompletionState::Completed
            } else {
                crate::report::CompletionState::TaskIncomplete
            },
            [
                REVISION_PINNED
                    .full_or_zero(snapshot.revision_pinned(), revision_reason(&snapshot)),
                MATCH_EQUIVALENT.award(snapshot.match_awarded(), match_reason(&snapshot))?,
                SCOPE_EXACT.full_or_zero(snapshot.scope_exact(), scope_reason(&snapshot)),
            ],
        ))
    }

    async fn cleanup(&self, _context: &E2eContext, run_id: &str) -> Result<()> {
        remove_directory(&workspace_root(run_id))
    }
}

fn oracle() -> Result<Oracle> {
    serde_json::from_str(ORACLE_JSON).context("decode embedded Alertmanager route-match oracle")
}

#[cfg(test)]
fn oracle_answer(payload: &Value) -> Option<Value> {
    let oracle = oracle().ok()?;
    let route = payload.get("route")?.as_str()?;
    let tree = if route == oracle.trees.route_test {
        "route_test"
    } else if route == oracle.trees.conf_good {
        "conf_good"
    } else {
        return None;
    };
    let incoming: BTreeMap<String, String> =
        serde_json::from_value(payload.get("labels").cloned().unwrap_or_else(|| json!({}))).ok()?;
    let case = oracle
        .cases
        .iter()
        .find(|case| case.tree == tree && case.labels == incoming)?;
    Some(json!({
        "receivers": case.receivers,
        "group_by": case.group_by,
        "group_by_all": case.group_by_all,
    }))
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "alertmanager_route_match_audit".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": [
                    "revision",
                    "match",
                    "scope"
                ],
                "properties": {
                    "revision": {"type": "object"},
                    "match": {"type": "object"},
                    "scope": {"type": "object"}
                },
                "additionalProperties": false
            }),
            max_size_bytes: 64 * 1024,
        }],
        invariants: vec![
            InvariantSpec {
                id: "revision_pinned".to_string(),
                description: "The checkout stays on the pinned Alertmanager revision.".to_string(),
            },
            InvariantSpec {
                id: "match_equivalent".to_string(),
                description: "Live route::match calls agree with the frozen oracle.".to_string(),
            },
            InvariantSpec {
                id: "scope_exact".to_string(),
                description: "Protected Alertmanager trees stay exact.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

async fn prepare_workspace(root: &Path) -> Result<()> {
    validate_public_manifest()?;
    let input = root.join("input");
    fs::create_dir_all(&input).with_context(|| format!("create {}", input.display()))?;
    write_exact(&root.join(BUNDLE_RELATIVE_PATH), BUNDLE_BYTES)?;
    write_exact(
        &root.join(MANIFEST_RELATIVE_PATH),
        PUBLIC_MANIFEST.as_bytes(),
    )?;
    let preflight = root.join(".fixture-preflight");
    remove_directory(&preflight)?;
    let validation = validate_bundle(&root.join(BUNDLE_RELATIVE_PATH), &preflight).await;
    let cleanup = remove_directory(&preflight);
    match (validation, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error).context("remove fixture preflight workspace"),
        (Err(error), Err(cleanup_error)) => {
            Err(error.context(format!("also failed to remove preflight: {cleanup_error}")))
        }
    }
}

fn validate_public_manifest() -> Result<()> {
    let manifest: Value =
        serde_json::from_str(PUBLIC_MANIFEST).context("decode embedded public case manifest")?;
    if manifest
        .pointer("/history/revision")
        .and_then(Value::as_str)
        != Some(PINNED_REVISION)
    {
        bail!("public manifest revision does not match the pinned commit");
    }
    if manifest.pointer("/bundle/sha256").and_then(Value::as_str)
        != Some(&sha256_bytes(BUNDLE_BYTES))
    {
        bail!("embedded Git bundle SHA-256 differs from public manifest");
    }
    let size = manifest
        .pointer("/bundle/size_bytes")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if size != BUNDLE_BYTES.len() as u64 {
        bail!("embedded Git bundle size differs from public manifest");
    }
    Ok(())
}

async fn validate_bundle(bundle: &Path, checkout: &Path) -> Result<()> {
    let bundle_arg = bundle.display().to_string();
    let checkout_arg = checkout.display().to_string();
    git(
        bundle.parent().unwrap_or(Path::new(".")),
        &["clone", "--no-hardlinks", &bundle_arg, &checkout_arg],
    )
    .await?;
    let head = git(checkout, &["rev-parse", "HEAD"]).await?;
    if head != PINNED_REVISION {
        bail!("bundle HEAD {head} is not the pinned revision {PINNED_REVISION}");
    }
    let tree = git(checkout, &["rev-parse", "HEAD^{tree}"]).await?;
    if tree != PINNED_TREE {
        bail!("bundle tree {tree} is not the pinned tree {PINNED_TREE}");
    }
    Ok(())
}

async fn collect_snapshot(context: &E2eContext, root: &Path) -> Result<Snapshot> {
    let checkout = root.join(CHECKOUT_RELATIVE_PATH);
    let checkout_is_repository = checkout.join(".git").exists();
    let head = if checkout_is_repository {
        git_optional(&checkout, &["rev-parse", "HEAD"]).await
    } else {
        None
    };
    let pinned_reachable =
        checkout_is_repository && revision_is_pinned(&checkout, head.as_deref()).await;
    let protected_unchanged = checkout_is_repository && {
        let mut args = vec!["--"];
        args.extend(PROTECTED_PATHS.iter().copied());
        git_diff_empty(&checkout, PINNED_REVISION, &args).await
    };
    let function_registered = context.function_exists(FUNCTION_ID).await.unwrap_or(false);
    let match_cases = if function_registered {
        probe_match_cases(context).await
    } else {
        Vec::new()
    };
    Ok(Snapshot {
        checkout_is_repository,
        head,
        pinned_reachable,
        protected_unchanged,
        function_registered,
        match_cases,
    })
}

async fn probe_match_cases(context: &E2eContext) -> Vec<(String, bool, String)> {
    let Ok(oracle) = oracle() else {
        return vec![("oracle".into(), false, "embedded oracle is invalid".into())];
    };
    let mut results = Vec::new();
    for case in oracle.cases {
        let route = match case.tree.as_str() {
            "route_test" => oracle.trees.route_test.as_str(),
            "conf_good" => oracle.trees.conf_good.as_str(),
            other => {
                results.push((case.id, false, format!("unknown oracle tree {other}")));
                continue;
            }
        };
        let payload = json!({
            "route": route,
            "labels": case.labels,
        });
        match context.trigger_value(FUNCTION_ID, payload).await {
            Ok(value) => match parse_match_result(&value) {
                Some(observed) if equivalent(&expected_result(&case), &observed) => {
                    results.push((case.id, true, "oracle match".into()));
                }
                Some(observed) => results.push((
                    case.id,
                    false,
                    format!(
                        "receivers {:?} group_by {:?} group_by_all {:?}",
                        observed.receivers, observed.group_by, observed.group_by_all
                    ),
                )),
                None => results.push((case.id, false, format!("unrecognized payload {value}"))),
            },
            Err(error) => results.push((case.id, false, format!("{error:#}"))),
        }
    }
    results
}

fn expected_result(case: &OracleCase) -> MatchResult {
    MatchResult {
        receivers: case.receivers.clone(),
        group_by: normalized_groups(&case.group_by),
        group_by_all: case.group_by_all.clone(),
    }
}

fn equivalent(expected: &MatchResult, observed: &MatchResult) -> bool {
    expected.receivers == observed.receivers
        && normalized_groups(&expected.group_by) == normalized_groups(&observed.group_by)
        && expected.group_by_all == observed.group_by_all
}

fn normalized_groups(groups: &[Vec<String>]) -> Vec<Vec<String>> {
    groups
        .iter()
        .map(|group| {
            let mut labels = group.clone();
            labels.sort();
            labels
        })
        .collect()
}

fn parse_match_result(value: &Value) -> Option<MatchResult> {
    let value = value
        .get("result")
        .or_else(|| value.get("output"))
        .or_else(|| value.get("data"))
        .unwrap_or(value);
    if let Some(matches) = value.get("matches").and_then(Value::as_array) {
        let mut receivers = Vec::new();
        let mut group_by = Vec::new();
        let mut group_by_all = Vec::new();
        for entry in matches {
            receivers.push(entry.get("receiver")?.as_str()?.to_string());
            group_by.push(string_list(entry.get("group_by")?));
            group_by_all.push(
                entry
                    .get("group_by_all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            );
        }
        return Some(MatchResult {
            receivers,
            group_by: normalized_groups(&group_by),
            group_by_all,
        });
    }
    let receivers = value
        .get("receivers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })?;
    let group_by = value.get("group_by").map(group_by_lists)?;
    let group_by_all = value
        .get("group_by_all")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_bool).collect::<Vec<_>>())
        .unwrap_or_else(|| vec![false; receivers.len()]);
    Some(MatchResult {
        receivers,
        group_by: normalized_groups(&group_by),
        group_by_all,
    })
}

fn group_by_lists(value: &Value) -> Vec<Vec<String>> {
    match value {
        Value::Array(items) if items.first().is_some_and(Value::is_array) => {
            items.iter().map(string_list_ref).collect()
        }
        Value::Array(_) => vec![string_list(value)],
        _ => Vec::new(),
    }
}

fn string_list(value: &Value) -> Vec<String> {
    string_list_ref(value)
}

fn string_list_ref(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn audit_content(snapshot: &Snapshot) -> Value {
    json!({
        "revision": {
            "head": snapshot.head,
            "pinned": PINNED_REVISION,
            "reachable": snapshot.pinned_reachable,
        },
        "match": {
            "registered": snapshot.function_registered,
            "awarded": snapshot.match_awarded(),
            "cases": snapshot.match_cases.iter().map(|(id, passed, reason)| {
                json!({"id": id, "passed": passed, "reason": reason})
            }).collect::<Vec<_>>(),
        },
        "scope": {
            "protected_unchanged": snapshot.protected_unchanged,
        }
    })
}

fn revision_reason(snapshot: &Snapshot) -> String {
    if snapshot.head.as_deref() == Some(PINNED_REVISION) {
        format!("checkout HEAD is {PINNED_REVISION}")
    } else if snapshot.revision_pinned() {
        format!(
            "checkout HEAD {:?} descends from {PINNED_REVISION}",
            snapshot.head
        )
    } else if !snapshot.checkout_is_repository {
        "checkout is not a Git repository".into()
    } else {
        format!(
            "pinned revision {PINNED_REVISION} is not an ancestor of HEAD {:?}",
            snapshot.head
        )
    }
}

fn call_answered(passed: bool, reason: &str) -> bool {
    passed || reason.starts_with("receivers ")
}

fn match_reason(snapshot: &Snapshot) -> String {
    let passed = snapshot
        .match_cases
        .iter()
        .filter(|(_, passed, _)| *passed)
        .count();
    if snapshot.match_equivalent() {
        format!("route::match agreed on {passed} live calls")
    } else if !snapshot.function_registered {
        "route::match is not registered".into()
    } else if snapshot.match_cases.is_empty() {
        "route::match returned no cases".into()
    } else {
        let failures = snapshot
            .match_cases
            .iter()
            .filter(|(_, passed, _)| !*passed)
            .map(|(id, _, reason)| format!("{id}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "route::match agreed on {passed} of {} live calls; {failures}",
            snapshot.match_cases.len()
        )
    }
}

fn scope_reason(snapshot: &Snapshot) -> String {
    if snapshot.scope_exact() {
        "notify/, api/, and config/testdata/ match the pinned tree".into()
    } else {
        "protected Alertmanager paths changed".into()
    }
}

/// The pinned bundle is depth 1, so `merge-base --is-ancestor` cannot read the
/// missing parent even when HEAD is that commit. Equality is pinned, and a
/// later commit is pinned when `rev-list --ancestry-path` can see it.
async fn revision_is_pinned(checkout: &Path, head: Option<&str>) -> bool {
    if head == Some(PINNED_REVISION) {
        return true;
    }
    git(
        checkout,
        &[
            "rev-list",
            "--ancestry-path",
            &format!("{PINNED_REVISION}..HEAD"),
        ],
    )
    .await
    .is_ok_and(|output| output.lines().any(|line| !line.is_empty()))
}

async fn git_diff_empty(cwd: &Path, revision: &str, extra: &[&str]) -> bool {
    let mut args = vec!["diff", "--quiet", revision];
    args.extend(extra.iter().copied());
    git_succeeds(cwd, &args).await
}

async fn git_optional(cwd: &Path, args: &[&str]) -> Option<String> {
    git(cwd, args).await.ok()
}

async fn git_succeeds(cwd: &Path, args: &[&str]) -> bool {
    git(cwd, args).await.is_ok()
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))
        .env("GIT_CONFIG_SYSTEM", OsStr::new("/dev/null"))
        .env("GIT_TERMINAL_PROMPT", OsStr::new("0"))
        .stdin(Stdio::null())
        .output();
    let output = tokio::time::timeout(GIT_TIMEOUT, output)
        .await
        .with_context(|| format!("git {} timed out", args.join(" ")))?
        .with_context(|| format!("run git {} in {}", args.join(" "), cwd.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn write_exact(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        let existing = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        if existing != bytes {
            bail!(
                "refuse to replace unexpected fixture file {}",
                path.display()
            );
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

fn remove_directory(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn workspace_root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = fs::canonicalize(&base).unwrap_or(base);
    base.join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::scenarios::{ScenarioId, ScenarioObservation};

    fn valid_snapshot() -> Snapshot {
        Snapshot {
            checkout_is_repository: true,
            head: Some(PINNED_REVISION.to_string()),
            pinned_reachable: true,
            protected_unchanged: true,
            function_registered: true,
            match_cases: vec![(
                "route_test_owner-team-A".into(),
                true,
                "oracle match".into(),
            )],
        }
    }

    #[test]
    fn scenario_and_materialization_validate() {
        AlertmanagerRouteMatch
            .spec("alertmanager-route-test")
            .validate()
            .unwrap();
        ScenarioId::AlertmanagerRouteMatch
            .materialize("alertmanager-route-test", CANONICAL_SEED)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn prompt_states_pinned_clone_and_route_match_contract() {
        let spec = AlertmanagerRouteMatch.spec("attempt");
        assert!(spec.prompt.contains("prometheus/alertmanager"));
        assert!(spec.prompt.contains(FUNCTION_ID));
        assert!(spec.prompt.contains("dispatch.Route.Match"));
        assert!(spec
            .prompt
            .contains("calling `route::match` on the iii stack"));
        assert!(spec.prompt.contains("does not start Alertmanager"));
        assert!(!spec.prompt.contains("go test"));
        assert!(spec.prompt.contains("do not fetch another"));
        assert!(spec.prompt.contains("input/repository.bundle"));
        assert_eq!(spec.criteria.iter().map(|c| c.weight).sum::<u8>(), 100);
        assert_eq!(
            spec.criteria.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec!["revision_pinned", "match_equivalent", "scope_exact"]
        );
    }

    #[test]
    fn public_manifest_matches_embedded_bundle() {
        validate_public_manifest().unwrap();
        let case = ScenarioId::AlertmanagerRouteMatch
            .materialize("alertmanager-route-test", CANONICAL_SEED)
            .unwrap()
            .case;
        assert_eq!(case.inputs["fixture_revision"], json!(PINNED_REVISION));
        assert_eq!(
            case.inputs["bundle_sha256"],
            json!(sha256_bytes(BUNDLE_BYTES))
        );
        assert_eq!(case.inputs["oracle_cases"], json!(18));
        assert_eq!(
            case.characterization.realism.execution,
            ExecutionRealism::FrozenRealArtifact
        );
        assert_eq!(case.characterization.human_horizon.min_minutes, Some(180));
    }

    #[test]
    fn oracle_covers_route_test_and_conf_good() {
        let oracle = oracle().unwrap();
        assert!(oracle.trees.route_test.contains("notify-productionA"));
        assert!(oracle.trees.conf_good.contains("team-DB-pager"));
        let production = oracle
            .cases
            .iter()
            .find(|case| case.id == "route_test_owner-team-A_env-production")
            .unwrap();
        assert_eq!(
            production.receivers,
            vec!["notify-productionA", "notify-productionB"]
        );
        let continued = oracle
            .cases
            .iter()
            .find(|case| case.id == "service_database_continue")
            .unwrap();
        assert_eq!(continued.receivers, vec!["team-X-pager", "team-Y-pager"]);
        assert_eq!(oracle.cases.len(), 18);
    }

    #[test]
    fn match_parser_accepts_matches_array_and_sorted_group_by() {
        let parsed = parse_match_result(&json!({
            "matches": [
                {"receiver": "notify-BC", "group_by": ["bar", "foo"], "group_by_all": false}
            ]
        }))
        .unwrap();
        assert!(equivalent(
            &MatchResult {
                receivers: vec!["notify-BC".into()],
                group_by: vec![vec!["foo".into(), "bar".into()]],
                group_by_all: vec![false],
            },
            &parsed
        ));
    }

    #[test]
    fn live_calls_score_partially_and_scope_stays_separate() {
        let mut snapshot = valid_snapshot();
        assert!(snapshot.match_equivalent());
        assert_eq!(snapshot.match_awarded(), MATCH_EQUIVALENT.weight());
        assert!(snapshot.scope_exact());
        snapshot.match_cases.push((
            "service_database_continue".into(),
            false,
            "receivers [\"team-X-pager\"]".into(),
        ));
        assert!(!snapshot.match_equivalent());
        assert!(snapshot.task_completed());
        assert_eq!(snapshot.match_awarded(), 40);
        assert!(match_reason(&snapshot).contains("1 of 2 live calls"));
        snapshot.match_cases = vec![(
            "service_database_continue".into(),
            false,
            "receivers [\"team-X-pager\"]".into(),
        )];
        assert!(snapshot.task_completed());
        assert_eq!(snapshot.match_awarded(), 0);
        snapshot.match_cases = vec![(
            "route_test_owner-team-A".into(),
            false,
            "trigger failed".into(),
        )];
        assert!(!snapshot.task_completed());
        snapshot.function_registered = false;
        assert_eq!(snapshot.match_awarded(), 0);
        assert!(match_reason(&snapshot).contains("not registered"));
        snapshot.function_registered = true;
        snapshot.protected_unchanged = false;
        assert!(!snapshot.scope_exact());
    }

    #[tokio::test]
    #[ignore = "requires the local iii engine at ws://127.0.0.1:49134 in namespace my-project"]
    async fn live_oracle_function_scores_on_the_running_engine() {
        std::env::set_var("III_NAMESPACE", "my-project");
        let context = E2eContext::connect("ws://127.0.0.1:49134")
            .await
            .expect("connect to the local iii engine");
        let run_id = "live-alertmanager-probe";
        let root = workspace_root(run_id);
        let _ = remove_directory(&root);
        AlertmanagerRouteMatch
            .setup(&context, run_id)
            .await
            .expect("prepare the bundle workspace");
        validate_bundle(
            &root.join(BUNDLE_RELATIVE_PATH),
            &root.join(CHECKOUT_RELATIVE_PATH),
        )
        .await
        .expect("clone the pinned bundle into checkout");
        context.client().register_function(
            FUNCTION_ID,
            iii_sdk::RegisterFunction::new_async(|payload: Value| async move {
                Ok::<Value, iii_sdk::errors::Error>(
                    oracle_answer(&payload).unwrap_or_else(|| json!({ "error": "no oracle case" })),
                )
            })
            .description("Live probe: route::match answers from the frozen oracle."),
        );
        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if context.function_exists(FUNCTION_ID).await.unwrap_or(false) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(ready.is_ok(), "route::match did not register on the engine");
        let snapshot = collect_snapshot(&context, &root)
            .await
            .expect("score the live calls");
        let evaluation = AlertmanagerRouteMatch
            .evaluate(
                &context,
                &ScenarioObservation {
                    case: ScenarioId::AlertmanagerRouteMatch
                        .materialize(run_id, CANONICAL_SEED)
                        .unwrap()
                        .case,
                    metrics: crate::wire::SessionMetricsResponse::from_normalized(
                        crate::wire::SessionMetricsPayload {
                            root_session_id: run_id.into(),
                            complete: true,
                            totals: Default::default(),
                            by_session: Vec::new(),
                            traces: None,
                        },
                    ),
                    transcript: Value::Null,
                    response: String::new(),
                    deliverables: Vec::new(),
                },
                run_id,
            )
            .await
            .expect("evaluate");
        let _ = remove_directory(&root);
        context.shutdown().await;
        assert!(
            snapshot.task_completed(),
            "task incomplete: {}",
            match_reason(&snapshot)
        );
        assert_eq!(
            snapshot.match_awarded(),
            MATCH_EQUIVALENT.weight(),
            "{}",
            match_reason(&snapshot)
        );
        assert!(snapshot.revision_pinned(), "{}", revision_reason(&snapshot));
        assert!(snapshot.scope_exact(), "{}", scope_reason(&snapshot));
        assert_eq!(
            evaluation.completion,
            crate::report::CompletionState::Completed
        );
        let match_award = evaluation
            .awards
            .iter()
            .find(|award| award.id == "match_equivalent")
            .expect("match award");
        assert_eq!(match_award.awarded, Some(MATCH_EQUIVALENT.weight()));
    }

    #[tokio::test]
    async fn embedded_bundle_head_is_the_pinned_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle = temporary.path().join("repository.bundle");
        fs::write(&bundle, BUNDLE_BYTES).unwrap();
        let checkout = temporary.path().join("checkout");
        validate_bundle(&bundle, &checkout).await.unwrap();
        let head = git(&checkout, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(head, PINNED_REVISION);
        assert!(revision_is_pinned(&checkout, Some(&head)).await);
        git(
            &checkout,
            &[
                "-c",
                "user.email=probe@example.com",
                "-c",
                "user.name=probe",
                "commit",
                "--allow-empty",
                "-m",
                "descendant",
            ],
        )
        .await
        .unwrap();
        let descendant = git(&checkout, &["rev-parse", "HEAD"]).await.unwrap();
        assert_ne!(descendant, PINNED_REVISION);
        assert!(revision_is_pinned(&checkout, Some(&descendant)).await);
    }
}
