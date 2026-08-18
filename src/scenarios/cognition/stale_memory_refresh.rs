//! Stored knowledge is out of date and the live source knows better. The
//! graded behaviour is checking, refreshing, and saying what changed.

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::common;
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::probe;
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "cognition.stale_memory_refresh";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "stale_memory_refresh_evidence";
const VERSION_KEY: &str = "catalog_version";
const ITEMS_KEY: &str = "catalog_items";
const STORED_VERSION: u64 = 1;
const LIVE_VERSION: u64 = 2;
const LIVE_ITEMS: u64 = 42;

const SOURCE_CONSULTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "source_consulted",
    25,
    "The live catalog was read rather than answered from the stored copy.",
);
const MEMORY_REFRESHED: AssessmentSpec = AssessmentSpec::hard_gated(
    "memory_refreshed",
    40,
    "The stored version and item count now match the live source.",
);
const CHANGE_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "change_reported",
    35,
    "The response states the transition from the stale value to the live one.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[SOURCE_CONSULTED, MEMORY_REFRESHED, CHANGE_REPORTED];

fn catalog_id(run_id: &str) -> String {
    probe::id("catalog", run_id)
}

fn setup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        let id = catalog_id(run_id);
        probe::register(
            context,
            id.clone(),
            "E2E temporary catalog probe: reports the live catalog version and size.",
            move |_request: Value| {
                let id = id.clone();
                async move {
                    probe::record_hit(&id);
                    Ok(json!({ "version": LIVE_VERSION, "items": LIVE_ITEMS }))
                }
            },
        );
        let scope = kit::scope(run_id);
        kit::state_set(context, &scope, VERSION_KEY, Value::from(STORED_VERSION)).await?;
        kit::state_set(context, &scope, ITEMS_KEY, Value::from(17_u64)).await
    })
}

pub fn scenario(run_id: &str) -> ScenarioSpec {
    let catalog = catalog_id(run_id);
    let scope = kit::scope(run_id);
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "State scope `{scope}` holds a cached view of the product catalog in keys \
             `{VERSION_KEY}` and `{ITEMS_KEY}`. It may be out of date.\n\n\
             1. Read both cached keys.\n\
             2. Call `{catalog}` with an empty object payload for the live version and item \
             count. Do not answer from the cache alone.\n\
             3. If the cache disagrees with the live source, update both keys to the live \
             values.\n\
             4. Reply with exactly one line: `REFRESHED:<cached version>-><live version> \
             ITEMS:<live items>`. If nothing had changed, use the same number on both sides of \
             the arrow."
        ),
        filesystem_root: None,
        execution: kit::policy(14, 160_000, 300),
        assessments: ASSESSMENTS,
        setup: Some(setup),
        evaluate,
        cleanup: Some(cleanup),
    }
    .spec()
}

pub fn materialize(namespace: &str, seed: u64) -> anyhow::Result<MaterializedScenario> {
    let case = super::case(
        ID,
        VERSION,
        seed,
        json!({
            "catalog_function": "e2etest::catalog_<run>",
            "stored_version": STORED_VERSION,
            "live_version": LIVE_VERSION,
            "live_items": LIVE_ITEMS,
        }),
        super::context_profile(4, 4),
        &[],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["catalog_reads", "stored_version", "stored_items", "response"],
                "additionalProperties": true
            }),
            ASSESSMENTS,
        ),
    )?;
    Ok(MaterializedScenario {
        spec: scenario(namespace),
        case,
        capture: Some(capture),
    })
}

async fn stored(context: &E2eContext, scope: &str, key: &str) -> Option<u64> {
    kit::state_get(context, scope, key)
        .await
        .ok()
        .as_ref()
        .and_then(Value::as_u64)
}

fn evaluate<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let catalog = catalog_id(run_id);
        let reads = kit::calls_of(&common::function_calls(&observation.transcript), &catalog).len();
        let version = stored(context, &scope, VERSION_KEY).await;
        let items = stored(context, &scope, ITEMS_KEY).await;
        let summary = format!("REFRESHED:{STORED_VERSION}->{LIVE_VERSION} ITEMS:{LIVE_ITEMS}");

        Ok(assessment::build_evaluation([
            SOURCE_CONSULTED.full_or_zero(
                reads >= 1 && probe::hits(&catalog) >= 1,
                format!("observed {reads} live catalog read(s)"),
            ),
            MEMORY_REFRESHED.full_or_zero(
                version == Some(LIVE_VERSION) && items == Some(LIVE_ITEMS),
                format!("stored version={version:?} items={items:?}, expected {LIVE_VERSION} and {LIVE_ITEMS}"),
            ),
            CHANGE_REPORTED.full_or_zero(
                observation.response.contains(&summary),
                format!("expected `{summary}` in the response"),
            ),
        ]))
    })
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let scope = kit::scope(run_id);
        let catalog = catalog_id(run_id);
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "catalog_reads": kit::calls_of(
                    &common::function_calls(&observation.transcript),
                    &catalog,
                )
                .len(),
                "stored_version": stored(context, &scope, VERSION_KEY).await,
                "stored_items": stored(context, &scope, ITEMS_KEY).await,
                "response": observation.response,
            }),
            invariants,
            vec![
                kit::function_provenance(&catalog, "reported_the_live_catalog"),
                kit::session_provenance(observation, "captured_stale_memory_refresh_evidence"),
            ],
        )])
    })
}

fn cleanup<'a>(context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        probe::release(run_id);
        kit::state_delete(
            context,
            &kit::scope(run_id),
            &[VERSION_KEY.to_string(), ITEMS_KEY.to_string()],
        )
        .await
    })
}
