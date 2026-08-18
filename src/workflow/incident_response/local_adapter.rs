//! Runner-owned deterministic implementation of the incident fixture contract.
//!
//! The adapter is intentionally scoped to the disposable clone selected by
//! `HARNESS_E2E_INCIDENT_FIXTURE_PATH`. It has no network or production
//! integration and all mutable state is owned by one composite runtime.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iii_sdk::errors::Error;
use iii_sdk::RegisterFunction;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::process::Command;

use super::schemas::*;
use super::*;

const CAPABILITY_VERSION: &str = "incident_fixture::v1";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const DIAGNOSIS_PROBES: &[&str] = &["replay_same_event", "compare_revision_boundary"];
const CANDIDATE_PROBES: &[&str] = &[
    "focused_tests",
    "duplicate_delivery",
    "concurrent_duplicate",
    "ack_timeout_replay",
    "distinct_events",
    "ledger_invariant",
    "audit_history",
    "full_regression",
    "canary_budget",
];

#[derive(Debug)]
struct LocalIncidentAdapter {
    root: PathBuf,
    initial_head: String,
    known_good_sha: String,
    incident_sha: String,
    fixture_contract_sha256: String,
    hidden_probe_manifest_sha256: String,
    protected_hashes: BTreeMap<String, String>,
    initial_refs_sha256: String,
    state: Mutex<AdapterState>,
}

#[derive(Debug, Default)]
struct AdapterState {
    deployed_revision: String,
    alerts: HashMap<String, u32>,
    candidate_sha: Option<String>,
    candidate_attempts: HashMap<String, u8>,
    terminal_action: Option<String>,
    settlement_count: u32,
    audit_entries: u32,
    incident_status: String,
    active_operations: u32,
}

impl LocalIncidentAdapter {
    fn from_environment() -> Result<Self> {
        Self::from_root(helpers::fixture_path()?)
    }

