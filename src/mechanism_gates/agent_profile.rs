//! E8 (null-canary, onboarding): a run under an agent profile really ran as
//! it. The session records the profile it was sent with, and the prompt
//! carries the profile's own body.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{ask, Attempt, Broken, Prompt, Run};
use crate::context::E2eContext;

/// The profile the null-canary suite runs the regression as.
const NULL_AGENT: &str = "null-agent";

/// The profile whose gates this run is held to: the null canary's, and the
/// one onboarding pins. Any other profiled run (a software-engineer lane)
/// is out of these gates' reach.
pub(super) fn gated<'a>(scenario: &str, agent: Option<&'a str>) -> Option<&'a str> {
    agent.filter(|agent| *agent == NULL_AGENT || scenario == super::onboarding::ID)
}

pub(super) async fn capture(
    context: &E2eContext,
    attempt: &Attempt<'_>,
    agent: &str,
    prompts: &[Prompt],
    probes: &mut BTreeMap<String, Value>,
) {
    let session = ask(
        context,
        "session::get",
        json!({"session_id": attempt.session_id}),
    )
    .await;
    // The suite's own resolution keeps only the profile's identity, not its body.
    let profile = ask(context, "directory::agents::get", json!({"id": agent})).await;
    let body = profile["system_prompt"].as_str().unwrap_or_default().trim();
    probes.insert(
        "agent_profile".into(),
        json!({
            "expected": agent,
            "recorded": session.pointer("/meta/metadata/agent_profile/id"),
            "body_in_prompt": prompts.first().map(|prompt| !body.is_empty() && prompt.system_prompt.contains(body)),
        }),
    );
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    let Some(profile) = run.probe("agent_profile") else {
        return Vec::new();
    };
    let mut broken = Vec::new();
    if profile["recorded"] != profile["expected"] {
        broken.push((
            "agent_profile_recorded",
            format!(
                "the session records profile {}, it was sent {}",
                profile["recorded"], profile["expected"]
            ),
        ));
    }
    // Not judged when the fail-open observer saw no prompt.
    if profile["body_in_prompt"] == false {
        broken.push((
            "agent_profile_body",
            format!(
                "the first prompt does not carry profile {}'s body",
                profile["expected"]
            ),
        ));
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gates_of(profile: Value) -> Vec<String> {
        let probes = BTreeMap::from([("agent_profile".to_string(), profile)]);
        let transcript = json!({"messages": []});
        gates(&Run {
            transcript: &transcript,
            metrics: None,
            probes: &probes,
        })
        .into_iter()
        .map(|(gate, _)| gate.to_string())
        .collect()
    }

    #[test]
    fn a_session_that_ran_as_its_profile_keeps_both_gates() {
        let profile =
            json!({"expected": "null-agent", "recorded": "null-agent", "body_in_prompt": true});

        assert!(gates_of(profile).is_empty());
    }

    #[test]
    fn only_the_null_canary_and_onboarding_profiles_are_gated() {
        assert_eq!(
            gated("persistent_state", Some("null-agent")),
            Some("null-agent")
        );
        assert_eq!(gated("persistent_state", Some("software-engineer")), None);
        assert_eq!(
            gated(super::super::onboarding::ID, Some("ade-worker-builder")),
            Some("ade-worker-builder")
        );
        assert_eq!(gated("persistent_state", None), None);
    }

    #[test]
    fn a_prompt_the_observer_never_saw_is_not_judged() {
        let profile =
            json!({"expected": "null-agent", "recorded": "null-agent", "body_in_prompt": null});

        assert!(gates_of(profile).is_empty());
    }

    #[test]
    fn a_session_without_its_profile_or_its_body_is_flagged() {
        let profile = json!({"expected": "null-agent", "recorded": null, "body_in_prompt": false});

        assert_eq!(
            gates_of(profile),
            ["agent_profile_recorded", "agent_profile_body"]
        );
    }
}
