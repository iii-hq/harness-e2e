use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::artifact;
use crate::assessment::{
    AnalyzerIdentity, AnalyzerUsage, AssessmentOutcome, AssessmentResult, AssessmentScore,
    AssetAssessmentResult,
};
use crate::context::E2eContext;
use crate::report::{DeliverableReport, ModelUsageReport};
use crate::scenarios::{CriterionAward, CriterionSpec, ScenarioSpec};

const JUDGE_SYSTEM_PROMPT: &str = "You are an impartial software-agent quality evaluator. \
Score only the supplied answer against the supplied rubric and reference. \
Do not reward claims that are not supported by the answer. Return exactly one JSON object, \
without Markdown or explanatory text.";
const ASSET_SYSTEM_PROMPT: &str = "You are an impartial generated-asset quality evaluator. \
Assess only the bounded, sanitized previews and immutable evidence identities supplied. \
Do not infer content outside that evidence. Return exactly one JSON object, without Markdown \
or explanatory text.";
pub const JUDGE_PROTOCOL: &str = "assessment-json-v2";
pub const CRITERION_ANALYZER_VERSION: &str = "1";
pub const CRITERION_PROMPT_VERSION: &str = "criterion-assessment-v1";
pub const CRITERION_RUBRIC_VERSION: &str = "criterion-rubric-v1";
pub const ASSET_ANALYZER_VERSION: &str = "1";
pub const ASSET_PROMPT_VERSION: &str = "asset-quality-v1";
pub const ASSET_RUBRIC_VERSION: &str = "asset-quality-rubric-v1";
const MAX_JUDGE_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone)]
pub struct JudgeConfig {
    pub model: String,
    pub provider: String,
}

pub struct JudgeOutcome {
    pub awards: Vec<CriterionAward>,
    pub confidences: HashMap<String, f64>,
    pub attempts: u8,
    pub usage: Option<ModelUsageReport>,
    pub analyzer: AnalyzerIdentity,
    pub analyzer_usage: AnalyzerUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeFailureKind {
    Unavailable,
    MalformedOutput,
    Timeout,
    Infrastructure,
}

impl JudgeFailureKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "judge_unavailable",
            Self::MalformedOutput => "judge_malformed_output",
            Self::Timeout => "judge_timeout",
            Self::Infrastructure => "judge_infrastructure",
        }
    }

    pub fn outcome(self) -> AssessmentOutcome {
        match self {
            Self::Unavailable => AssessmentOutcome::Unavailable,
            Self::MalformedOutput | Self::Timeout | Self::Infrastructure => {
                AssessmentOutcome::Error
            }
        }
    }
}

#[derive(Debug)]
pub struct JudgeFailure {
    pub kind: JudgeFailureKind,
    pub message: String,
    pub attempts: u8,
    pub usage: Option<ModelUsageReport>,
    pub analyzer: AnalyzerIdentity,
    pub analyzer_usage: AnalyzerUsage,
}

impl JudgeFailure {
    pub fn summary(&self) -> String {
        format!("{}: {}", self.kind.code(), self.message)
    }
}

impl fmt::Display for JudgeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary())
    }
}

impl std::error::Error for JudgeFailure {}

pub enum JudgeEvaluation {
    Completed(JudgeOutcome),
    Failed(JudgeFailure),
}

