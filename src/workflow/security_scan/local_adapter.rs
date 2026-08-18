use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use iii_sdk::{errors::Error as IiiError, RegisterFunction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;

const REQUEST_DESC: &str = "Queue a report-only security review for an operator-configured repository at an exact 40-character Git commit SHA. Duplicate repository, commit, and mode requests return the same run id.";
const READ_DESC: &str = "Read a security-scan run and its validated report without exposing internal checkout paths or Harness session identifiers.";
const LIST_DESC: &str = "List security-scan runs as sanitized lightweight summaries, newest update first. Optional repository and status filters are applied before the bounded result limit.";
const RECONCILIATION_DESC: &str = "Read or refresh a persisted, sanitized comparison of one Harness report with separately counted Dependabot and code-scanning snapshots. Supports bounded source, severity, lifecycle, and cursor filters; never reports a combined unique total.";
const SCHEMA_VERSION: &str = "1";
const DETERMINISTIC_TIME_MS: i64 = 1_700_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ScanModeV1 {
    Scan,
    Suggest,
}

impl ScanModeV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Suggest => "suggest",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanRequestV1 {
    repository: String,
    target_sha: String,
    mode: ScanModeV1,
    /// Metadata injected by the iii engine. It is accepted on the wire but is
    /// not part of the public function schema or the request identity.
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RunStatusV1 {
    Queued,
    Materializing,
    Materialized,
    Dispatching,
    Analyzing,
    Completed,
    Failed,
    Cancelling,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunErrorV1 {
    code: String,
    message: String,
    retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanResponseV1 {
    run_id: String,
    status: RunStatusV1,
    deduplicated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanReadRequestV1 {
    run_id: String,
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanListRequestV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<RunStatusV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PublicRunV1 {
    schema_version: String,
    run_id: String,
    repository: String,
    target_sha: String,
    mode: ScanModeV1,
    status: RunStatusV1,
    attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report: Option<SecurityReportV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<RunErrorV1>,
    created_at: i64,
    updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PublicRunSummaryV1 {
    run_id: String,
    repository: String,
    target_sha: String,
    mode: ScanModeV1,
    status: RunStatusV1,
    attempt: u32,
    finding_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<RunErrorV1>,
    created_at: i64,
    updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at: Option<i64>,
}

impl From<&PublicRunV1> for PublicRunSummaryV1 {
    fn from(run: &PublicRunV1) -> Self {
        Self {
            run_id: run.run_id.clone(),
            repository: run.repository.clone(),
            target_sha: run.target_sha.clone(),
            mode: run.mode,
            status: run.status,
            attempt: run.attempt,
            finding_count: run
                .report
                .as_ref()
                .map(|report| u32::try_from(report.findings.len()).unwrap_or(u32::MAX))
                .unwrap_or_default(),
            error: run.error.clone(),
            created_at: run.created_at,
            updated_at: run.updated_at,
            completed_at: run.completed_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanReadResponseV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<PublicRunV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanListResponseV1 {
    runs: Vec<PublicRunSummaryV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SeverityV1 {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReconciliationSourceV1 {
    Dependabot,
    CodeScanning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReconciliationLifecycleV1 {
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReconciliationScopeV1 {
    ExactCommit,
    RepositoryDefaultBranch,
    RepositorySnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReconciliationSourceStatusV1 {
    Complete,
    Partial,
    Unavailable,
    AuthenticationRequired,
    PermissionDenied,
    Disabled,
    NotConfigured,
    NotCollected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReconciliationHealthStatusV1 {
    Healthy,
    Warning,
    Error,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconciliationSourceHealthV1 {
    status: ReconciliationHealthStatusV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconciliationSourceSummaryV1 {
    source: ReconciliationSourceV1,
    status: ReconciliationSourceStatusV1,
    scope: ReconciliationScopeV1,
    /// Collection time in Unix milliseconds. Null means the source was not queried.
    collected_at: Option<i64>,
    /// Number of normalized records when collection returned usable data. Null
    /// is unavailable/not-collected and is deliberately distinct from zero.
    record_count: Option<u32>,
    health: ReconciliationSourceHealthV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconciliationAlertV1 {
    source: ReconciliationSourceV1,
    number: u64,
    severity: SeverityV1,
    lifecycle: ReconciliationLifecycleV1,
    scope: ReconciliationScopeV1,
    title: String,
    description: String,
    /// Reconstructed public github.com URL. Dependency-provided URLs are never persisted.
    public_url: String,
    /// Exact source identifiers only, such as GHSA, CVE, or scanner rule IDs.
    #[serde(default)]
    structured_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum HarnessReconciliationStatusV1 {
    Verified,
    NotAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HarnessReconciliationSummaryV1 {
    status: HarnessReconciliationStatusV1,
    /// Validated Harness report findings. This is never added to GitHub source counts.
    verified_count: Option<u32>,
    verified_at: Option<i64>,
    scope: ReconciliationScopeV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum ReconciliationMatchingStatusV1 {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconciliationMatchingV1 {
    status: ReconciliationMatchingStatusV1,
    /// Present only when exact structured identifiers produced matches.
    matched_records: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanReconciliationRequestV1 {
    run_id: String,
    #[serde(default)]
    refresh: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<ReconciliationSourceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    severity: Option<SeverityV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle: Option<ReconciliationLifecycleV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(rename = "_caller_worker_id", default)]
    #[schemars(skip)]
    _caller_worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityScanReconciliationResponseV1 {
    schema_version: String,
    run_id: String,
    repository: String,
    target_sha: String,
    harness: HarnessReconciliationSummaryV1,
    github_repository: Option<String>,
    sources: Vec<ReconciliationSourceSummaryV1>,
    matching: ReconciliationMatchingV1,
    records: Vec<ReconciliationAlertV1>,
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum AssessmentStatusV1 {
    Assessed,
    NotAssessed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityAreaAssessmentV1 {
    status: AssessmentStatusV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityAssessmentsV1 {
    vulnerabilities: SecurityAreaAssessmentV1,
    dependencies: SecurityAreaAssessmentV1,
    secrets: SecurityAreaAssessmentV1,
    supply_chain: SecurityAreaAssessmentV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindingLocationV1 {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_end: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityFindingV1 {
    rule_id: String,
    severity: SeverityV1,
    title: String,
    description: String,
    evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    location: Option<FindingLocationV1>,
    remediation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_patch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityReportV1 {
    summary: String,
    assessments: SecurityAssessmentsV1,
    findings: Vec<SecurityFindingV1>,
}

#[derive(Default)]
struct LocalState {
    runs: BTreeMap<String, PublicRunV1>,
    identities: HashMap<(String, String, String), String>,
    snapshots: HashMap<String, SecurityScanReconciliationResponseV1>,
    sequence: i64,
}

struct LocalAdapter {
    fixture: PathBuf,
    state: Mutex<LocalState>,
}

impl LocalAdapter {
    fn new(fixture: PathBuf) -> Self {
        Self {
            fixture,
            state: Mutex::new(LocalState::default()),
        }
    }

    async fn request(&self, mut request: SecurityScanRequestV1) -> Result<SecurityScanResponseV1> {
        if request.repository != REPOSITORY {
            bail!(
                "repository '{}' is not the configured local security fixture",
                request.repository
            );
        }
        validate_sha(&request.target_sha)?;
        request.target_sha.make_ascii_lowercase();
        let commit = format!("{}^{{commit}}", request.target_sha);
        git(&self.fixture, &["cat-file", "-e", &commit])
            .await
            .with_context(|| format!("fixture does not contain commit {}", request.target_sha))?;
        for path in SEEDED_PATHS {
            let object = format!("{}:{path}", request.target_sha);
            git(&self.fixture, &["cat-file", "-e", &object])
                .await
                .with_context(|| {
                    format!(
                        "fixture commit {} does not contain required seeded path {path}",
                        request.target_sha
                    )
                })?;
        }

        Ok(self.record_request(request))
    }

    fn record_request(&self, request: SecurityScanRequestV1) -> SecurityScanResponseV1 {
        let identity = (
            request.repository.clone(),
            request.target_sha.clone(),
            request.mode.as_str().to_string(),
        );
        let mut state = self.lock();
        if let Some(run_id) = state.identities.get(&identity).cloned() {
            return SecurityScanResponseV1 {
                run_id,
                status: RunStatusV1::Completed,
                deduplicated: true,
            };
        }
        let run_id = deterministic_run_id(&identity);
        let timestamp = DETERMINISTIC_TIME_MS + state.sequence;
        state.sequence += 1;
        let report = deterministic_report(request.mode);
        let run = PublicRunV1 {
            schema_version: SCHEMA_VERSION.into(),
            run_id: run_id.clone(),
            repository: request.repository,
            target_sha: request.target_sha,
            mode: request.mode,
            status: RunStatusV1::Completed,
            attempt: 1,
            report: Some(report),
            error: None,
            created_at: timestamp,
            updated_at: timestamp,
            completed_at: Some(timestamp),
        };
        state.identities.insert(identity, run_id.clone());
        state.runs.insert(run_id.clone(), run);
        SecurityScanResponseV1 {
            run_id,
            status: RunStatusV1::Completed,
            deduplicated: false,
        }
    }

    fn read(&self, request: SecurityScanReadRequestV1) -> SecurityScanReadResponseV1 {
        SecurityScanReadResponseV1 {
            run: self.lock().runs.get(&request.run_id).cloned(),
        }
    }

    fn list(&self, request: SecurityScanListRequestV1) -> SecurityScanListResponseV1 {
        let mut runs = self
            .lock()
            .runs
            .values()
            .filter(|run| {
                request
                    .repository
                    .as_ref()
                    .is_none_or(|repository| &run.repository == repository)
                    && request.status.is_none_or(|status| run.status == status)
            })
            .map(PublicRunSummaryV1::from)
            .collect::<Vec<_>>();
        runs.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.run_id.cmp(&left.run_id))
        });
        runs.truncate(request.limit.unwrap_or(50).clamp(1, 100) as usize);
        SecurityScanListResponseV1 { runs }
    }

    fn reconciliation(
        &self,
        request: SecurityScanReconciliationRequestV1,
    ) -> Result<SecurityScanReconciliationResponseV1> {
        let mut state = self.lock();
        let run =
            state.runs.get(&request.run_id).cloned().with_context(|| {
                format!("security scan run '{}' does not exist", request.run_id)
            })?;
        if request.refresh || !state.snapshots.contains_key(&request.run_id) {
            state
                .snapshots
                .insert(request.run_id.clone(), deterministic_snapshot(&run));
        }
        let mut response = state.snapshots[&request.run_id].clone();
        let offset = parse_cursor(request.cursor.as_deref())?;
        response.records.retain(|record| {
            request.source.is_none_or(|source| record.source == source)
                && request
                    .severity
                    .is_none_or(|severity| record.severity == severity)
                && request
                    .lifecycle
                    .is_none_or(|lifecycle| record.lifecycle == lifecycle)
        });
        let limit = request.limit.unwrap_or(50).clamp(1, 100) as usize;
        let total = response.records.len();
        response.records = response
            .records
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect();
        response.next_cursor = (offset + response.records.len() < total)
            .then(|| format!("local:{}", offset + response.records.len()));
        Ok(response)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LocalState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) async fn register_local_adapter_if_configured(context: &E2eContext) -> Result<bool> {
    if std::env::var_os(FIXTURE_PATH_ENV).is_none() {
        return Ok(false);
    }
    let function_ids = [
        REQUEST_FUNCTION,
        READ_FUNCTION,
        LIST_FUNCTION,
        RECONCILIATION_FUNCTION,
    ];
    let mut present = Vec::with_capacity(function_ids.len());
    for function_id in function_ids {
        present.push(context.function_exists(function_id).await?);
    }
    if present.iter().all(|present| *present) {
        return Ok(false);
    }
    if present.iter().any(|present| *present) {
        bail!("security-scan worker registration is partial; refusing to mix local and external contracts");
    }

    let adapter = Arc::new(LocalAdapter::new(fixture_path()?));
    let request_adapter = adapter.clone();
    context.client().register_function(
        REQUEST_FUNCTION,
        RegisterFunction::new_async(move |request: SecurityScanRequestV1| {
            let adapter = request_adapter.clone();
            async move { adapter.request(request).await.map_err(handler_error) }
        })
        .description(REQUEST_DESC),
    );
    let read_adapter = adapter.clone();
    context.client().register_function(
        READ_FUNCTION,
        RegisterFunction::new_async(move |request: SecurityScanReadRequestV1| {
            let adapter = read_adapter.clone();
            async move { Ok::<SecurityScanReadResponseV1, IiiError>(adapter.read(request)) }
        })
        .description(READ_DESC),
    );
    let list_adapter = adapter.clone();
    context.client().register_function(
        LIST_FUNCTION,
        RegisterFunction::new_async(move |request: SecurityScanListRequestV1| {
            let adapter = list_adapter.clone();
            async move { Ok::<SecurityScanListResponseV1, IiiError>(adapter.list(request)) }
        })
        .description(LIST_DESC),
    );
    context.client().register_function(
        RECONCILIATION_FUNCTION,
        RegisterFunction::new_async(move |request: SecurityScanReconciliationRequestV1| {
            let adapter = adapter.clone();
            async move { adapter.reconciliation(request).map_err(handler_error) }
        })
        .description(RECONCILIATION_DESC),
    );
    Ok(true)
}

fn handler_error(error: anyhow::Error) -> IiiError {
    IiiError::Handler(format!("local security-scan adapter: {error:#}"))
}

fn deterministic_run_id(identity: &(String, String, String)) -> String {
    let mut digest = Sha256::new();
    digest.update(identity.0.as_bytes());
    digest.update([0]);
    digest.update(identity.1.as_bytes());
    digest.update([0]);
    digest.update(identity.2.as_bytes());
    format!("sec_local_{:x}", digest.finalize())
}

fn assessed() -> SecurityAreaAssessmentV1 {
    SecurityAreaAssessmentV1 {
        status: AssessmentStatusV1::Assessed,
        reason: None,
    }
}

fn deterministic_report(_mode: ScanModeV1) -> SecurityReportV1 {
    let locations = [
        (
            "fixture.command-injection",
            SeverityV1::Critical,
            "Untrusted shell command construction",
            "src/vulnerable.rs",
            7,
        ),
        (
            "fixture.dependencies",
            SeverityV1::High,
            "Unpinned vulnerable dependencies",
            "package.json",
            6,
        ),
        (
            "fixture.fake-secret",
            SeverityV1::Medium,
            "Credential-shaped value in environment template",
            ".env.example",
            2,
        ),
        (
            "fixture.supply-chain",
            SeverityV1::High,
            "Unpinned action and piped installer",
            ".github/workflows/insecure.yml",
            15,
        ),
    ];
    let findings = locations
        .into_iter()
        .map(|(rule_id, severity, title, path, line)| SecurityFindingV1 {
            rule_id: rule_id.into(),
            severity,
            title: title.into(),
            description: "Deterministic finding from the intentionally vulnerable E2E fixture."
                .into(),
            evidence: format!("Seeded test pattern at {path}:{line}."),
            location: Some(FindingLocationV1 {
                path: path.into(),
                line_start: Some(line),
                line_end: Some(line),
            }),
            remediation: "Replace the seeded unsafe construct with a bounded, pinned alternative."
                .into(),
            // Patch generation is optional in the public contract. The local adapter
            // keeps suggest-mode output deterministic without creating a disposable
            // worktree or mutating the launcher-owned fixture.
            suggested_patch: None,
        })
        .collect();
    SecurityReportV1 {
        summary:
            "Deterministic local analysis found the four intentionally seeded security patterns."
                .into(),
        assessments: SecurityAssessmentsV1 {
            vulnerabilities: assessed(),
            dependencies: assessed(),
            secrets: assessed(),
            supply_chain: assessed(),
        },
        findings,
    }
}

fn deterministic_snapshot(run: &PublicRunV1) -> SecurityScanReconciliationResponseV1 {
    let finding_count = run
        .report
        .as_ref()
        .map(|report| u32::try_from(report.findings.len()).unwrap_or(u32::MAX));
    let records = vec![
        ReconciliationAlertV1 {
            source: ReconciliationSourceV1::Dependabot,
            number: 1,
            severity: SeverityV1::High,
            lifecycle: ReconciliationLifecycleV1::Open,
            scope: ReconciliationScopeV1::RepositoryDefaultBranch,
            title: "Deterministic Dependabot fixture alert".into(),
            description: "Sanitized local reconciliation record.".into(),
            public_url: "https://github.com/iii-hq/security-scan-e2e-fixture/security/dependabot/1"
                .into(),
            structured_ids: vec!["GHSA-LOCAL-E2E".into()],
            path: Some("package.json".into()),
            start_line: Some(6),
            end_line: Some(7),
            observed_at: Some("2023-11-14T22:13:20Z".into()),
        },
        ReconciliationAlertV1 {
            source: ReconciliationSourceV1::CodeScanning,
            number: 2,
            severity: SeverityV1::Critical,
            lifecycle: ReconciliationLifecycleV1::Open,
            scope: ReconciliationScopeV1::RepositorySnapshot,
            title: "Deterministic code-scanning fixture alert".into(),
            description: "Sanitized local reconciliation record.".into(),
            public_url:
                "https://github.com/iii-hq/security-scan-e2e-fixture/security/code-scanning/2"
                    .into(),
            structured_ids: vec!["fixture/command-injection".into()],
            path: Some("src/vulnerable.rs".into()),
            start_line: Some(7),
            end_line: Some(7),
            observed_at: Some("2023-11-14T22:13:20Z".into()),
        },
    ];
    let summary = |source, scope, count| ReconciliationSourceSummaryV1 {
        source,
        status: ReconciliationSourceStatusV1::Complete,
        scope,
        collected_at: Some(DETERMINISTIC_TIME_MS),
        record_count: Some(count),
        health: ReconciliationSourceHealthV1 {
            status: ReconciliationHealthStatusV1::Healthy,
            tool: Some("harness-e2e-local-adapter".into()),
            commit_sha: Some(run.target_sha.clone()),
            observed_at: Some("2023-11-14T22:13:20Z".into()),
        },
    };
    SecurityScanReconciliationResponseV1 {
        schema_version: SCHEMA_VERSION.into(),
        run_id: run.run_id.clone(),
        repository: run.repository.clone(),
        target_sha: run.target_sha.clone(),
        harness: HarnessReconciliationSummaryV1 {
            status: HarnessReconciliationStatusV1::Verified,
            verified_count: finding_count,
            verified_at: Some(DETERMINISTIC_TIME_MS),
            scope: ReconciliationScopeV1::ExactCommit,
        },
        github_repository: Some(REPOSITORY.into()),
        sources: vec![
            summary(
                ReconciliationSourceV1::Dependabot,
                ReconciliationScopeV1::RepositoryDefaultBranch,
                1,
            ),
            summary(
                ReconciliationSourceV1::CodeScanning,
                ReconciliationScopeV1::RepositorySnapshot,
                1,
            ),
        ],
        matching: ReconciliationMatchingV1 {
            status: ReconciliationMatchingStatusV1::Available,
            matched_records: Some(0),
        },
        records,
        next_cursor: None,
    }
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    cursor
        .strip_prefix("local:")
        .context("invalid local reconciliation cursor")?
        .parse()
        .context("invalid local reconciliation cursor offset")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema<T: JsonSchema>() -> Value {
        serde_json::to_value(
            schemars::r#gen::SchemaSettings::draft07()
                .into_generator()
                .into_root_schema_for::<T>(),
        )
        .unwrap()
    }

    #[test]
    fn local_adapter_preserves_exact_worker_contract_hashes() {
        let contracts = [
            (
                REQUEST_FUNCTION,
                schema::<SecurityScanRequestV1>(),
                schema::<SecurityScanResponseV1>(),
            ),
            (
                READ_FUNCTION,
                schema::<SecurityScanReadRequestV1>(),
                schema::<SecurityScanReadResponseV1>(),
            ),
            (
                LIST_FUNCTION,
                schema::<SecurityScanListRequestV1>(),
                schema::<SecurityScanListResponseV1>(),
            ),
            (
                RECONCILIATION_FUNCTION,
                schema::<SecurityScanReconciliationRequestV1>(),
                schema::<SecurityScanReconciliationResponseV1>(),
            ),
        ];
        for (function_id, request, response) in contracts {
            let expected = required_contract(function_id);
            assert_eq!(
                expected.request_schema_sha256.as_deref(),
                Some(crate::artifact::sha256_value(&request).unwrap().as_str()),
                "request schema drift for {function_id}"
            );
            assert_eq!(
                expected.response_schema_sha256.as_deref(),
                Some(crate::artifact::sha256_value(&response).unwrap().as_str()),
                "response schema drift for {function_id}"
            );
        }
    }

    #[test]
    fn state_deduplicates_lists_and_persists_reconciliation() {
        let target_sha = "a".repeat(40);
        let adapter = LocalAdapter::new(PathBuf::from("."));
        let request = || SecurityScanRequestV1 {
            repository: REPOSITORY.into(),
            target_sha: target_sha.clone(),
            mode: ScanModeV1::Scan,
            _caller_worker_id: None,
        };
        let first = adapter.record_request(request());
        let duplicate = adapter.record_request(request());
        assert!(!first.deduplicated);
        assert!(duplicate.deduplicated);
        assert_eq!(first.run_id, duplicate.run_id);
        let run_id = first.run_id;
        assert_eq!(
            adapter
                .list(SecurityScanListRequestV1::default())
                .runs
                .len(),
            1
        );
        let refreshed = adapter
            .reconciliation(SecurityScanReconciliationRequestV1 {
                run_id: run_id.clone(),
                refresh: true,
                ..Default::default()
            })
            .unwrap();
        let reread = adapter
            .reconciliation(SecurityScanReconciliationRequestV1 {
                run_id,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(refreshed, reread);
        assert_eq!(reread.records.len(), 2);
    }
}
