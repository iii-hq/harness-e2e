//! E8 (onboarding): a profile's preloaded functions and skills are frozen
//! into the prompt once and stay byte-identical on every later step.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{Broken, Prompt, Run};

pub const ID: &str = crate::scenarios::onboarding_tour::ID;
const BLOCKS: [&str; 2] = ["preloaded_functions", "preloaded_skills"];

fn block<'a>(prompt: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = prompt.find(&open)?;
    let end = prompt[start..].find(&close)? + start + close.len();
    Some(&prompt[start..end])
}

/// Per block: on how many steps it was missing and how many versions it had.
pub(super) fn capture(prompts: &[Prompt], probes: &mut BTreeMap<String, Value>) {
    for tag in BLOCKS {
        let seen: Vec<Option<&str>> = prompts
            .iter()
            .map(|p| block(&p.system_prompt, tag))
            .collect();
        let versions: BTreeSet<&str> = seen.iter().flatten().copied().collect();
        probes.insert(
            tag.into(),
            json!({"steps": prompts.len(), "missing": seen.iter().filter(|b| b.is_none()).count(),
                   "versions": versions.len()}),
        );
    }
}

pub(super) fn gates(run: &Run<'_>) -> Vec<Broken> {
    BLOCKS
        .iter()
        .filter_map(|tag| Some((tag, run.probe(tag)?)))
        // A profile that preloads nothing renders no block at all.
        .filter(|(_, seen)| {
            seen["versions"] != 0 && (seen["missing"] != 0 || seen["versions"] != 1)
        })
        .map(|(tag, seen)| {
            (
                "preloaded_blocks_frozen",
                format!(
                    "<{tag}> over {} step(s): missing on {}, {} version(s)",
                    seen["steps"], seen["missing"], seen["versions"]
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(system_prompt: &str) -> Prompt {
        Prompt {
            system_prompt: system_prompt.into(),
            ..Prompt::default()
        }
    }

    fn gates_of(prompts: &[Prompt]) -> Vec<String> {
        let mut probes = BTreeMap::new();
        capture(prompts, &mut probes);
        let transcript = json!({"messages": []});
        gates(&Run {
            transcript: &transcript,
            metrics: None,
            probes: &probes,
        })
        .into_iter()
        .map(|(gate, detail)| format!("{gate}: {detail}"))
        .collect()
    }

    const FROZEN: &str = "id\n<preloaded_functions>\nf\n</preloaded_functions>\n<preloaded_skills>s</preloaded_skills>";

    #[test]
    fn blocks_identical_on_every_step_keep_the_gate() {
        let later = format!("{FROZEN}\n\n# Conversation summary\n\nx");

        assert!(gates_of(&[prompt(FROZEN), prompt(&later)]).is_empty());
        assert!(gates_of(&[prompt("id"), prompt("id")]).is_empty());
    }

    #[test]
    fn a_block_that_changed_or_went_missing_is_flagged() {
        let changed = FROZEN.replace("\nf\n", "\ng\n");

        assert_eq!(
            gates_of(&[prompt(FROZEN), prompt(&changed), prompt("id")]),
            [
                "preloaded_blocks_frozen: <preloaded_functions> over 3 step(s): missing on 1, 2 version(s)",
                "preloaded_blocks_frozen: <preloaded_skills> over 3 step(s): missing on 1, 1 version(s)",
            ]
        );
    }
}
