//! World building with rules. The setting may be invented freely; the
//! entity and relation model it is expressed in may not.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::context::E2eContext;
use crate::scenarios::assessment::{self, AssessmentSpec};
use crate::scenarios::kit::{self, Blueprint};
use crate::scenarios::{
    CleanupFuture, DeliverableCaptureFuture, EvaluationFuture, MaterializedScenario,
    ScenarioObservation, ScenarioSpec,
};

use super::workspace;

pub const ID: &str = "deliverable.world_bible";
const VERSION: u32 = 1;
const DELIVERABLE_ID: &str = "world_bible_artifact";
const ENTITIES_FILE: &str = "world/entities.json";
const RELATIONS_FILE: &str = "world/relations.json";
const KINDS: [&str; 3] = ["region", "faction", "character"];
const PER_KIND: usize = 2;
const ENTITY_COUNT: usize = KINDS.len() * PER_KIND;

const REFERENTIAL_INTEGRITY: AssessmentSpec = AssessmentSpec::hard_gated(
    "referential_integrity",
    35,
    "Every relation endpoint refers to an entity that exists.",
);
const ENTITY_MODEL: AssessmentSpec = AssessmentSpec::hard_gated(
    "entity_model",
    25,
    "Six entities with unique ids, two of each kind, each fully described.",
);
const RELATION_RULES: AssessmentSpec = AssessmentSpec::hard_gated(
    "relation_rules",
    25,
    "Residence, rule, and alliance relations connect the kinds they are allowed to connect.",
);
const SUMMARY_REPORTED: AssessmentSpec = AssessmentSpec::hard_gated(
    "summary_reported",
    15,
    "The response reports the entity and relation counts that were written.",
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    REFERENTIAL_INTEGRITY,
    ENTITY_MODEL,
    RELATION_RULES,
    SUMMARY_REPORTED,
];

