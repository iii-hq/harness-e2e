//! `result_bounds` — the two ceilings on a function result and the media
//! rule, measured on what the Harness stores and on what the model is sent.
//! Two short exchanges call run-scoped fixtures: a ~120 KB result (under the
//! Harness byte cap, over the context-manager token cap), a ~300 KiB result
//! returned once as a success and once as an error, then a PNG. A
//! `pre_generate` observer, last in the chain and open on error, records the
//! messages each step of the attempt's session is sent.
//!
//! - Byte cap (`harness` `max_result_bytes`, 256 KiB): the 300 KiB results
//!   are stored as `<omitted: result was ~…>` with `result_capped`, keeping
//!   `is_error`; the 120 KB one is stored whole.
//! - Token cap (context-manager `max_result_tokens`): the 120 KB result
//!   reaches the model as head `[…result capped: was ~…]` tail, without its
//!   middle, byte-identical on every step that carries it.
//! - Media: a model without vision gets the PNG as the text-only marker on
//!   every step; one with vision sees the image on the next step and the
//!   omitted-after-use marker after that.

use iii_sdk::protocol::RegisterTriggerInput;
use iii_sdk::RegisterFunction;
use serde_json::{json, Value};

use crate::context::E2eContext;

use super::assessment::{self, AssessmentSpec};
use super::mechanism::{self, record, Check};
use super::{
    async_trait, common, Capability, CapturedDeliverable, ExecutionPolicy, ObjectiveEvaluation,
    Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "result_bounds";
const DELIVERABLE_ID: &str = "result_bounds_checks";
const PRE_GENERATE: &str = "harness::hook::pre-generate";
/// The Harness default `max_result_bytes`.
const MAX_RESULT_BYTES: u64 = 262_144;
/// Over the cap however the result is framed.
const BIG_BYTES: usize = 300 * 1024;
/// Stored twice (content and details), so ~240 KB against the byte cap,
/// and ~30k tokens against the 20k-token cap on what the model is sent.
const WIDE_BYTES: usize = 120_000;
const TOKEN_CAP_MARKER: &str = "[…result capped: was ~";
const BYTE_CAP_MARKER: &str = "<omitted: result was ~";
const TEXT_ONLY_IMAGE: &str = "[image omitted: model does not support vision]";
const TOOL_IMAGE_AFTER_USE: &str =
    "[tool image omitted after use; re-call the function if visual details are needed]";
/// A 1x1 PNG.
const PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

const BYTE_CAP: AssessmentSpec = AssessmentSpec::gate(
    "byte_cap",
    35,
    "Were the ~300 KiB success and error results stored as the `<omitted: result was ~…>` marker with `result_capped.original_bytes` over 262144 and their `is_error` kept, while the ~120 KB result was stored whole?",
)
.labeled("Over-cap results stored as a marker, is_error kept");
const TOKEN_CAP: AssessmentSpec = AssessmentSpec::gate(
    "token_cap",
    35,
    "Was the ~120 KB result sent to the model as head, `[…result capped: was ~…]` and tail, without its middle, byte-identical on every step that carried it?",
)
.labeled("A long result reaches the model capped, the same every step");
const MEDIA: AssessmentSpec = AssessmentSpec::gate(
    "media_by_vision",
    30,
    "Did the PNG result reach the model as the catalog's vision support says: the text-only marker on every step without vision, the image and then the omitted-after-use marker with it?",
)
.labeled("An image result follows the model's vision support");
const ASSESSMENTS: &[AssessmentSpec] = &[BYTE_CAP, TOKEN_CAP, MEDIA];

fn big_function(run_id: &str) -> String {
    format!("e2etest::bounds_big_{}", mechanism::short(run_id))
}

fn wide_function(run_id: &str) -> String {
    format!("e2etest::bounds_wide_{}", mechanism::short(run_id))
}

fn png_function(run_id: &str) -> String {
    format!("e2etest::bounds_png_{}", mechanism::short(run_id))
}

fn observer_function(run_id: &str) -> String {
    format!("e2e::bounds::observe::{}", mechanism::short(run_id))
}

fn code(run_id: &str, exchange: usize) -> String {
    let hex = super::stable_seed(&format!("{ID}:{run_id}")) & 0xff_ffff;
    format!("RB-{hex:06x}-{exchange}")
}

fn prompts(run_id: &str) -> [String; 2] {
    let (wide, big, png) = (
        wide_function(run_id),
        big_function(run_id),
        png_function(run_id),
    );
    let (c0, c1) = (code(run_id, 0), code(run_id, 1));
    [
        format!(
            "Make these three calls one at a time, in this order, each exactly once: `{wide}` \
             with `{{}}`, then `{big}` with `{{\"kind\": \"ok\"}}`, then `{big}` with \
             `{{\"kind\": \"error\"}}`. Their outputs are large and the last one reports an \
             error on purpose: do not retry, repeat or describe them. Then reply with exactly \
             `ACK {c0}` and nothing else."
        ),
        format!(
            "Call `{png}` once with `{{}}`; it returns an image, do not describe it. Then reply \
             with exactly `ACK {c1}` and nothing else."
        ),
    ]
}

pub struct ResultBounds;

#[async_trait]
impl Scenario for ResultBounds {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> Option<&'static str> {
        Some("Keep oversized tool results bounded")
    }

    fn summary(&self) -> Option<&'static str> {
        Some("Two short exchanges that call fixtures returning a long text, an oversized success and error, and a PNG; the runner checks what is stored and what the model is sent.")
    }

    fn protects(&self) -> Option<&'static str> {
        Some("A tool result too big for the session or the window is cut to a stable, labeled view instead of breaking the turn, and an image follows the model's vision support. If it fails, one large tool output wedges a worker, floods the context or breaks the prompt cache on every step.")
    }

    /// One retained cohort: the probes do not vary with a seed.
    fn case(&self, _seed: u64) -> anyhow::Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            super::stable_seed(ID),
            json!({ "big_bytes": BIG_BYTES, "wide_bytes": WIDE_BYTES }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::E2eRunScopedFixtures,
                Capability::IiiFunctions,
            ],
            mechanism::contract(DELIVERABLE_ID),
        )
    }

    fn canonical_seed_only(&self) -> bool {
        true
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let [prompt, _] = prompts(run_id);
        ScenarioSpec {
            id: ID,
            prompt,
            filesystem_root: None,
            execution: ExecutionPolicy {
                max_turns: Some(8),
                max_output_tokens: Some(2_048),
                max_total_tokens: Some(400_000),
                stuck_timeout_seconds: 240,
                max_validation_retries: None,
            },
            denied_functions: &[],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    fn dialogue_followups(&self, run_id: &str) -> Vec<String> {
        prompts(run_id).into_iter().skip(1).collect()
    }

    fn required_functions(&self, run_id: &str) -> Vec<String> {
        vec![
            wide_function(run_id),
            big_function(run_id),
            png_function(run_id),
        ]
    }

    fn allowed_functions(&self, run_id: &str) -> Option<Vec<String>> {
        Some(self.required_functions(run_id))
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::reset(run_id);
        let wide = mechanism::marked_text("RB", WIDE_BYTES);
        let function = context.client().register_function(
            wide_function(run_id),
            RegisterFunction::new_async(move |_: Value| {
                let wide = wide.clone();
                async move { Ok::<Value, iii_sdk::errors::Error>(Value::String(wide)) }
            })
            .description("E2E run-scoped report: returns one long text."),
        );
        mechanism::keep_function(run_id, function);

        let big = "B".repeat(BIG_BYTES);
        let function = context.client().register_function(
            big_function(run_id),
            RegisterFunction::new_async(move |input: Value| {
                let big = big.clone();
                async move {
                    Ok::<Value, iii_sdk::errors::Error>(json!({
                        "content": [{ "type": "text", "text": big }],
                        "is_error": input["kind"] == "error",
                    }))
                }
            })
            .description(
                "E2E run-scoped export: returns an oversized result; kind `ok` or `error`.",
            ),
        );
        mechanism::keep_function(run_id, function);

        let function = context.client().register_function(
            png_function(run_id),
            RegisterFunction::new_async(move |_: Value| async move {
                Ok::<Value, iii_sdk::errors::Error>(json!({
                    "content": [{ "type": "image", "mime": "image/png", "data": PNG_BASE64 }],
                }))
            })
            .description("E2E run-scoped snapshot: returns a PNG image."),
        );
        mechanism::keep_function(run_id, function);

        let owner = run_id.to_string();
        let function = context.client().register_function(
            observer_function(run_id),
            RegisterFunction::new_async(move |input: Value| {
                let owner = owner.clone();
                async move {
                    observe(&owner, &input);
                    Ok::<Value, iii_sdk::errors::Error>(json!({ "decision": "continue" }))
                }
            })
            .description("E2E result_bounds: records what each step of the attempt is sent.")
            .metadata(json!({ "internal": true })),
        );
        mechanism::keep_function(run_id, function);
        // Last in the chain, and open on error: it never changes or stops a turn.
        let trigger = context
            .client()
            .register_trigger(RegisterTriggerInput::new(
                PRE_GENERATE,
                observer_function(run_id),
                json!({ "priority": 1000, "timeout_ms": 2000, "on_error": "fail_open" }),
            ))?;
        mechanism::keep_trigger(run_id, trigger);
        Ok(())
    }

    async fn capture(
        &self,
        context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<Vec<CapturedDeliverable>> {
        let steps = mechanism::noted(run_id, "steps").unwrap_or_else(|| json!([]));
        let vision = match mechanism::subject_route(&observation.transcript) {
            Some((model, _)) => context
                .trigger_value("router::models::get", json!({ "id": model }))
                .await
                .ok()
                .and_then(|reply| reply["model"]["supports_vision"].as_bool()),
            None => None,
        };
        for check in checks(&observation.transcript, run_id, &steps, vision) {
            record(run_id, &check.id, check.passed, check.detail);
        }
        Ok(vec![mechanism::deliverable(DELIVERABLE_ID, run_id)])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        run_id: &str,
    ) -> anyhow::Result<ObjectiveEvaluation> {
        Ok(evaluate_checks(
            observation,
            run_id,
            &mechanism::observed_checks(observation, run_id),
        ))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> anyhow::Result<()> {
        mechanism::release(context, run_id).await
    }
}

/// One step of the attempt's session as the model is sent it: the wide
/// result's text and the PNG result's blocks, when the step carries them.
fn observe(run_id: &str, input: &Value) {
    if input["session_id"] != mechanism::session_id(run_id).as_str() {
        return;
    }
    let messages = input["generate"]["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let result = |function: String| {
        messages
            .iter()
            .find(|message| {
                message["role"] == "function_result" && message["function_id"] == function
            })
            .map(|message| message["content"].clone())
    };
    mechanism::append(
        run_id,
        "steps",
        json!({
            "turn_id": input["turn_id"],
            "step": input["step"],
            "wide": result(wide_function(run_id)).map(|content| mechanism::text_of(&json!({ "content": content }))),
            "png": result(png_function(run_id)),
        }),
    );
}

fn checks(transcript: &Value, run_id: &str, steps: &Value, vision: Option<bool>) -> Vec<Check> {
    let check = |id: &str, passed: bool, detail: String| Check {
        id: id.to_string(),
        passed,
        detail,
    };
    let stored = |function: &str| -> Vec<Value> {
        mechanism::entries(transcript)
            .filter_map(|entry| entry.get("message"))
            .filter(|message| {
                message["role"] == "function_result" && message["function_id"] == function
            })
            .cloned()
            .collect()
    };
    let big = stored(&big_function(run_id));
    let capped_with = |is_error: bool| {
        big.iter().any(|message| {
            mechanism::text_of(message).starts_with(BYTE_CAP_MARKER)
                && message["is_error"] == is_error
                && message["details"]["result_capped"]["original_bytes"]
                    .as_u64()
                    .is_some_and(|bytes| bytes > MAX_RESULT_BYTES)
                && message["details"]["result_capped"]["max_bytes"] == MAX_RESULT_BYTES
        })
    };
    let wide = stored(&wide_function(run_id));
    let wide_whole = wide.len() == 1 && mechanism::text_of(&wide[0]).len() == WIDE_BYTES;

    let steps: Vec<&Value> = steps.as_array().into_iter().flatten().collect();
    let views: Vec<&str> = steps
        .iter()
        .filter_map(|step| step["wide"].as_str())
        .collect();
    let view_capped = !views.is_empty()
        && views.iter().all(|view| {
            view.contains(TOKEN_CAP_MARKER)
                && view.starts_with("RB-H ")
                && view.ends_with(" RB-T")
                && !view.contains("RB-M")
        });
    let stable = views.len() >= 2 && views.windows(2).all(|pair| pair[0] == pair[1]);

    let images: Vec<&Value> = steps
        .iter()
        .map(|step| &step["png"])
        .filter(|png| !png.is_null())
        .collect();
    let is_image =
        |content: &Value| content[0]["type"] == "image" && content[0]["data"] == PNG_BASE64;
    let is_text = |content: &Value, text: &str| {
        content.as_array().is_some_and(|blocks| blocks.len() == 1) && content[0]["text"] == text
    };
    let media = match vision {
        Some(false) => {
            !images.is_empty()
                && images
                    .iter()
                    .all(|content| is_text(content, TEXT_ONLY_IMAGE))
        }
        Some(true) => {
            images.first().is_some_and(|content| is_image(content))
                && images
                    .iter()
                    .skip(1)
                    .all(|content| is_text(content, TOOL_IMAGE_AFTER_USE))
        }
        None => false,
    };
    vec![
        check(
            "big_ok_capped",
            capped_with(false),
            format!("{} stored result(s): {}", big.len(), summary(&big)),
        ),
        check(
            "big_error_capped",
            capped_with(true),
            format!("{} stored result(s): {}", big.len(), summary(&big)),
        ),
        check(
            "wide_stored_whole",
            wide_whole,
            format!(
                "{} stored, {:?} bytes",
                wide.len(),
                wide.first()
                    .map(|message| mechanism::text_of(message).len())
            ),
        ),
        check(
            "wide_view_capped",
            view_capped,
            format!(
                "{} step view(s), lengths {:?}, marker {:?}",
                views.len(),
                views.iter().map(|view| view.len()).collect::<Vec<_>>(),
                views.first().and_then(|view| view
                    .find(TOKEN_CAP_MARKER)
                    .map(|at| &view[at..(at + 60).min(view.len())]))
            ),
        ),
        check(
            "wide_view_stable",
            stable,
            format!("{} step view(s)", views.len()),
        ),
        check(
            "png_by_vision",
            media,
            format!("supports_vision={vision:?} views={images:?}"),
        ),
    ]
}

fn summary(results: &[Value]) -> String {
    results
        .iter()
        .map(|message| {
            let text = mechanism::text_of(message);
            format!(
                "is_error={} text={:?} result_capped={}",
                message["is_error"],
                &text[..text.len().min(48)],
                message["details"]["result_capped"]
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn evaluate_checks(
    observation: &ScenarioObservation,
    run_id: &str,
    checks: &[Check],
) -> ObjectiveEvaluation {
    if !observation.metrics.complete {
        return assessment::prerequisite_failure(
            ASSESSMENTS,
            "metrics_complete",
            "subject metrics are incomplete",
        );
    }
    let calls = common::function_calls(&observation.transcript);
    let calls_of = |function: String| {
        calls
            .iter()
            .filter(|call| call.function_id == function)
            .count()
    };
    // The bounds apply to the subject's calls: one that never made them says
    // nothing about the caps.
    if calls_of(wide_function(run_id)) == 0
        || calls_of(big_function(run_id)) < 2
        || calls_of(png_function(run_id)) == 0
    {
        return assessment::prerequisite_failure(
            ASSESSMENTS,
            "fixture_calls",
            "the subject did not make the four fixture calls",
        );
    }
    assessment::build_evaluation([
        mechanism::grade(
            BYTE_CAP,
            checks,
            &["big_ok_capped", "big_error_capped", "wide_stored_whole"],
        ),
        mechanism::grade(TOKEN_CAP, checks, &["wide_view_capped", "wide_view_stable"]),
        mechanism::grade(MEDIA, checks, &["png_by_vision"]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "control";

    fn result(function: String, text: &str, is_error: bool, details: Value) -> Value {
        json!({ "message": {
            "role": "function_result", "function_id": function, "is_error": is_error,
            "content": [{ "type": "text", "text": text }], "details": details,
        } })
    }

    fn capped(original: u64) -> Value {
        json!({ "result_capped": { "original_bytes": original, "max_bytes": MAX_RESULT_BYTES } })
    }

    fn view() -> String {
        format!("RB-H 0123{TOKEN_CAP_MARKER}30000 tokens; middle omitted]6789 RB-T")
    }

    fn healthy() -> (Value, Value) {
        let marker = format!("{BYTE_CAP_MARKER}600 KB, over the 256 KB harness result cap>");
        let transcript = json!({ "messages": [
            result(wide_function(RUN), &mechanism::marked_text("RB", WIDE_BYTES), false, json!(null)),
            result(big_function(RUN), &marker, false, capped(614_430)),
            result(big_function(RUN), &marker, true, capped(614_445)),
        ] });
        let text_only = json!([{ "type": "text", "text": TEXT_ONLY_IMAGE }]);
        let steps = json!([
            { "wide": view(), "png": null },
            { "wide": view(), "png": null },
            { "wide": view(), "png": text_only },
            { "wide": view(), "png": text_only },
        ]);
        (transcript, steps)
    }

    fn failing(transcript: &Value, steps: &Value, vision: Option<bool>) -> Vec<String> {
        checks(transcript, RUN, steps, vision)
            .into_iter()
            .filter(|check| !check.passed)
            .map(|check| check.id)
            .collect()
    }

    #[test]
    fn a_healthy_run_passes_every_check() {
        let (transcript, steps) = healthy();
        assert_eq!(
            failing(&transcript, &steps, Some(false)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_lost_is_error_or_an_uncapped_result_is_caught() {
        let (mut transcript, steps) = healthy();
        transcript["messages"][2]["message"]["is_error"] = json!(false);
        assert_eq!(
            failing(&transcript, &steps, Some(false)),
            ["big_error_capped"]
        );
        let (mut transcript, steps) = healthy();
        transcript["messages"][1] = result(big_function(RUN), &"B".repeat(64), false, json!(null));
        assert_eq!(failing(&transcript, &steps, Some(false)), ["big_ok_capped"]);
    }

    #[test]
    fn a_view_with_the_middle_or_one_that_moves_is_caught() {
        let (transcript, mut steps) = healthy();
        steps[1]["wide"] = json!(format!("RB-H RB-M {TOKEN_CAP_MARKER} RB-T"));
        assert_eq!(
            failing(&transcript, &steps, Some(false)),
            ["wide_view_capped", "wide_view_stable"]
        );
        let (transcript, mut steps) = healthy();
        steps[3]["wide"] = json!(format!("RB-H 01{TOKEN_CAP_MARKER} RB-T"));
        assert_eq!(
            failing(&transcript, &steps, Some(false)),
            ["wide_view_stable"]
        );
    }

    #[test]
    fn the_image_rule_follows_vision_support() {
        let (transcript, steps) = healthy();
        assert_eq!(failing(&transcript, &steps, Some(true)), ["png_by_vision"]);
        assert_eq!(failing(&transcript, &steps, None), ["png_by_vision"]);
        let (transcript, mut steps) = healthy();
        steps[2]["png"] = json!([{ "type": "image", "mime": "image/png", "data": PNG_BASE64 }]);
        steps[3]["png"] = json!([{ "type": "text", "text": TOOL_IMAGE_AFTER_USE }]);
        assert_eq!(
            failing(&transcript, &steps, Some(true)),
            Vec::<String>::new()
        );
        assert_eq!(failing(&transcript, &steps, Some(false)), ["png_by_vision"]);
    }

    #[test]
    fn criteria_weights_total_one_hundred() {
        ResultBounds.spec("run").validate().unwrap();
    }
}
