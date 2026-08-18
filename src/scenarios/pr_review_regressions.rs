//! Real-world pull-request review cases derived from iii-hq/workers#808.
//!
//! Each case isolates one regression behind the production code review. The
//! subject receives enough surrounding contract to reason about the failure,
//! but not the expected finding. A fixed judge grades root cause, impact, and
//! remediation independently so one missed issue does not hide the others.

use serde_json::{json, Value};

use crate::context::E2eContext;

use super::common;
use super::{
    ArtifactExpectation, CapturedDeliverable, ComplexityProfile, DeliverableCaptureFuture,
    DeliverableContract, ExecutionPolicy, InvariantSpec, MaterializedScenario, ProvenanceEvidence,
    ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const TOKEN_TAKEOVER_ID: &str = "pr_review.token_takeover";
pub const RECONNECT_SWEEP_ID: &str = "pr_review.reconnect_sweep";
pub const ASSET_RETRY_ACK_ID: &str = "pr_review.asset_retry_ack";
pub const PRESENCE_RECONNECT_ID: &str = "pr_review.presence_reconnect";
pub const PROMPT_PROVENANCE_ID: &str = "pr_review.prompt_provenance";

const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "review_finding";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewCase {
    TokenTakeover,
    ReconnectSweep,
    AssetRetryAck,
    PresenceReconnect,
    PromptProvenance,
}

impl ReviewCase {
    pub const ALL: [Self; 5] = [
        Self::TokenTakeover,
        Self::ReconnectSweep,
        Self::AssetRetryAck,
        Self::PresenceReconnect,
        Self::PromptProvenance,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::TokenTakeover => TOKEN_TAKEOVER_ID,
            Self::ReconnectSweep => RECONNECT_SWEEP_ID,
            Self::AssetRetryAck => ASSET_RETRY_ACK_ID,
            Self::PresenceReconnect => PRESENCE_RECONNECT_ID,
            Self::PromptProvenance => PROMPT_PROVENANCE_ID,
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::TokenTakeover => TOKEN_TAKEOVER_PROMPT,
            Self::ReconnectSweep => RECONNECT_SWEEP_PROMPT,
            Self::AssetRetryAck => ASSET_RETRY_ACK_PROMPT,
            Self::PresenceReconnect => PRESENCE_RECONNECT_PROMPT,
            Self::PromptProvenance => PROMPT_PROVENANCE_PROMPT,
        }
    }

    fn reference(self) -> Value {
        match self {
            Self::TokenTakeover => json!({
                "expected_finding": "The new unregister function removes a token-bound provider using only its public id. Internal metadata is discovery filtering, not authorization, so any connected caller can erase the binding and claim the id with a newly minted token.",
                "impact": "Provider hijack or denial of service; the existing takeover protection is bypassed.",
                "acceptable_remediation": [
                    "Put reset behind an operator-authenticated or capability-protected boundary.",
                    "Require authorization stronger than knowledge of the provider id."
                ],
                "severity": "blocking or high priority"
            }),
            Self::ReconnectSweep => json!({
                "expected_finding": "The registry availability flag is not connection state. An idle disconnect/reconnect leaves it true, so stale_provider_ids skips the provider exactly when the function-set diff would have detected the reappearing ready handler.",
                "impact": "Provider declaration and catalog application state are not replayed until a later dispatch failure or periodic refresh, recreating the multi-minute stale-provider window.",
                "acceptable_remediation": [
                    "Track function disappearance and reappearance independently of cached availability.",
                    "Use the lifecycle event identity or a live-set transition to select the provider to nudge."
                ],
                "severity": "blocking or high priority"
            }),
            Self::AssetRetryAck => json!({
                "expected_finding": "The retry tests a Result that cannot represent remote registration failure. iii-sdk 0.21.6 ignores send_message errors and always returns Ok after caching the trigger; asynchronous rejection is only logged later.",
                "impact": "Both the initial registration and first retry report success, the retry exits, and the generic fallback UI can remain sticky.",
                "acceptable_remediation": [
                    "Drive retry from an acknowledged registration result or read-back.",
                    "Use another observable confirmation instead of the fire-and-forget return value."
                ],
                "severity": "blocking or high priority"
            }),
            Self::PresenceReconnect => json!({
                "expected_finding": "The presence hook reads workers only at mount and then depends on lossy lifecycle events. If llm-router is added while the browser socket is disconnected, the event is missed and present stays false; the reconnect refresh is itself gated on routerAvailable, producing a recovery deadlock.",
                "impact": "The model picker remains disabled or empty until a full page reload even though llm-router is healthy.",
                "acceptable_remediation": [
                    "Re-run the authoritative workers-list probe whenever the WebSocket reconnects.",
                    "Install reconnect recovery independently of the current routerAvailable value."
                ],
                "severity": "blocking or high priority"
            }),
            Self::PromptProvenance => json!({
                "expected_finding": "Byte equality with the embedded fallback does not prove fallback provenance. A caller can explicitly override the prompt with identical text and later omit prompt fields to inherit it; the new branch then replaces that explicit value with router identity.",
                "impact": "A subsequent steer silently violates explicit prompt inheritance and changes session behavior.",
                "acceptable_remediation": [
                    "Persist whether the prior prompt came from fallback resolution.",
                    "Preserve explicit provenance instead of inferring it from resolved text."
                ],
                "severity": "medium priority"
            }),
        }
    }
}