pub fn scenario(run_id: &str) -> ScenarioSpec {
    Blueprint {
        id: ID,
        version: VERSION,
        prompt: format!(
            "Invent a small setting and write it down as data in this workspace. The setting is \
             yours; the model is fixed.\n\n\
             1. Write `{ENTITIES_FILE}`: a JSON array of exactly {ENTITY_COUNT} objects, each \
             with `id`, `name`, and `kind`. Ids are unique lowercase snake_case. Use exactly \
             {PER_KIND} entities of each kind: `region`, `faction`, `character`.\n\
             2. Write `{RELATIONS_FILE}`: a JSON array of objects with `from`, `to`, and `type`. \
             Every `from` and `to` is an entity id from the first file. The allowed types and \
             their direction are:\n\
             - `resides_in`: character to region. Each character has exactly one.\n\
             - `rules`: faction to region. A faction rules at most one region, and no region is \
             ruled twice.\n\
             - `allied_with`: faction to faction. Include exactly one alliance, and never ally a \
             faction with itself.\n\
             3. Add no other relation types.\n\
             4. Reply with exactly one line: `ENTITIES:{ENTITY_COUNT} RELATIONS:<n>` where `<n>` \
             is how many relations you wrote."
        ),
        filesystem_root: Some(workspace::root(ID, run_id)),
        execution: kit::policy(20, 260_000, 900),
        assessments: ASSESSMENTS,
        setup: None,
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
            "entities_file": ENTITIES_FILE,
            "relations_file": RELATIONS_FILE,
            "kinds": KINDS,
            "per_kind": PER_KIND,
        }),
        super::build_profile(2, 6),
        &["iii::shell"],
        super::contract(
            DELIVERABLE_ID,
            json!({
                "type": "object",
                "required": ["entities", "relations", "response"],
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

fn array(run_id: &str, relative: &str) -> Vec<Value> {
    workspace::read_json(&workspace::root(ID, run_id), relative)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

fn entity_kinds(entities: &[Value]) -> HashMap<String, String> {
    entities
        .iter()
        .filter_map(|entity| {
            let id = entity.get("id").and_then(Value::as_str)?;
            let kind = entity.get("kind").and_then(Value::as_str)?;
            Some((id.to_string(), kind.to_string()))
        })
        .collect()
}

fn entity_model_holds(entities: &[Value]) -> bool {
    let kinds = entity_kinds(entities);
    let described = entities.iter().all(|entity| {
        entity
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty())
    });
    let unique_ids: HashSet<&String> = kinds.keys().collect();
    let per_kind_correct = KINDS.iter().all(|kind| {
        kinds
            .values()
            .filter(|value| value.as_str() == *kind)
            .count()
            == PER_KIND
    });
    entities.len() == ENTITY_COUNT
        && kinds.len() == ENTITY_COUNT
        && unique_ids.len() == ENTITY_COUNT
        && described
        && per_kind_correct
}

fn referential_integrity_holds(entities: &[Value], relations: &[Value]) -> bool {
    let kinds = entity_kinds(entities);
    !relations.is_empty()
        && relations.iter().all(|relation| {
            let from = relation.get("from").and_then(Value::as_str);
            let to = relation.get("to").and_then(Value::as_str);
            from.is_some_and(|id| kinds.contains_key(id))
                && to.is_some_and(|id| kinds.contains_key(id))
        })
}

fn relation_rules_hold(entities: &[Value], relations: &[Value]) -> bool {
    let kinds = entity_kinds(entities);
    let kind_of = |id: Option<&str>| id.and_then(|id| kinds.get(id)).map(String::as_str);
    let mut residences: HashMap<&str, usize> = HashMap::new();
    let mut ruled_regions: HashSet<&str> = HashSet::new();
    let mut ruling_factions: HashSet<&str> = HashSet::new();
    let mut alliances = 0;

    for relation in relations {
        let from = relation.get("from").and_then(Value::as_str);
        let to = relation.get("to").and_then(Value::as_str);
        let (Some(from_id), Some(to_id)) = (from, to) else {
            return false;
        };
        match relation.get("type").and_then(Value::as_str) {
            Some("resides_in") => {
                if kind_of(from) != Some("character") || kind_of(to) != Some("region") {
                    return false;
                }
                *residences.entry(from_id).or_default() += 1;
            }
            Some("rules") => {
                if kind_of(from) != Some("faction") || kind_of(to) != Some("region") {
                    return false;
                }
                if !ruled_regions.insert(to_id) || !ruling_factions.insert(from_id) {
                    return false;
                }
            }
            Some("allied_with") => {
                if kind_of(from) != Some("faction")
                    || kind_of(to) != Some("faction")
                    || from_id == to_id
                {
                    return false;
                }
                alliances += 1;
            }
            _ => return false,
        }
    }

    let characters: Vec<&String> = kinds
        .iter()
        .filter(|(_, kind)| kind.as_str() == "character")
        .map(|(id, _)| id)
        .collect();
    alliances == 1
        && characters
            .iter()
            .all(|character| residences.get(character.as_str()) == Some(&1))
}

fn evaluate<'a>(
    _context: &'a E2eContext,
    observation: &'a ScenarioObservation,
    run_id: &'a str,
) -> EvaluationFuture<'a> {
    Box::pin(async move {
        let entities = array(run_id, ENTITIES_FILE);
        let relations = array(run_id, RELATIONS_FILE);
        let summary = format!("ENTITIES:{ENTITY_COUNT} RELATIONS:{}", relations.len());

        Ok(assessment::build_evaluation([
            REFERENTIAL_INTEGRITY.full_or_zero(
                referential_integrity_holds(&entities, &relations),
                format!(
                    "observed {} entity(ies) and {} relation(s)",
                    entities.len(),
                    relations.len()
                ),
            ),
            ENTITY_MODEL.full_or_zero(
                entity_model_holds(&entities),
                format!("observed {} entity(ies)", entities.len()),
            ),
            RELATION_RULES.full_or_zero(
                relation_rules_hold(&entities, &relations),
                format!("observed {} relation(s)", relations.len()),
            ),
            SUMMARY_REPORTED.full_or_zero(
                !relations.is_empty() && observation.response.contains(&summary),
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
        let invariants =
            kit::captured_gate_invariants(evaluate(context, observation, run_id).await?);
        Ok(vec![kit::evidence(
            DELIVERABLE_ID,
            super::DELIVERABLE_KIND,
            json!({
                "entities": array(run_id, ENTITIES_FILE),
                "relations": array(run_id, RELATIONS_FILE),
                "response": observation.response,
            }),
            invariants,
            vec![kit::session_provenance(
                observation,
                "captured_world_bible_before_cleanup",
            )],
        )])
    })
}

fn cleanup<'a>(_context: &'a E2eContext, run_id: &'a str) -> CleanupFuture<'a> {
    Box::pin(async move {
        workspace::remove(&workspace::root(ID, run_id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entities() -> Vec<Value> {
        vec![
            json!({ "id": "north_reach", "name": "North Reach", "kind": "region" }),
            json!({ "id": "salt_flats", "name": "Salt Flats", "kind": "region" }),
            json!({ "id": "iron_pact", "name": "Iron Pact", "kind": "faction" }),
            json!({ "id": "dust_choir", "name": "Dust Choir", "kind": "faction" }),
            json!({ "id": "mera", "name": "Mera", "kind": "character" }),
            json!({ "id": "oda", "name": "Oda", "kind": "character" }),
        ]
    }

    fn relations() -> Vec<Value> {
        vec![
            json!({ "from": "mera", "to": "north_reach", "type": "resides_in" }),
            json!({ "from": "oda", "to": "salt_flats", "type": "resides_in" }),
            json!({ "from": "iron_pact", "to": "north_reach", "type": "rules" }),
            json!({ "from": "dust_choir", "to": "salt_flats", "type": "rules" }),
            json!({ "from": "iron_pact", "to": "dust_choir", "type": "allied_with" }),
        ]
    }

    #[test]
    fn a_consistent_world_satisfies_every_rule() {
        assert!(entity_model_holds(&entities()));
        assert!(referential_integrity_holds(&entities(), &relations()));
        assert!(relation_rules_hold(&entities(), &relations()));
    }

    #[test]
    fn a_dangling_reference_fails_integrity() {
        let mut broken = relations();
        broken.push(json!({ "from": "mera", "to": "ghost_city", "type": "resides_in" }));
        assert!(!referential_integrity_holds(&entities(), &broken));
    }

    #[test]
    fn a_region_may_not_be_ruled_twice() {
        let mut broken = relations();
        broken.push(json!({ "from": "dust_choir", "to": "north_reach", "type": "rules" }));
        assert!(!relation_rules_hold(&entities(), &broken));
    }

    #[test]
    fn a_character_must_reside_somewhere() {
        let sparse: Vec<Value> = relations()
            .into_iter()
            .filter(|relation| relation["from"] != "oda")
            .collect();
        assert!(!relation_rules_hold(&entities(), &sparse));
    }
}
