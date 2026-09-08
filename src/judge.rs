//! Invocation of the auxiliary ("judge") model.
//!
//! The auxiliary model is used by Markdown scenarios, Registry planning,
//! and the opt-in transcript audit; this module owns
//! the one provider round trip they share and the usage bookkeeping around it.

use anyhow::Result;
use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::report::ModelUsageReport;

#[derive(Debug, Clone)]
pub struct JudgeConfig {
    pub model: String,
    pub provider: String,
}

pub(crate) async fn invoke(
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

pub(crate) fn assistant_text(response: &Value) -> String {
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

pub(crate) fn response_usage(response: &Value) -> Option<ModelUsageReport> {
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

pub(crate) fn aggregate_usage(attempts: &[Option<ModelUsageReport>]) -> Option<ModelUsageReport> {
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_request_does_not_require_native_structured_output() {
        let request = judge_request(
            &JudgeConfig {
                model: "judge".into(),
                provider: "provider".into(),
            },
            "You are an impartial evaluator.",
            "evaluate",
            2_048,
        );
        assert!(request.get("response_format").is_none());
        assert_eq!(request["max_output_tokens"], 2_048);
        assert_eq!(request["model"], "judge");
        assert_eq!(request["provider"], "provider");
    }

    #[test]
    fn aggregates_usage_from_every_attempt() {
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
        assert!(aggregate_usage(&[usage(1, 1, 0.0), None]).is_none());
    }

    #[test]
    fn assistant_text_joins_only_text_blocks() {
        let response = json!({
            "message": {"content": [
                {"type": "text", "text": "{\"a\":"},
                {"type": "tool_use", "name": "ignored"},
                {"type": "text", "text": "1}"},
            ]},
            "usage": {"input": 5, "output": 2, "cost_usd": 0.5},
        });
        assert_eq!(assistant_text(&response), "{\"a\":1}");
        let usage = response_usage(&response).unwrap();
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(2));
        assert_eq!(usage.cost_usd, Some(0.5));
    }
}