pub fn scenario(case: ReviewCase, _run_id: &str) -> ScenarioSpec {
    ScenarioSpec {
        id: case.id(),
        version: VERSION,
        prompt: case.prompt().to_string(),
        filesystem_root: None,
        execution: ExecutionPolicy {
            max_turns: 2,
            max_output_tokens: Some(4_096),
            max_total_tokens: 32_768,
            stuck_timeout_seconds: 180,
            max_validation_retries: None,
        },
        denied_functions: &[],
        criteria: vec![
            super::CriterionSpec::advisory_judge(
                "root_cause",
                50,
                "Full credit: identifies the exact state, authorization, acknowledgement, lifecycle, or provenance mechanism in the reference and connects it to the changed code. Partial: notices the symptom but gives an incomplete or generic mechanism. Zero: misses the regression or claims the code is safe.",
            ),
            super::CriterionSpec::advisory_judge(
                "impact",
                30,
                "Full credit: explains the concrete user or system failure and the triggering sequence. Partial: impact is plausible but vague or misses the trigger. Zero: no meaningful impact or an unrelated one.",
            ),
            super::CriterionSpec::advisory_judge(
                "remediation",
                20,
                "Full credit: proposes a fix that restores the reference invariant without relying on timing or retries that cannot observe success. Partial: directionally useful but underspecified. Zero: no fix or a fix that preserves the bug.",
            ),
        ],
        judge_reference: Some(case.reference()),
        setup: None,
        evaluate: common::evaluate_text_response,
        cleanup: None,
    }
}