pub struct AssetJudgeOutcome {
    pub results: Vec<AssessmentResult>,
    pub attempts: u8,
    pub usage: Option<ModelUsageReport>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JudgeResponse {
    criteria: Vec<JudgeCriterion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JudgeCriterion {
    id: String,
    awarded: u8,
    confidence: f64,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetJudgeResponse {
    assets: Vec<AssetJudgeResult>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetJudgeResult {
    asset_id: String,
    awarded: u8,
    confidence: f64,
    summary: String,
    evidence: Vec<AssetJudgeEvidence>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
struct AssetJudgeEvidence {
    artifact_id: String,
    artifact_sha256: String,
}

pub async fn evaluate(
    context: &E2eContext,
    config: &JudgeConfig,
    spec: &ScenarioSpec,
    answer: &str,
) -> Result<JudgeEvaluation> {
    if spec.criteria.is_empty() {
        bail!("scenario {} has no judge criteria", spec.id);
    }
    let reference = spec
        .judge_reference
        .as_ref()
        .ok_or_else(|| anyhow!("scenario {} has no judge reference", spec.id))?;
    let rubric: Vec<_> = spec
        .criteria
        .iter()
        .map(|criterion| {
            json!({
                "id": criterion.id,
                "possible": criterion.weight,
                "description": criterion.description,
            })
        })
        .collect();
    let input = json!({
        "prompt_version": CRITERION_PROMPT_VERSION,
        "rubric_version": CRITERION_RUBRIC_VERSION,
        "task_prompt": spec.prompt,
        "assistant_answer": answer,
        "reference": reference,
        "rubric": rubric,
    });
    let response_template = json!({
        "criteria": spec.criteria.iter().map(|criterion| {
            json!({
                "id": criterion.id,
                "awarded": 0,
                "confidence": 0.0,
                "reason": "brief evidence-based justification",
            })
        }).collect::<Vec<_>>(),
    });
    let prompt = format!(
        "Evaluate this case:\n{}\n\n\
Your response must satisfy this JSON Schema:\n{}\n\n\
Include every rubric id exactly once. For each criterion, `awarded` must be an \
integer from zero through that criterion's `possible` value, not a percentage. \
`confidence` must be a number from 0 through 1. \
Use this exact object shape and replace only the scores and explanatory text:\n{}",
        serde_json::to_string(&input).context("serialize judge input")?,
        serde_json::to_string(&response_schema()).context("serialize judge response schema")?,
        serde_json::to_string(&response_template).context("serialize judge response template")?,
    );
    let mut attempt_prompt = prompt.clone();
    let mut attempt_usage = Vec::new();
    let started = Instant::now();
    let input_sha256 = artifact::sha256_value(&input).context("hash criterion judge input")?;
    let analyzer = analyzer_identity(
        "criterion-assessment",
        CRITERION_ANALYZER_VERSION,
        CRITERION_PROMPT_VERSION,
        config,
        input_sha256,
    );

    for attempt in 1..=MAX_JUDGE_ATTEMPTS {
        let response =
            match invoke(context, config, JUDGE_SYSTEM_PROMPT, &attempt_prompt, 2_048).await {
                Ok(response) => response,
                Err(error) => {
                    let latency_ms = elapsed_ms(started);
                    let usage = aggregate_usage(&attempt_usage);
                    return Ok(JudgeEvaluation::Failed(JudgeFailure {
                        kind: classify_invocation_failure(&error),
                        message: format!("invoke E2E judge attempt {attempt}: {error:#}"),
                        attempts: attempt,
                        analyzer_usage: analyzer_usage(usage.as_ref(), latency_ms),
                        usage,
                        analyzer,
                    }));
                }
            };
        attempt_usage.push(response_usage(&response));
        let response_text = assistant_text(&response);

        match parse_response(&response_text)
            .and_then(|parsed| validate_response(&spec.criteria, parsed))
        {
            Ok((awards, confidences)) => {
                let latency_ms = elapsed_ms(started);
                let usage = aggregate_usage(&attempt_usage);
                return Ok(JudgeEvaluation::Completed(JudgeOutcome {
                    awards,
                    confidences,
                    attempts: attempt,
                    analyzer_usage: analyzer_usage(usage.as_ref(), latency_ms),
                    usage,
                    analyzer,
                }));
            }
            Err(error) if attempt < MAX_JUDGE_ATTEMPTS => {
                attempt_prompt = repair_prompt(&prompt, &error, &response_text, attempt);
            }
            Err(error) => {
                let latency_ms = elapsed_ms(started);
                let usage = aggregate_usage(&attempt_usage);
                return Ok(JudgeEvaluation::Failed(JudgeFailure {
                    kind: JudgeFailureKind::MalformedOutput,
                    message: format!(
                        "invalid rubric result after {attempt} attempts: {error:#}; response: {}",
                        response_excerpt(&response_text)
                    ),
                    attempts: attempt,
                    analyzer_usage: analyzer_usage(usage.as_ref(), latency_ms),
                    usage,
                    analyzer,
                }));
            }
        }
    }

    unreachable!("judge attempt loop always returns")
}

pub async fn evaluate_asset_quality(
    context: &E2eContext,
    config: &JudgeConfig,
    deliverables: &[DeliverableReport],
    assessments: &[AssetAssessmentResult],
) -> Result<AssetJudgeOutcome> {
    let deliverables = deliverables
        .iter()
        .map(|deliverable| (deliverable.id.as_str(), deliverable))
        .collect::<HashMap<_, _>>();
    let mut results = assessments
        .iter()
        .map(|asset| {
            let mut result = asset.qualitative_assessment.clone();
            result.evidence = asset.validation.evidence.clone();
            result.summary = if result.evidence.is_empty() {
                "Asset quality was not evaluated because no immutable content evidence was captured."
                    .into()
            } else if !deliverables.contains_key(asset.validation.asset_id.as_str()) {
                "Asset quality was not evaluated because no bounded captured preview was available."
                    .into()
            } else {
                result.summary
            };
            result
        })
        .collect::<Vec<_>>();
    let assessable = assessments
        .iter()
        .enumerate()
        .filter_map(|(index, asset)| {
            let deliverable = deliverables.get(asset.validation.asset_id.as_str())?;
            (!asset.validation.evidence.is_empty()).then_some((index, asset, *deliverable))
        })
        .collect::<Vec<_>>();
    if assessable.is_empty() {
        return Ok(AssetJudgeOutcome {
            results,
            attempts: 0,
            usage: None,
        });
    }

    let input = json!({
        "prompt_version": ASSET_PROMPT_VERSION,
        "rubric_version": ASSET_RUBRIC_VERSION,
        "rubric": {
            "possible": 100,
            "dimensions": [
                "correctness against the declared asset contract",
                "completeness of the generated content",
                "clarity and internal coherence",
                "practical usability for the declared purpose"
            ],
            "instruction": "Use only the supplied bounded preview and cite every immutable evidence identity inspected."
        },
        "assets": assessable.iter().map(|(_, asset, deliverable)| json!({
            "asset_id": asset.validation.asset_id,
            "validation_outcome": asset.validation.outcome,
            "validation_summary": asset.validation.summary,
            "media_type": deliverable.media_type,
            "content_size_bytes": deliverable.content_size_bytes,
            "preview": deliverable.preview,
            "evidence": asset.validation.evidence,
        })).collect::<Vec<_>>(),
    });
    let response_template = json!({
        "assets": assessable.iter().map(|(_, asset, _)| json!({
            "asset_id": asset.validation.asset_id,
            "awarded": 0,
            "confidence": 0.0,
            "summary": "brief evidence-based qualitative conclusion",
            "evidence": asset.validation.evidence.iter().map(|evidence| json!({
                "artifact_id": evidence.artifact_id,
                "artifact_sha256": evidence.artifact_sha256,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    let prompt = format!(
        "Assess these captured assets:\n{}\n\n\
Your response must satisfy this JSON Schema:\n{}\n\n\
Include every asset id exactly once. `awarded` must be an integer from 0 through 100 and \
`confidence` a number from 0 through 1. Copy the exact immutable evidence identities \
you inspected. Use this object shape and replace only scores, confidence, and summaries:\n{}",
        serde_json::to_string(&input).context("serialize asset judge input")?,
        serde_json::to_string(&asset_response_schema())
            .context("serialize asset judge response schema")?,
        serde_json::to_string(&response_template)
            .context("serialize asset judge response template")?,
    );
    let input_sha256 = artifact::sha256_value(&input).context("hash asset judge input")?;
    let analyzer = analyzer_identity(
        "asset-quality",
        ASSET_ANALYZER_VERSION,
        ASSET_PROMPT_VERSION,
        config,
        input_sha256,
    );
    let mut attempt_prompt = prompt.clone();
    let mut attempt_usage = Vec::new();
    let started = Instant::now();

    for attempt in 1..=MAX_JUDGE_ATTEMPTS {
        let response =
            match invoke(context, config, ASSET_SYSTEM_PROMPT, &attempt_prompt, 8_192).await {
                Ok(response) => response,
                Err(error) => {
                    let latency_ms = elapsed_ms(started);
                    let usage = aggregate_usage(&attempt_usage);
                    let failure = JudgeFailure {
                        kind: classify_invocation_failure(&error),
                        message: format!("invoke asset judge attempt {attempt}: {error:#}"),
                        attempts: attempt,
                        analyzer_usage: analyzer_usage(usage.as_ref(), latency_ms),
                        usage: usage.clone(),
                        analyzer: analyzer.clone(),
                    };
                    apply_asset_failure(&mut results, &assessable, &failure);
                    return Ok(AssetJudgeOutcome {
                        results,
                        attempts: attempt,
                        usage,
                    });
                }
            };
        attempt_usage.push(response_usage(&response));
        let response_text = assistant_text(&response);
        match parse_asset_response(&response_text)
            .and_then(|response| validate_asset_response(&assessable, response))
        {
            Ok(assessed) => {
                let latency_ms = elapsed_ms(started);
                let usage = aggregate_usage(&attempt_usage);
                let analyzer_usage = analyzer_usage(usage.as_ref(), latency_ms);
                apply_asset_success(
                    &mut results,
                    &assessable,
                    assessed,
                    &analyzer,
                    &analyzer_usage,
                );
                return Ok(AssetJudgeOutcome {
                    results,
                    attempts: attempt,
                    usage,
                });
            }
            Err(error) if attempt < MAX_JUDGE_ATTEMPTS => {
                attempt_prompt = repair_prompt(&prompt, &error, &response_text, attempt);
            }
            Err(error) => {
                let latency_ms = elapsed_ms(started);
                let usage = aggregate_usage(&attempt_usage);
                let failure = JudgeFailure {
                    kind: JudgeFailureKind::MalformedOutput,
                    message: format!(
                        "invalid asset result after {attempt} attempts: {error:#}; response: {}",
                        response_excerpt(&response_text)
                    ),
                    attempts: attempt,
                    analyzer_usage: analyzer_usage(usage.as_ref(), latency_ms),
                    usage: usage.clone(),
                    analyzer,
                };
                apply_asset_failure(&mut results, &assessable, &failure);
                return Ok(AssetJudgeOutcome {
                    results,
                    attempts: attempt,
                    usage,
                });
            }
        }
    }

    unreachable!("asset judge attempt loop always returns")
}

async fn invoke(
    context: &E2eContext,
    config: &JudgeConfig,
    system_prompt: &str,
    prompt: &str,
    max_output_tokens: u64,
) -> Result<Value> {
    context
        .trigger_value(
            "router::complete",
            judge_request(config, system_prompt, prompt, max_output_tokens),
        )
        .await
}

fn judge_request(
    config: &JudgeConfig,
    system_prompt: &str,
    prompt: &str,
    max_output_tokens: u64,
) -> Value {
    json!({
        "model": config.model,
        "provider": config.provider,
        "system_prompt": system_prompt,
        "messages": [{
            "role": "user",
            "content": [{
                "type": "text",
                "text": prompt,
            }],
            "timestamp": now_ms() as i64,
        }],
        "max_output_tokens": max_output_tokens,
    })
}

fn repair_prompt(base_prompt: &str, error: &anyhow::Error, response: &str, attempt: u8) -> String {
    format!(
        "{base_prompt}\n\nYour response from attempt {attempt} was invalid.\n\
Validation error: {error:#}\n\
Previous response:\n{response}\n\nReturn a corrected JSON object only."
    )
}

fn parse_response(text: &str) -> Result<JudgeResponse> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("judge response contains no JSON object"))?;
    let end = text
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| anyhow!("judge response contains no complete JSON object"))?;
    serde_json::from_str(&text[start..=end]).context("judge returned invalid JSON")
}

fn validate_response(
    criteria: &[CriterionSpec],
    response: JudgeResponse,
) -> Result<(Vec<CriterionAward>, HashMap<String, f64>)> {
    let expected: HashMap<_, _> = criteria
        .iter()
        .map(|criterion| (criterion.id, criterion.weight))
        .collect();
    let mut seen = HashSet::new();
    let mut awards = Vec::with_capacity(response.criteria.len());
    let mut confidences = HashMap::with_capacity(response.criteria.len());
    for result in response.criteria {
        if !seen.insert(result.id.clone()) {
            bail!("judge repeated criterion {}", result.id);
        }
        let possible = expected
            .get(result.id.as_str())
            .ok_or_else(|| anyhow!("judge returned unknown criterion {}", result.id))?;
        if result.awarded > *possible {
            bail!(
                "judge awarded {} of {} points for {}",
                result.awarded,
                possible,
                result.id
            );
        }
        if result.reason.trim().is_empty() {
            bail!("judge returned no reason for {}", result.id);
        }
        if !result.confidence.is_finite() || !(0.0..=1.0).contains(&result.confidence) {
            bail!(
                "judge returned confidence {} outside 0..=1 for {}",
                result.confidence,
                result.id
            );
        }
        confidences.insert(result.id.clone(), result.confidence);
        awards.push(CriterionAward {
            id: result.id,
            awarded: result.awarded,
            reason: result.reason,
        });
    }
    for id in expected.keys() {
        if !seen.contains(*id) {
            bail!("judge omitted criterion {id}");
        }
    }
    Ok((awards, confidences))
}

fn parse_asset_response(text: &str) -> Result<AssetJudgeResponse> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("asset judge response contains no JSON object"))?;
    let end = text
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| anyhow!("asset judge response contains no complete JSON object"))?;
    serde_json::from_str(&text[start..=end]).context("asset judge returned invalid JSON")
}

fn validate_asset_response(
    assessable: &[(usize, &AssetAssessmentResult, &DeliverableReport)],
    response: AssetJudgeResponse,
) -> Result<HashMap<String, AssetJudgeResult>> {
    let expected = assessable
        .iter()
        .map(|(_, asset, _)| (asset.validation.asset_id.as_str(), *asset))
        .collect::<HashMap<_, _>>();
    let mut observed = HashMap::with_capacity(response.assets.len());
    for result in response.assets {
        let asset = expected
            .get(result.asset_id.as_str())
            .ok_or_else(|| anyhow!("asset judge returned unknown asset {}", result.asset_id))?;
        if result.awarded > 100 {
            bail!(
                "asset judge awarded {} of 100 points for {}",
                result.awarded,
                result.asset_id
            );
        }
        if !result.confidence.is_finite() || !(0.0..=1.0).contains(&result.confidence) {
            bail!(
                "asset judge returned confidence {} outside 0..=1 for {}",
                result.confidence,
                result.asset_id
            );
        }
        if result.summary.trim().is_empty() {
            bail!("asset judge returned no summary for {}", result.asset_id);
        }
        let expected_evidence = asset
            .validation
            .evidence
            .iter()
            .map(|evidence| {
                (
                    evidence.artifact_id.as_str(),
                    evidence.artifact_sha256.as_str(),
                )
            })
            .collect::<HashSet<_>>();
        let observed_evidence = result
            .evidence
            .iter()
            .map(|evidence| {
                (
                    evidence.artifact_id.as_str(),
                    evidence.artifact_sha256.as_str(),
                )
            })
            .collect::<HashSet<_>>();
        if observed_evidence.len() != result.evidence.len()
            || observed_evidence != expected_evidence
        {
            bail!(
                "asset judge evidence for {} differs from the bounded captured evidence",
                result.asset_id
            );
        }
        if observed.insert(result.asset_id.clone(), result).is_some() {
            bail!("asset judge repeated an asset result");
        }
    }
    for asset_id in expected.keys() {
        if !observed.contains_key(*asset_id) {
            bail!("asset judge omitted asset {asset_id}");
        }
    }
    Ok(observed)
}

fn apply_asset_failure(
    results: &mut [AssessmentResult],
    assessable: &[(usize, &AssetAssessmentResult, &DeliverableReport)],
    failure: &JudgeFailure,
) {
    for (index, asset, _) in assessable {
        results[*index] = AssessmentResult {
            outcome: failure.kind.outcome(),
            score: None,
            confidence: None,
            summary: failure.summary(),
            evidence: asset.validation.evidence.clone(),
            analyzer: Some(failure.analyzer.clone()),
            analyzer_usage: Some(failure.analyzer_usage.clone()),
            ..asset.qualitative_assessment.clone()
        };
    }
}

fn apply_asset_success(
    results: &mut [AssessmentResult],
    assessable: &[(usize, &AssetAssessmentResult, &DeliverableReport)],
    mut assessed: HashMap<String, AssetJudgeResult>,
    analyzer: &AnalyzerIdentity,
    analyzer_usage: &AnalyzerUsage,
) {
    for (index, asset, _) in assessable {
        let assessed = assessed
            .remove(asset.validation.asset_id.as_str())
            .expect("validated response contains every asset");
        results[*index] = AssessmentResult {
            outcome: score_outcome(assessed.awarded, 100),
            score: Some(AssessmentScore {
                awarded: assessed.awarded,
                possible: 100,
            }),
            confidence: Some(assessed.confidence),
            summary: assessed.summary,
            evidence: asset.validation.evidence.clone(),
            analyzer: Some(analyzer.clone()),
            analyzer_usage: Some(analyzer_usage.clone()),
            ..asset.qualitative_assessment.clone()
        };
    }
}

fn score_outcome(awarded: u8, possible: u8) -> AssessmentOutcome {
    if awarded == possible {
        AssessmentOutcome::Passed
    } else if awarded == 0 {
        AssessmentOutcome::Failed
    } else {
        AssessmentOutcome::Partial
    }
}

fn assistant_text(response: &Value) -> String {
    response
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

fn response_usage(response: &Value) -> Option<ModelUsageReport> {
    let usage = response.get("usage")?;
    Some(ModelUsageReport {
        input_tokens: usage.get("input").and_then(Value::as_u64),
        output_tokens: usage.get("output").and_then(Value::as_u64),
        cache_read_tokens: usage.get("cache_read").and_then(Value::as_u64),
        cache_write_tokens: usage.get("cache_write").and_then(Value::as_u64),
        reasoning_tokens: usage.get("reasoning").and_then(Value::as_u64),
        cost_usd: usage.get("cost_usd").and_then(Value::as_f64),
    })
}

fn aggregate_usage(attempts: &[Option<ModelUsageReport>]) -> Option<ModelUsageReport> {
    let usages: Option<Vec<_>> = attempts.iter().map(Option::as_ref).collect();
    let usages = usages?;
    if usages.is_empty() {
        return None;
    }
    Some(ModelUsageReport {
        input_tokens: sum_u64(usages.iter().map(|usage| usage.input_tokens)),
        output_tokens: sum_u64(usages.iter().map(|usage| usage.output_tokens)),
        cache_read_tokens: sum_u64(usages.iter().map(|usage| usage.cache_read_tokens)),
        cache_write_tokens: sum_u64(usages.iter().map(|usage| usage.cache_write_tokens)),
        reasoning_tokens: sum_u64(usages.iter().map(|usage| usage.reasoning_tokens)),
        cost_usd: usages
            .iter()
            .map(|usage| usage.cost_usd)
            .try_fold(0.0, |total, value| Some(total + value?)),
    })
}

fn sum_u64(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values
        .into_iter()
        .try_fold(0_u64, |total, value| total.checked_add(value?))
}

fn response_excerpt(response: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let mut excerpt: String = response.chars().take(MAX_CHARS).collect();
    if response.chars().count() > MAX_CHARS {
        excerpt.push('…');
    }
    excerpt
}

fn response_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["criteria"],
        "properties": {
            "criteria": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "awarded", "confidence", "reason"],
                    "properties": {
                        "id": { "type": "string" },
                        "awarded": { "type": "integer", "minimum": 0, "maximum": 100 },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                        "reason": { "type": "string" }
                    }
                }
            }
        }
    })
}

fn asset_response_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["assets"],
        "properties": {
            "assets": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["asset_id", "awarded", "confidence", "summary", "evidence"],
                    "properties": {
                        "asset_id": { "type": "string" },
                        "awarded": { "type": "integer", "minimum": 0, "maximum": 100 },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                        "summary": { "type": "string" },
                        "evidence": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["artifact_id", "artifact_sha256"],
                                "properties": {
                                    "artifact_id": { "type": "string" },
                                    "artifact_sha256": {
                                        "type": "string",
                                        "pattern": "^sha256:[0-9a-f]{64}$"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn analyzer_identity(
    analyzer: &str,
    analyzer_version: &str,
    prompt_version: &str,
    config: &JudgeConfig,
    input_sha256: String,
) -> AnalyzerIdentity {
    AnalyzerIdentity {
        analyzer: analyzer.to_string(),
        analyzer_version: analyzer_version.to_string(),
        provider: Some(config.provider.clone()),
        model: Some(config.model.clone()),
        prompt_version: prompt_version.to_string(),
        input_sha256,
    }
}

fn analyzer_usage(usage: Option<&ModelUsageReport>, latency_ms: u64) -> AnalyzerUsage {
    AnalyzerUsage {
        latency_ms: Some(latency_ms),
        input_tokens: usage.and_then(|usage| usage.input_tokens),
        output_tokens: usage.and_then(|usage| usage.output_tokens),
        cost_usd: usage.and_then(|usage| usage.cost_usd),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn classify_invocation_failure(error: &anyhow::Error) -> JudgeFailureKind {
    let message = format!("{error:#}").to_ascii_lowercase();
    if ["timed out", "timeout", "deadline exceeded"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        JudgeFailureKind::Timeout
    } else if [
        "unavailable",
        "not configured",
        "unknown provider",
        "unknown model",
        "model not found",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        JudgeFailureKind::Unavailable
    } else {
        JudgeFailureKind::Infrastructure
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured_asset() -> (AssetAssessmentResult, DeliverableReport) {
        let evidence = crate::assessment::EvidenceReference {
            artifact_id: "result".into(),
            artifact_sha256: format!("sha256:{}", "b".repeat(64)),
            locator: None,
        };
        let asset = AssetAssessmentResult {
            validation: crate::assessment::AssetValidationResult {
                asset_id: "result".into(),
                outcome: crate::assessment::AssetValidationOutcome::Valid,
                summary: "deterministic validation passed".into(),
                evidence: vec![evidence],
            },
            qualitative_assessment: AssessmentResult {
                criterion_id: "asset_quality".into(),
                target: crate::assessment::AssessmentTarget {
                    kind: crate::assessment::AssessmentTargetKind::Asset,
                    id: "result".into(),
                },
                kind: crate::assessment::AssessmentKind::AssetQuality,
                policy: crate::assessment::AssessmentPolicy::Advisory,
                dimension: crate::report::EvaluationDimension::Deliverable,
                source: crate::assessment::AssessmentSource::AssetAnalyzer,
                outcome: AssessmentOutcome::NotEvaluated,
                score: None,
                confidence: None,
                summary: "not evaluated".into(),
                evidence: Vec::new(),
                analyzer: None,
                analyzer_usage: None,
            },
        };
        let deliverable = DeliverableReport {
            id: "result".into(),
            kind: "state_value".into(),
            media_type: "application/json".into(),
            content_sha256: format!("sha256:{}", "c".repeat(64)),
            content_size_bytes: 18,
            schema_valid: true,
            provenance_valid: true,
            invariants: Vec::new(),
            provenance: Vec::new(),
            preview: json!({"status": "ready"}),
            artifact: None,
            content: json!({"status": "ready"}),
        };
        (asset, deliverable)
    }

    fn test_analyzer() -> AnalyzerIdentity {
        AnalyzerIdentity {
            analyzer: "asset-quality".into(),
            analyzer_version: ASSET_ANALYZER_VERSION.into(),
            provider: Some("provider".into()),
            model: Some("model".into()),
            prompt_version: ASSET_PROMPT_VERSION.into(),
            input_sha256: format!("sha256:{}", "a".repeat(64)),
        }
    }

    fn criteria() -> Vec<CriterionSpec> {
        vec![
            CriterionSpec::advisory_judge("correctness", 70, "correct"),
            CriterionSpec::advisory_judge("clarity", 30, "clear"),
        ]
    }

    #[test]
    fn qualitative_asset_review_preserves_exact_evidence_and_analyzer_identity() {
        let (asset, deliverable) = captured_asset();
        let assessable = vec![(0, &asset, &deliverable)];
        let assessed = validate_asset_response(
            &assessable,
            AssetJudgeResponse {
                assets: vec![AssetJudgeResult {
                    asset_id: "result".into(),
                    awarded: 74,
                    confidence: 0.82,
                    summary: "The captured result is coherent but omits one useful detail.".into(),
                    evidence: vec![AssetJudgeEvidence {
                        artifact_id: "result".into(),
                        artifact_sha256: format!("sha256:{}", "b".repeat(64)),
                    }],
                }],
            },
        )
        .unwrap();
        let mut results = vec![asset.qualitative_assessment.clone()];
        let usage = AnalyzerUsage {
            latency_ms: Some(25),
            input_tokens: Some(100),
            output_tokens: Some(30),
            cost_usd: Some(0.01),
        };
        apply_asset_success(
            &mut results,
            &assessable,
            assessed,
            &test_analyzer(),
            &usage,
        );

        assert_eq!(results[0].outcome, AssessmentOutcome::Partial);
        assert_eq!(results[0].score.as_ref().unwrap().awarded, 74);
        assert_eq!(results[0].confidence, Some(0.82));
        assert_eq!(results[0].evidence, asset.validation.evidence);
        assert_eq!(
            results[0].analyzer.as_ref().unwrap().prompt_version,
            ASSET_PROMPT_VERSION
        );
        results[0].validate().unwrap();
    }

    #[test]
    fn malformed_asset_output_becomes_an_explicit_advisory_error() {
        let (asset, deliverable) = captured_asset();
        let assessable = vec![(0, &asset, &deliverable)];
        let malformed = AssetJudgeResponse {
            assets: vec![AssetJudgeResult {
                asset_id: "result".into(),
                awarded: 90,
                confidence: 0.9,
                summary: "looks good".into(),
                evidence: vec![AssetJudgeEvidence {
                    artifact_id: "result".into(),
                    artifact_sha256: format!("sha256:{}", "d".repeat(64)),
                }],
            }],
        };
        assert!(validate_asset_response(&assessable, malformed).is_err());

        let failure = JudgeFailure {
            kind: JudgeFailureKind::MalformedOutput,
            message: "evidence identity differs".into(),
            attempts: 3,
            usage: None,
            analyzer: test_analyzer(),
            analyzer_usage: AnalyzerUsage {
                latency_ms: Some(30),
                ..AnalyzerUsage::default()
            },
        };
        let mut results = vec![asset.qualitative_assessment.clone()];
        apply_asset_failure(&mut results, &assessable, &failure);

        assert_eq!(
            asset.validation.outcome,
            crate::assessment::AssetValidationOutcome::Valid
        );
        assert_eq!(results[0].outcome, AssessmentOutcome::Error);
        assert!(results[0].summary.starts_with("judge_malformed_output:"));
        assert_eq!(results[0].evidence, asset.validation.evidence);
        results[0].validate().unwrap();
    }

    #[test]
    fn judge_transport_failures_have_stable_classifications() {
        assert_eq!(
            classify_invocation_failure(&anyhow!("request timed out")),
            JudgeFailureKind::Timeout
        );
        assert_eq!(
            classify_invocation_failure(&anyhow!("provider unavailable")),
            JudgeFailureKind::Unavailable
        );
        assert_eq!(
            classify_invocation_failure(&anyhow!("connection reset")),
            JudgeFailureKind::Infrastructure
        );
    }

    #[test]
    fn accepts_exact_bounded_criterion_set() {
        let specs = criteria();
        let (awards, confidences) = validate_response(
            &specs,
            JudgeResponse {
                criteria: vec![
                    JudgeCriterion {
                        id: "correctness".into(),
                        awarded: 60,
                        confidence: 0.8,
                        reason: "mostly correct".into(),
                    },
                    JudgeCriterion {
                        id: "clarity".into(),
                        awarded: 30,
                        confidence: 0.9,
                        reason: "clear".into(),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(awards.iter().map(|award| award.awarded).sum::<u8>(), 90);
        assert_eq!(confidences["correctness"], 0.8);
    }

    #[test]
    fn parses_a_json_object_even_when_the_provider_ignores_response_format() {
        let response = parse_response("```json\n{\"criteria\":[]}\n```").unwrap();
        assert!(response.criteria.is_empty());
    }

    #[test]
    fn portable_request_does_not_require_native_structured_output() {
        let request = judge_request(
            &JudgeConfig {
                model: "judge".into(),
                provider: "provider".into(),
            },
            JUDGE_SYSTEM_PROMPT,
            "evaluate",
            2_048,
        );
        assert!(request.get("response_format").is_none());
        assert_eq!(request["max_output_tokens"], 2_048);
    }

    #[test]
    fn aggregates_usage_from_every_judge_attempt() {
        let usage = |input, output, cost| {
            Some(ModelUsageReport {
                input_tokens: Some(input),
                output_tokens: Some(output),
                cost_usd: Some(cost),
                ..ModelUsageReport::default()
            })
        };
        let total = aggregate_usage(&[usage(100, 10, 0.01), usage(120, 12, 0.02)]).unwrap();
        assert_eq!(total.input_tokens, Some(220));
        assert_eq!(total.output_tokens, Some(22));
        assert_eq!(total.cost_usd, Some(0.03));
    }

    #[test]
    fn rejects_missing_unknown_duplicate_and_excessive_scores() {
        let specs = criteria();
        for criteria in [
            vec![JudgeCriterion {
                id: "correctness".into(),
                awarded: 60,
                confidence: 0.8,
                reason: "ok".into(),
            }],
            vec![
                JudgeCriterion {
                    id: "correctness".into(),
                    awarded: 60,
                    confidence: 0.8,
                    reason: "ok".into(),
                },
                JudgeCriterion {
                    id: "unknown".into(),
                    awarded: 10,
                    confidence: 0.8,
                    reason: "no".into(),
                },
            ],
            vec![
                JudgeCriterion {
                    id: "correctness".into(),
                    awarded: 60,
                    confidence: 0.8,
                    reason: "ok".into(),
                },
                JudgeCriterion {
                    id: "correctness".into(),
                    awarded: 10,
                    confidence: 0.8,
                    reason: "again".into(),
                },
            ],
            vec![
                JudgeCriterion {
                    id: "correctness".into(),
                    awarded: 71,
                    confidence: 0.8,
                    reason: "too high".into(),
                },
                JudgeCriterion {
                    id: "clarity".into(),
                    awarded: 20,
                    confidence: 0.8,
                    reason: "ok".into(),
                },
            ],
        ] {
            assert!(validate_response(&specs, JudgeResponse { criteria },).is_err());
        }
    }
}