    fn from_root(root: PathBuf) -> Result<Self> {
        helpers::validate_fixture_tree(&root)?;
        ensure_clean_sync(&root)?;
        let initial_head = git_sync(&root, &["rev-parse", "HEAD"])?;
        let known_good_sha = git_sync(
            &root,
            &[
                "rev-parse",
                crate::scenarios::incident_response::KNOWN_GOOD_REF,
            ],
        )?;
        let incident_sha = git_sync(
            &root,
            &[
                "rev-parse",
                crate::scenarios::incident_response::INCIDENT_REF,
            ],
        )?;
        for sha in [&initial_head, &known_good_sha, &incident_sha] {
            helpers::validate_sha(sha)?;
        }
        if initial_head != incident_sha || known_good_sha == incident_sha {
            bail!("local incident fixture must start at the distinct incident revision");
        }

        let contract_path = root.join("fixture_contract.json");
        let contract: Value = serde_json::from_slice(
            &fs::read(&contract_path)
                .with_context(|| format!("read {}", contract_path.display()))?,
        )?;
        if contract != crate::scenarios::incident_response::expected_fixture_contract_identity() {
            bail!("local incident fixture contract differs from the code-owned identity");
        }
        let fixture_contract_sha256 = crate::artifact::sha256_value(&contract)?;
        let hidden_probe_manifest_sha256 = crate::artifact::sha256_value(&json!({
            "contract": "incident-hidden-probes-v1",
            "probe_count": 5,
        }))?;
        let protected_hashes = protected_hashes(&root)?;
        let initial_refs_sha256 = crate::artifact::sha256_bytes(
            git_sync(
                &root,
                &[
                    "for-each-ref",
                    "--sort=refname",
                    "--format=%(refname) %(objectname)",
                ],
            )?
            .as_bytes(),
        );
        Ok(Self {
            root,
            initial_head,
            known_good_sha,
            incident_sha: incident_sha.clone(),
            fixture_contract_sha256,
            hidden_probe_manifest_sha256,
            protected_hashes,
            initial_refs_sha256,
            state: Mutex::new(AdapterState {
                deployed_revision: incident_sha,
                settlement_count: 1,
                incident_status: "open".into(),
                ..AdapterState::default()
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdapterState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn preflight(
        &self,
        request: FixturePreflightRequest,
    ) -> Result<FixturePreflightResponse> {
        self.require_workspace(&request.workspace_root)?;
        helpers::ensure_clean(&self.root).await?;
        Ok(FixturePreflightResponse {
            repository: "iii-hq/incident-response-e2e-fixture".into(),
            workspace_root: self.root.to_string_lossy().into_owned(),
            known_good_sha: self.known_good_sha.clone(),
            incident_sha: self.incident_sha.clone(),
            fixture_contract_sha256: self.fixture_contract_sha256.clone(),
            hidden_probe_manifest_sha256: self.hidden_probe_manifest_sha256.clone(),
            clean: true,
            capability_version: CAPABILITY_VERSION.into(),
        })
    }

    fn baseline(&self, request: BaselineRequest) -> Result<BaselineResponse> {
        require_id("attempt_id", &request.attempt_id)?;
        let state = self.lock();
        Ok(BaselineResponse {
            deployed_revision: state.deployed_revision.clone(),
            data_sha256: digest(
                &json!({"orders": 3, "incident_event": crate::scenarios::incident_response::INCIDENT_EVENT_ID}),
            )?,
            telemetry_sha256: digest(&telemetry_payload("metrics"))?,
            ledger_sha256: digest(&json!({"settlements": state.settlement_count}))?,
            audit_sha256: digest(&json!({"entries": state.audit_entries, "append_only": true}))?,
            incident_status: state.incident_status.clone(),
            active_operations: state.active_operations,
        })
    }

    fn alert(&self, request: AlertRequest) -> Result<AlertResponse> {
        require_incident_event(&request.event_id)?;
        require_id("idempotency_key", &request.idempotency_key)?;
        let mut state = self.lock();
        let count = state.alerts.entry(request.idempotency_key).or_default();
        *count = count.saturating_add(1);
        let request_count = *count;
        state.incident_status = "investigating".into();
        Ok(AlertResponse {
            incident_id: "inc-duplicate-42".into(),
            alert_fingerprint:
                "sha256:52a9bc23bb1546f03c8d6d887a476f2750fd91a115ae15ee12338c946c13c320".into(),
            request_count,
            deduplicated: request_count > 1,
        })
    }

    fn reproduce(&self, request: ReproduceRequest) -> Result<ReproduceResponse> {
        require_incident_event(&request.event_id)?;
        require_id("reproduction_key", &request.reproduction_key)?;
        let mut state = self.lock();
        state.settlement_count = 2;
        state.audit_entries = 2;
        state.incident_status = "reproduced".into();
        Ok(ReproduceResponse {
            event_id: request.event_id,
            attempts: 2,
            timeout_point: "after_settlement_before_ack".into(),
            expected_settlement_count: 1,
            observed_settlement_count: 2,
            ledger_delta: 1,
            audit_entries: 2,
            evidence_ids: vec![
                "fixture.logs.duplicate-42".into(),
                "fixture.metrics.duplicate-42".into(),
                "fixture.trace-change.duplicate-42".into(),
            ],
        })
    }

    fn telemetry(&self, request: TelemetryRequest) -> Result<TelemetryResponse> {
        require_incident_event(&request.event_id)?;
        if !matches!(request.kind.as_str(), "logs" | "metrics" | "trace_change") {
            bail!("unsupported incident telemetry kind '{}'", request.kind);
        }
        let evidence_id = match request.kind.as_str() {
            "logs" => "fixture.logs.duplicate-42",
            "metrics" => "fixture.metrics.duplicate-42",
            _ => "fixture.trace-change.duplicate-42",
        };
        Ok(TelemetryResponse {
            payload: telemetry_payload(&request.kind),
            kind: request.kind,
            evidence_id: evidence_id.into(),
        })
    }

    async fn validate(&self, request: ValidateRequest) -> Result<ValidateResponse> {
        self.require_workspace(&request.workspace_root)?;
        require_id("attempt_id", &request.attempt_id)?;
        match request.mode.as_str() {
            "diagnosis" => self.validate_diagnosis(request).await,
            "candidate" => self.validate_candidate(request).await,
            other => bail!("unsupported incident validation mode '{other}'"),
        }
    }

    async fn validate_diagnosis(&self, request: ValidateRequest) -> Result<ValidateResponse> {
        let unchanged = candidate_paths(&self.root).await?.is_empty();
        let probes = request
            .probe_ids
            .into_iter()
            .map(|id| {
                let passed = unchanged && DIAGNOSIS_PROBES.contains(&id.as_str());
                let summary = if passed {
                    "The fixture-owned replay distinguishes the incident revision from known-good without mutating the workspace."
                } else {
                    "Unknown probe id or repository mutation preceded diagnosis."
                };
                (id, ProbeResult { passed, summary: summary.into() })
            })
            .collect();
        Ok(ValidateResponse {
            candidate_sha: None,
            changed_paths: Vec::new(),
            protected_paths_unchanged: protected_hashes(&self.root)? == self.protected_hashes,
            tests_unchanged: tests_unchanged(&self.root, &self.protected_hashes)?,
            fixture_contract_unchanged: fixture_contract_unchanged(
                &self.root,
                &self.protected_hashes,
            )?,
            working_tree_candidate_only: unchanged,
            repair_rounds: 0,
            probes,
            patch: String::new(),
            before_after_hashes: BTreeMap::new(),
        })
    }

    async fn validate_candidate(&self, request: ValidateRequest) -> Result<ValidateResponse> {
        let paths = candidate_paths(&self.root).await?;
        let patch = git_async(
            &self.root,
            &["diff", "--no-ext-diff", &self.incident_sha, "--"],
        )
        .await?;
        let protected = protected_hashes(&self.root)? == self.protected_hashes;
        let tests_exact = tests_unchanged(&self.root, &self.protected_hashes)?;
        let contract_exact = fixture_contract_unchanged(&self.root, &self.protected_hashes)?;
        let head_exact = git_async(&self.root, &["rev-parse", "HEAD"]).await? == self.incident_sha;
        let refs_exact = refs_sha256(&self.root).await? == self.initial_refs_sha256;
        let allowed = !paths.is_empty()
            && paths
                .iter()
                .all(|path| helpers::allowed_production_path(path));
        let focused = allowed && protected && candidate_behavior_passes(&self.root).await;
        let full = focused && public_test_passes(&self.root).await;
        let patch_bounded = !patch.trim().is_empty() && patch.len() <= helpers::MAX_PATCH_BYTES;
        let base_passed = focused
            && full
            && patch_bounded
            && tests_exact
            && contract_exact
            && head_exact
            && refs_exact;
        let probes = request
            .probe_ids
            .into_iter()
            .map(|id| {
                let known = CANDIDATE_PROBES.contains(&id.as_str());
                let passed = known && base_passed;
                (
                    id,
                    ProbeResult {
                        passed,
                        summary: if passed {
                            "Deterministic local probe passed against the candidate workspace."
                        } else {
                            "Unknown probe id or candidate invariant failed."
                        }
                        .into(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let repair_rounds = {
            let mut state = self.lock();
            let rounds = state
                .candidate_attempts
                .entry(request.attempt_id.clone())
                .or_default();
            *rounds = rounds.saturating_add(1);
            *rounds
        };
        let candidate_sha = patch_bounded.then(|| candidate_identity(&self.incident_sha, &patch));
        if base_passed {
            self.lock().candidate_sha = candidate_sha.clone();
        }
        let before_after_hashes = paths
            .iter()
            .map(|path| {
                let before = git_sync(
                    &self.root,
                    &["show", &format!("{}:{path}", self.incident_sha)],
                )
                .map(|value| crate::artifact::sha256_bytes(value.as_bytes()))
                .unwrap_or_else(|_| "missing".into());
                let after = fs::read(self.root.join(path))
                    .map(|value| crate::artifact::sha256_bytes(&value))
                    .unwrap_or_else(|_| "missing".into());
                (path.clone(), format!("{before}->{after}"))
            })
            .collect();
        Ok(ValidateResponse {
            candidate_sha,
            changed_paths: paths,
            protected_paths_unchanged: protected,
            tests_unchanged: tests_exact,
            fixture_contract_unchanged: contract_exact,
            working_tree_candidate_only: allowed && head_exact && refs_exact,
            repair_rounds,
            probes,
            patch,
            before_after_hashes,
        })
    }

    fn deploy(&self, request: DeployRequest) -> Result<DeployResponse> {
        require_id("attempt_id", &request.attempt_id)?;
        let mut state = self.lock();
        if state.terminal_action.is_some() {
            bail!("incident fixture permits exactly one terminal operation");
        }
        let authorized = match request.action.as_str() {
            "promote" => state.candidate_sha.as_deref() == Some(request.revision.as_str()),
            "rollback" => request.revision == self.known_good_sha,
            _ => false,
        };
        if !authorized {
            bail!("terminal action or revision was not authorized by local validation");
        }
        state.terminal_action = Some(request.action.clone());
        state.deployed_revision = request.revision.clone();
        state.settlement_count = 1;
        state.incident_status = if request.action == "promote" {
            "resolved".into()
        } else {
            "rolled_back".into()
        };
        state.active_operations = 0;
        Ok(DeployResponse {
            action: request.action,
            deployed_revision: request.revision,
            active_operations: 0,
        })
    }

    fn reconcile(&self, request: ReconcileRequest) -> Result<ReconcileResponse> {
        require_id("attempt_id", &request.attempt_id)?;
        let state = self.lock();
        Ok(ReconcileResponse {
            deployed_revision: state.deployed_revision.clone(),
            event_id: crate::scenarios::incident_response::INCIDENT_EVENT_ID.into(),
            settlement_count: state.settlement_count,
            distinct_events_preserved: true,
            audit_history_preserved: true,
            incident_status: state.incident_status.clone(),
            active_operations: state.active_operations,
        })
    }

    async fn reset(&self, request: ResetRequest) -> Result<ResetResponse> {
        require_id("attempt_id", &request.attempt_id)?;
        if request.initial_revision != self.initial_head {
            bail!("reset revision differs from the adapter-owned initial revision");
        }
        git_async(&self.root, &["reset", "--hard", &self.initial_head]).await?;
        git_async(&self.root, &["clean", "-fd"]).await?;
        helpers::ensure_clean(&self.root).await?;
        *self.lock() = AdapterState {
            deployed_revision: self.initial_head.clone(),
            settlement_count: 1,
            incident_status: "reset".into(),
            ..AdapterState::default()
        };
        Ok(ResetResponse {
            restored_revision: self.initial_head.clone(),
            clean: true,
            active_operations: 0,
        })
    }

    fn require_workspace(&self, value: &str) -> Result<()> {
        if Path::new(value) != self.root {
            bail!("request workspace does not match the adapter-owned fixture root");
        }
        Ok(())
    }
}

pub(super) fn register(context: &E2eContext) -> Result<()> {
    let adapter = Arc::new(LocalIncidentAdapter::from_environment()?);
    register_one(
        context,
        PREFLIGHT_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.preflight(request).await },
    );
    register_one(
        context,
        BASELINE_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.baseline(request) },
    );
    register_one(
        context,
        ALERT_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.alert(request) },
    );
    register_one(
        context,
        REPRODUCE_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.reproduce(request) },
    );
    register_one(
        context,
        TELEMETRY_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.telemetry(request) },
    );
    register_one(
        context,
        VALIDATE_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.validate(request).await },
    );
    register_one(
        context,
        DEPLOY_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.deploy(request) },
    );
    register_one(
        context,
        RECONCILE_FUNCTION,
        adapter.clone(),
        |adapter, request| async move { adapter.reconcile(request) },
    );
    register_one(
        context,
        RESET_FUNCTION,
        adapter,
        |adapter, request| async move { adapter.reset(request).await },
    );
    Ok(())
}

fn register_one<Request, Response, Fut, Handler>(
    context: &E2eContext,
    function_id: &str,
    adapter: Arc<LocalIncidentAdapter>,
    handler: Handler,
) where
    Request: serde::de::DeserializeOwned + schemars::JsonSchema + Send + 'static,
    Response: Serialize + schemars::JsonSchema + Send + 'static,
    Fut: std::future::Future<Output = Result<Response>> + Send + 'static,
    Handler: Fn(Arc<LocalIncidentAdapter>, Request) -> Fut + Send + Sync + 'static,
{
    context.client().register_function(
        function_id,
        RegisterFunction::new_async(move |request: Request| {
            let adapter = adapter.clone();
            let future = handler(adapter, request);
            async move { future.await.map_err(handler_error) }
        })
        .description("Runner-owned deterministic local incident fixture adapter.")
        .metadata(json!({
            "internal": true,
            "local_adapter": true,
            "capability_version": CAPABILITY_VERSION,
        })),
    );
}

fn handler_error(error: anyhow::Error) -> Error {
    Error::Handler(format!("local incident fixture: {error:#}"))
}

fn require_id(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 {
        bail!("{field} must contain 1..=256 bytes");
    }
    Ok(())
}

fn require_incident_event(value: &str) -> Result<()> {
    if value != crate::scenarios::incident_response::INCIDENT_EVENT_ID {
        bail!("event_id is outside the synthetic incident case");
    }
    Ok(())
}

fn digest(value: &Value) -> Result<String> {
    crate::artifact::sha256_value(value)
}

fn telemetry_payload(kind: &str) -> Value {
    match kind {
        "logs" => json!({
            "event_id": crate::scenarios::incident_response::INCIDENT_EVENT_ID,
            "entries": [
                "attempt=1 settlement=committed acknowledgement=timeout",
                "attempt=2 redelivery=true settlement=committed acknowledgement=ok"
            ],
            "fixture_probe_ids": DIAGNOSIS_PROBES,
        }),
        "metrics" => json!({
            "delivery_attempts": 2,
            "expected_settlements": 1,
            "observed_settlements": 2,
            "application_errors": 0,
            "fixture_probe_ids": DIAGNOSIS_PROBES,
        }),
        _ => json!({
            "known_good_ref": crate::scenarios::incident_response::KNOWN_GOOD_REF,
            "incident_ref": crate::scenarios::incident_response::INCIDENT_REF,
            "changed_paths": ["src/settlement.py"],
            "change": "the incident revision changed the settlement identity boundary",
            "fixture_probe_ids": DIAGNOSIS_PROBES,
        }),
    }
}

fn candidate_identity(parent: &str, patch: &str) -> String {
    let digest = crate::artifact::sha256_bytes(
        serde_json::to_string(&json!({"parent": parent, "patch": patch}))
            .unwrap_or_default()
            .as_bytes(),
    );
    digest.trim_start_matches("sha256:")[..40].to_string()
}

fn protected_hashes(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = vec![root.join("fixture_contract.json")];
    collect_files(&root.join("tests"), &mut files)?;
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            Ok((relative, crate::artifact::sha256_bytes(&fs::read(path)?)))
        })
        .collect()
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect_files(&path, files)?;
        } else if metadata.is_file() && !metadata.file_type().is_symlink() {
            files.push(path);
        }
    }
    Ok(())
}