pub fn materialize(
    case: ReviewCase,
    namespace: &str,
    seed: u64,
) -> anyhow::Result<MaterializedScenario> {
    let spec = scenario(case, namespace);
    let scenario_case = ScenarioCase::new(
        case.id(),
        VERSION,
        seed,
        json!({
            "source": "iii-hq/workers#808",
            "review_case": case.id(),
            "review_dimensions": ["root_cause", "impact", "remediation"],
        }),
        ComplexityProfile {
            planning_depth: 2,
            dependency_depth: 1,
            artifact_count: 1,
            ambiguity_level: 2,
            ..ComplexityProfile::default()
        },
        vec!["e2e::control-plane-v1".to_string()],
        deliverable_contract(),
    )?;
    Ok(MaterializedScenario {
        spec,
        case: scenario_case,
        capture: Some(capture),
    })
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: DELIVERABLE_ID.to_string(),
            kind: "code_review".to_string(),
            media_type: "application/json".to_string(),
            schema: json!({
                "type": "object",
                "required": ["scenario_id", "review"],
                "properties": {
                    "scenario_id": { "type": "string" },
                    "review": { "type": "string", "minLength": 1 }
                },
                "additionalProperties": false
            }),
            max_size_bytes: 65_536,
        }],
        invariants: vec![
            InvariantSpec {
                id: "response_present".to_string(),
                description: "The review contains a textual finding.".to_string(),
            },
            InvariantSpec {
                id: "no_function_calls".to_string(),
                description: "The review is completed without external actions.".to_string(),
            },
            InvariantSpec {
                id: "single_turn".to_string(),
                description: "The review completes in one subject turn.".to_string(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

fn capture<'a>(
    context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> DeliverableCaptureFuture<'a> {
    Box::pin(async move {
        let objective = common::evaluate_text_response(context, observation, run_id).await?;
        Ok(vec![CapturedDeliverable {
            id: DELIVERABLE_ID.to_string(),
            kind: "code_review".to_string(),
            content: json!({
                "scenario_id": observation.case.scenario_id.as_str(),
                "review": observation.response.as_str(),
            })
            .into(),
            invariants: super::captured_gate_invariants(objective),
            provenance: vec![ProvenanceEvidence {
                kind: "session".to_string(),
                source_id: observation.metrics.root_session_id.clone(),
                relation: "produced_review_finding".to_string(),
            }],
        }])
    })
}

#[cfg(test)]
const REVIEW_INSTRUCTIONS: &str = r#"Review the proposed change below as if it were a pull request. Return exactly one concise, actionable review finding. Include severity, the precise failing sequence or invariant, concrete impact, and a viable remediation. Do not discuss style, summarize the patch, or use tools."#;

const TOKEN_TAKEOVER_PROMPT: &str = r#"Review the proposed change below as if it were a pull request. Return exactly one concise, actionable review finding. Include severity, the precise failing sequence or invariant, concrete impact, and a viable remediation. Do not discuss style, summarize the patch, or use tools.

Existing contract:
- RegistryStore::upsert binds a provider id to a hashed registration token.
- Re-register, resolve, reconcile, and credential writes must present the token.
- Function metadata.internal only omits the function from default discovery; a connected worker that knows the function id may still invoke it.

Proposed code:
```rust
iii.register_function(
    "router::provider::unregister",
    RegisterFunction::new_async_with_bad_request(
        make_provider_unregister(registry.clone(), catalog.clone()),
        invalid_request_from_serde,
    )
    .metadata(json!({ "internal": true })),
);

async fn unregister(input: ProviderUnregisterRequest) -> Result<Response, Error> {
    let removed = registry.remove(&input.id).await?;
    if removed {
        catalog.remove_slice(&input.id).await?;
    }
    Ok(Response { ok: true, removed })
}
```

The endpoint is intended as an operator escape hatch when the original provider lost its raw token."#;

const RECONNECT_SWEEP_PROMPT: &str = r#"Review the proposed change below as if it were a pull request. Return exactly one concise, actionable review finding. Include severity, the precise failing sequence or invariant, concrete impact, and a viable remediation. Do not discuss style, summarize the patch, or use tools.

Relevant lifecycle:
- A provider SDK reconnect replays registered functions, but not its application-owned declaration or model-catalog push.
- RegistryRecord.available is set false at router boot or after a chat dispatch observes function_not_found. Engine function lifecycle events do not update it.
- engine::functions-available wakes this sweep after function-registration bursts.

Old selection:
```rust
let added = live.iter().filter(|id| !known.contains(*id)).cloned().collect();
*known = live.into_iter().collect();
nudge_provider_ids(&iii, &added).await;
```

Proposed selection:
```rust
async fn stale_provider_ids(registry: &RegistryStore, live: Vec<String>) -> Vec<String> {
    let mut stale = Vec::new();
    for id in live {
        let up = registry.get(&id).await.map(|r| r.available).unwrap_or(false);
        if !up {
            stale.push(id);
        }
    }
    stale
}

let live = live_provider_ids(&iii).await;
let stale = stale_provider_ids(&registry, live).await;
nudge_provider_ids(&iii, &stale).await;
```

The change aims to avoid nudging healthy providers during unrelated function-registration churn."#;

const ASSET_RETRY_ACK_PROMPT: &str = r#"Review the proposed change below as if it were a pull request. Return exactly one concise, actionable review finding. Include severity, the precise failing sequence or invariant, concrete impact, and a viable remediation. Do not discuss style, summarize the patch, or use tools.

Pinned SDK behavior (iii-sdk 0.21.6):
```rust
pub fn register_trigger(&self, input: RegisterTriggerInput) -> Result<Trigger, Error> {
    let message = RegisterTriggerMessage::from(input);
    self.inner.triggers.lock_or_recover().insert(message.id.clone(), message.clone());
    let _ = self.send_message(message.to_message());
    Ok(Trigger::new(/* explicit unregister closure */))
}

// A later TriggerRegistrationResult { error: Some(err), .. } is logged by
// handle_message; it is not returned to the caller above.
```

Proposed recovery:
```rust
fn spawn_registration_retry(iii: Arc<IIIClient>, path: String) {
    tokio::spawn(async move {
        let mut delay = Duration::from_secs(2);
        loop {
            tokio::time::sleep(delay).await;
            match register_asset_trigger(&iii, &path) {
                Ok(_handle) => {
                    tracing::info!(path, "registered console ui asset (after retry)");
                    return;
                }
                Err(error) => {
                    tracing::debug!(%error, path, "retry failed");
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    });
}
```

The change is intended to recover when the console rejects console:script registration during startup."#;

const PRESENCE_RECONNECT_PROMPT: &str = r#"Review the proposed change below as if it were a pull request. Return exactly one concise, actionable review finding. Include severity, the precise failing sequence or invariant, concrete impact, and a viable remediation. Do not discuss style, summarize the patch, or use tools.

```ts
function useWorkerPresence(name: string, enabled: boolean) {
  const [present, setPresent] = useState(!enabled)

  useWorkerLifecycle({
    enabled,
    operations: ['add', 'remove'],
    onEvent: (event) => {
      if (event.worker === name && event.stage === 'done') {
        if (event.operation === 'add') setPresent(true)
        if (event.operation === 'remove') setPresent(false)
      }
    },
  })

  useEffect(() => {
    if (!enabled) return
    void getIiiClient()
      .then((client) => client.trigger('engine::workers::list', {}))
      .then((result) => setPresent(result.workers.some((w) => w.name === name)))
      .catch(() => setPresent(false))
  }, [enabled, name])

  return present
}

const routerAvailable = useWorkerPresence('llm-router', backend === 'real')

useEffect(() => {
  if (backend !== 'real' || !routerAvailable) return
  return addConnectionStateListener((state) => {
    if (state === 'connected') refreshModelsAndProviders()
  })
}, [backend, routerAvailable])
```

Lifecycle events are not replayed. Consider a tab whose initial list says llm-router is absent, then its WebSocket disconnects, llm-router is added, and the socket reconnects."#;

const PROMPT_PROVENANCE_PROMPT: &str = r#"Review the proposed change below as if it were a pull request. Return exactly one concise, actionable review finding. Include severity, the precise failing sequence or invariant, concrete impact, and a viable remediation. Do not discuss style, summarize the patch, or use tools.

Contract:
- A caller may choose system_prompt_strategy = override and provide any string.
- On later sends, omitting both prompt fields inherits the prior resolved prompt verbatim.
- When no identity or caller prompt exists, build_system_prompt(mode, None) returns the embedded fallback.

Proposed recovery after inheriting prior options:
```rust
inherit_prior_system_prompt(&mut options, &previous.options);

if cfg.provider_identity_prompt
    && options.system_prompt.as_deref()
        == Some(build_system_prompt(PromptOpts {
            mode: previous.options.mode,
            identity: None,
        }).as_str())
{
    if let Some(identity) = router.system_prompt_get(options.provider.as_deref()).await {
        options.system_prompt = Some(build_system_prompt(PromptOpts {
            mode: previous.options.mode,
            identity: Some(&identity),
        }));
    }
}
```

The branch is intended to upgrade sessions that started while llm-router was unavailable and therefore froze the embedded fallback."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pr_review_case_is_valid_and_materializes_stably() {
        for case in ReviewCase::ALL {
            scenario(case, "review").validate().unwrap();
            let first = materialize(case, "attempt-a", 808).unwrap();
            let retry = materialize(case, "attempt-b", 808).unwrap();
            assert_eq!(first.case.case_id, retry.case.case_id, "{case:?}");
            assert_eq!(first.case.inputs, retry.case.inputs, "{case:?}");
            assert_eq!(
                first.case.inputs_sha256, retry.case.inputs_sha256,
                "{case:?}"
            );
            assert_eq!(first.case.deliverable_contract.artifacts.len(), 1);
            assert!(first.capture.is_some());
        }
    }

    #[test]
    fn shared_review_instruction_stays_in_sync() {
        for case in ReviewCase::ALL {
            assert!(case.prompt().starts_with(REVIEW_INSTRUCTIONS), "{case:?}");
        }
    }
}