fn tests_unchanged(root: &Path, expected: &BTreeMap<String, String>) -> Result<bool> {
    let actual = protected_hashes(root)?;
    Ok(expected
        .iter()
        .filter(|(path, _)| path.starts_with("tests/"))
        .all(|(path, digest)| actual.get(path) == Some(digest)))
}

fn fixture_contract_unchanged(root: &Path, expected: &BTreeMap<String, String>) -> Result<bool> {
    let digest = crate::artifact::sha256_bytes(&fs::read(root.join("fixture_contract.json"))?);
    Ok(expected.get("fixture_contract.json") == Some(&digest))
}

async fn candidate_paths(root: &Path) -> Result<Vec<String>> {
    let status = git_async(root, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    let mut paths = status
        .lines()
        .filter_map(|line| {
            if line.as_bytes().get(2) == Some(&b' ') {
                line.get(3..)
            } else if line.as_bytes().get(1) == Some(&b' ') {
                // `git_async` trims the leading space of the first unstaged
                // Porcelain record along with the final newline.
                line.get(2..)
            } else {
                None
            }
        })
        .map(str::trim)
        .filter(|path| !path.starts_with(".harness-e2e/"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

async fn refs_sha256(root: &Path) -> Result<String> {
    Ok(crate::artifact::sha256_bytes(
        git_async(
            root,
            &[
                "for-each-ref",
                "--sort=refname",
                "--format=%(refname) %(objectname)",
            ],
        )
        .await?
        .as_bytes(),
    ))
}

async fn candidate_behavior_passes(root: &Path) -> bool {
    run_python(
        root,
        "from src.settlement import settlement_key; e='evt-duplicate-42'; assert settlement_key(e, 1) != settlement_key(e, 2); assert settlement_key(e, 1) == settlement_key(e, 1); assert settlement_key(e, 1) != settlement_key('evt-distinct-43', 1)",
    )
    .await
}

async fn public_test_passes(root: &Path) -> bool {
    run_python(
        root,
        "import runpy; ns=runpy.run_path('tests/test_settlement.py'); ns['test_distinct_attempts_have_distinct_keys']()",
    )
    .await
}

async fn run_python(root: &Path, source: &str) -> bool {
    let mut command = Command::new("python3");
    command
        .args(["-E", "-s", "-c", source])
        .current_dir(root)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH");
    matches!(
        tokio::time::timeout(COMMAND_TIMEOUT, command.output()).await,
        Ok(Ok(output)) if output.status.success()
    )
}

fn git_sync(root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))
        .env("GIT_CONFIG_SYSTEM", OsStr::new("/dev/null"))
        .env("GIT_TERMINAL_PROMPT", OsStr::new("0"))
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), root.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

async fn git_async(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))
        .env("GIT_CONFIG_SYSTEM", OsStr::new("/dev/null"))
        .env("GIT_TERMINAL_PROMPT", OsStr::new("0"))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .context("Git command timed out")??;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn ensure_clean_sync(root: &Path) -> Result<()> {
    let status = git_sync(root, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    if !status.is_empty() {
        bail!("local incident fixture must be clean before registration: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_contracts_accept_runtime_metadata_without_publishing_it() {
        assert_metadata_compatible::<FixturePreflightRequest>(
            json!({"workspace_root": "/tmp/fixture"}),
        );
        assert_metadata_compatible::<BaselineRequest>(json!({"attempt_id": "attempt-1"}));
        assert_metadata_compatible::<AlertRequest>(json!({
            "event_id": "evt-duplicate-42",
            "idempotency_key": "alert-1"
        }));
        assert_metadata_compatible::<ReproduceRequest>(json!({
            "event_id": "evt-duplicate-42",
            "reproduction_key": "reproduce-1"
        }));
        assert_metadata_compatible::<TelemetryRequest>(json!({
            "kind": "logs",
            "event_id": "evt-duplicate-42"
        }));
        assert_metadata_compatible::<ValidateRequest>(json!({
            "mode": "diagnosis",
            "attempt_id": "attempt-1",
            "workspace_root": "/tmp/fixture",
            "probe_ids": []
        }));
        assert_metadata_compatible::<DeployRequest>(json!({
            "action": "rollback",
            "revision": "0000000000000000000000000000000000000000",
            "attempt_id": "attempt-1"
        }));
        assert_metadata_compatible::<ReconcileRequest>(json!({"attempt_id": "attempt-1"}));
        assert_metadata_compatible::<ResetRequest>(json!({
            "attempt_id": "attempt-1",
            "initial_revision": "0000000000000000000000000000000000000000"
        }));
    }

    fn assert_metadata_compatible<T>(mut payload: Value)
    where
        T: serde::de::DeserializeOwned + schemars::JsonSchema,
    {
        payload["_caller_worker_id"] = json!("runtime-worker");
        serde_json::from_value::<T>(payload).unwrap();
        let published = serde_json::to_string(&schemars::schema_for!(T)).unwrap();
        assert!(!published.contains("_caller_worker_id"));
    }

    #[tokio::test]
    async fn local_adapter_runs_deterministic_lifecycle_and_resets_fixture() {
        let temporary =
            tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("target")).unwrap();
        let root = temporary.path().join("fixture");
        create_fixture(&root).unwrap();
        let adapter = LocalIncidentAdapter::from_root(root.clone()).unwrap();
        let workspace = root.to_string_lossy().into_owned();

        let preflight = adapter
            .preflight(FixturePreflightRequest {
                workspace_root: workspace.clone(),
                _caller_worker_id: None,
            })
            .await
            .unwrap();
        assert_eq!(preflight.capability_version, CAPABILITY_VERSION);
        assert!(preflight.clean);

        let first = adapter
            .alert(AlertRequest {
                event_id: crate::scenarios::incident_response::INCIDENT_EVENT_ID.into(),
                idempotency_key: "attempt:alert".into(),
                _caller_worker_id: None,
            })
            .unwrap();
        let second = adapter
            .alert(AlertRequest {
                event_id: crate::scenarios::incident_response::INCIDENT_EVENT_ID.into(),
                idempotency_key: "attempt:alert".into(),
                _caller_worker_id: None,
            })
            .unwrap();
        assert!(!first.deduplicated);
        assert!(second.deduplicated);
        assert_eq!(second.request_count, 2);

        let reproduction = adapter
            .reproduce(ReproduceRequest {
                event_id: crate::scenarios::incident_response::INCIDENT_EVENT_ID.into(),
                reproduction_key: "attempt:reproduce".into(),
                _caller_worker_id: None,
            })
            .unwrap();
        assert_eq!(reproduction.observed_settlement_count, 2);

        fs::write(
            root.join("src/settlement.py"),
            "def settlement_key(event_id: str, attempt: int) -> str:\n    return f\"{event_id}:{attempt}\"\n",
        )
        .unwrap();
        let validation = adapter
            .validate(ValidateRequest {
                mode: "candidate".into(),
                attempt_id: "attempt-1".into(),
                workspace_root: workspace,
                candidate_sha: None,
                probe_ids: CANDIDATE_PROBES.iter().map(|id| (*id).into()).collect(),
                _caller_worker_id: None,
            })
            .await
            .unwrap();
        assert!(validation.candidate_sha.is_some());
        assert!(
            validation.probes.values().all(|probe| probe.passed),
            "candidate validation failed: {validation:#?}"
        );
        let candidate = validation.candidate_sha.unwrap();
        adapter
            .deploy(DeployRequest {
                action: "promote".into(),
                revision: candidate.clone(),
                attempt_id: "attempt-1".into(),
                _caller_worker_id: None,
            })
            .unwrap();
        let final_state = adapter
            .reconcile(ReconcileRequest {
                attempt_id: "attempt-1".into(),
                _caller_worker_id: None,
            })
            .unwrap();
        assert_eq!(final_state.deployed_revision, candidate);
        assert_eq!(final_state.incident_status, "resolved");
        assert_eq!(final_state.settlement_count, 1);

        let reset = adapter
            .reset(ResetRequest {
                attempt_id: "attempt-1".into(),
                initial_revision: adapter.initial_head.clone(),
                _caller_worker_id: None,
            })
            .await
            .unwrap();
        assert!(reset.clean);
        assert_eq!(
            git_sync(&root, &["rev-parse", "HEAD"]).unwrap(),
            adapter.initial_head
        );
        ensure_clean_sync(&root).unwrap();
    }

    fn create_fixture(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("tests"))?;
        fs::write(
            root.join("fixture_contract.json"),
            serde_json::to_vec_pretty(
                &crate::scenarios::incident_response::expected_fixture_contract_identity(),
            )?,
        )?;
        fs::write(
            root.join("tests/test_settlement.py"),
            "from src.settlement import settlement_key\n\ndef test_distinct_attempts_have_distinct_keys():\n    assert settlement_key('evt-duplicate-42', 1) != settlement_key('evt-duplicate-42', 2)\n",
        )?;
        fs::write(
            root.join("src/settlement.py"),
            "def settlement_key(event_id: str, attempt: int) -> str:\n    return f\"{event_id}:{attempt}\"\n",
        )?;
        git_test(root, &["init", "-q"])?;
        git_test(root, &["add", "."])?;
        commit(root, "known good")?;
        git_test(root, &["tag", "known_good"])?;
        fs::write(
            root.join("src/settlement.py"),
            "def settlement_key(event_id: str, attempt: int) -> str:\n    return event_id\n",
        )?;
        git_test(root, &["add", "."])?;
        commit(root, "incident")?;
        git_test(root, &["tag", "incident"])?;
        Ok(())
    }

    fn git_test(root: &Path, args: &[&str]) -> Result<()> {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))
            .env("GIT_CONFIG_SYSTEM", OsStr::new("/dev/null"))
            .status()?;
        if !status.success() {
            bail!("test Git command failed: {}", args.join(" "));
        }
        Ok(())
    }

    fn commit(root: &Path, message: &str) -> Result<()> {
        let status = std::process::Command::new("git")
            .args(["commit", "-qm", message])
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))
            .env("GIT_CONFIG_SYSTEM", OsStr::new("/dev/null"))
            .env("GIT_AUTHOR_NAME", "Harness E2E")
            .env("GIT_AUTHOR_EMAIL", "harness-e2e@example.invalid")
            .env("GIT_COMMITTER_NAME", "Harness E2E")
            .env("GIT_COMMITTER_EMAIL", "harness-e2e@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .status()?;
        if !status.success() {
            bail!("test Git commit failed");
        }
        Ok(())
    }
}
