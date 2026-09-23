//! Build a run-scoped chess Worker and validate it through its public iii functions.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use iii_sdk::protocol::TriggerRequest;
use iii_sdk::IIIClient;
use serde_json::{json, Value};
use shakmaty::fen::Fen;
use shakmaty::{CastlingMode, Chess, EnPassantMode};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{oneshot, Mutex};

use crate::context::E2eContext;
use crate::report::{CompletionState, EvaluationDimension};

use super::assessment::{self, AssessmentSpec};
use super::chess_engine;
use super::{
    async_trait, ArtifactExpectation, Capability, CapturedDeliverable, CapturedDeliverableContent,
    CapturedInvariant, DeliverableContract, ExecutionPolicy, InvariantSpec, ObjectiveEvaluation,
    ProvenanceEvidence, Scenario, ScenarioCase, ScenarioObservation, ScenarioSpec,
};

pub const ID: &str = "chess_engine_build";
pub const SUMMARY: &str = "Build a run-scoped iii Worker that implements correct chess rules and serves a playable board. The runner independently checks its Compose lifecycle and function contracts, compares chess behavior with the shared shakmaty oracle, drives e2-e4 and e7-e5 in a real browser, and captures before/after screenshots bound to the candidate source and verified FENs.";

const FIXTURE_REVISION: &str = "16f6b9e05e34e09c824191eed0631d77f85be6a9";
const CHESS_SUBTREE: &str = "chess";
const CHESS_MANIFEST_SHA256: &str =
    "sha256:b2166cc0001a75a2afa0fdc1275d9252ac0e45bec6d4e59e6d04b2d53bd5f9f7";
const EVIDENCE_ID: &str = "chess_worker_evidence";
const EVIDENCE_LIMIT: u64 = 12 * 1024 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 4 * 1024 * 1024;
const START_FEN: &str = chess_engine::STARTPOS;
const KIWIPETE_FEN: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
const EN_PASSANT_FEN: &str = "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3";
const PROMOTION_FEN: &str = "8/P7/8/8/8/8/8/k1K5 w - - 0 1";
const CHECK_EVASION_FEN: &str = "4k3/8/8/8/8/8/4R3/4K3 b - - 0 1";
const PINNED_FEN: &str = "4r1k1/8/8/8/8/8/4R3/4K3 w - - 0 1";

const RUNTIME_CONTRACT: AssessmentSpec = AssessmentSpec::scored_in(
    "runtime_contract",
    15,
    "worker-compose.yaml declares the run-scoped Worker, Compose reports its container ready, and all four described iii functions are registered.",
    EvaluationDimension::Deliverable,
);
const START_RULES: AssessmentSpec = AssessmentSpec::scored(
    "rules_start",
    7,
    "Starting-position legal moves and perft match the independent oracle.",
);
const CASTLING_RULES: AssessmentSpec = AssessmentSpec::scored(
    "rules_castling",
    7,
    "The castling position legal moves and perft match the independent oracle.",
);
const EN_PASSANT_RULES: AssessmentSpec = AssessmentSpec::scored(
    "rules_en_passant",
    7,
    "The en-passant position legal moves and perft match the independent oracle.",
);
const PROMOTION_RULES: AssessmentSpec = AssessmentSpec::scored(
    "rules_promotion",
    7,
    "The promotion position legal moves and perft match the independent oracle.",
);
const CHECK_RULES: AssessmentSpec = AssessmentSpec::scored(
    "rules_check_evasion",
    7,
    "The check-evasion position legal moves and perft match the independent oracle.",
);
const PLAY_RULES: AssessmentSpec = AssessmentSpec::scored(
    "play_contract",
    10,
    "The Worker applies a legal move to the exact oracle FEN and rejects an illegal move.",
);
const INVALID_INPUTS: AssessmentSpec = AssessmentSpec::scored(
    "invalid_inputs",
    10,
    "Malformed FEN, invalid depth, and malformed move requests are rejected.",
);
const PLAYABLE_UI: AssessmentSpec = AssessmentSpec::scored_in(
    "playable_ui",
    20,
    "Worker-authored HTML renders a playable 64-square board; a real browser plays e2-e4 and e7-e5 through the Worker and reaches both oracle FENs.",
    EvaluationDimension::Deliverable,
);
const EVIDENCE_COMPLETE: AssessmentSpec = AssessmentSpec::scored_in(
    "evidence_complete",
    10,
    "Portable before/after screenshots and capture metadata are bound to candidate, Compose, and checked-FEN hashes.",
    EvaluationDimension::StructuralIntegrity,
);
const ASSESSMENTS: &[AssessmentSpec] = &[
    RUNTIME_CONTRACT,
    START_RULES,
    CASTLING_RULES,
    EN_PASSANT_RULES,
    PROMOTION_RULES,
    CHECK_RULES,
    PLAY_RULES,
    INVALID_INPUTS,
    PLAYABLE_UI,
    EVIDENCE_COMPLETE,
];

pub struct ChessEngineBuild;

#[async_trait]
impl Scenario for ChessEngineBuild {
    fn id(&self) -> &'static str {
        ID
    }

    fn summary(&self) -> Option<&'static str> {
        Some(SUMMARY)
    }

    fn case(&self, seed: u64) -> Result<ScenarioCase> {
        ScenarioCase::new(
            ID,
            seed,
            json!({
                "fixture_revision": FIXTURE_REVISION,
                "fixture_manifest_sha256": CHESS_MANIFEST_SHA256,
                "worker_functions": ["legal_moves", "perft", "play", "view"],
                "browser_moves": ["e2e4", "e7e5"],
                "oracle": "shakmaty",
            }),
            vec![
                Capability::E2eControlPlaneV1,
                Capability::IiiFunctions,
                Capability::IiiCoder,
                Capability::IiiShell,
                Capability::BrowserInteractive,
                Capability::IiiCompose,
                Capability::IiiWorkers,
                Capability::Node,
            ],
            deliverable_contract(),
        )
    }

    fn spec(&self, run_id: &str) -> ScenarioSpec {
        let contract = worker_contract(run_id);
        let root = workspace_root(run_id);
        ScenarioSpec {
            id: ID,
            prompt: format!(
                r#"Build and start a playable chess Worker inside `{root}`. Read its `README.md`
first for the pinned SDK bootstrap and registration shape.

The frozen chess fixture in that directory is reference material. Replace its CLI-only delivery
with a run-scoped iii Worker declared in `{compose}`. The Compose worker and container name must be
`{worker}`, its `worker` URI must be exactly `path://.`, and `scripts.run` must be exactly
`npm start`. Declare no sibling containers, `start_after`, or external working directory.
Do not declare `engine:`; the run-scoped project uses the existing iii Engine.
Keep all chess logic and application HTML in this Worker. Chess libraries and external network
services are forbidden. The runner supplies the relative `/api/play` transport after completion.
The Harness has already materialized `iii-sdk@0.23.1-rc.6` and its lockfile in this workspace solely
for iii integration. Do not change dependencies. Set `scripts.run` to exactly `npm start`; the
supplied start script runs `src/index.mjs`.
Start and stop the Worker only through Compose; do not launch `npm`, `node`, or the Worker directly.

Register these exact functions with non-empty descriptions and JSON request/response schemas:
  - `{legal}`: `{{"fen": string}} -> {{"moves": sorted UCI string[]}}`
  - `{perft}`: `{{"fen": string, "depth": integer 0..4}} -> {{"nodes": integer}}`
  - `{play}`: `{{"fen": string, "move": UCI string}} -> {{"fen": resulting FEN}}`
  - `{view}`: `{{"fen": string}} -> {{"html": complete HTML document}}`

Reject malformed/illegal FENs and moves and depths outside 0..4. Implement standard chess,
including castling, en passant, four promotion pieces, pins, and check evasion.

The HTML returned by `{view}` is the application. It must render exactly 64 board squares carrying
`data-square="a1"` through `data-square="h8"`; occupied squares must also carry `data-piece` with
the FEN piece letter. Show the current FEN in an element with
`data-testid="fen"`, and let a user choose a source and destination square. Its own JavaScript must
POST `{{"fen": currentFen, "move": source + destination}}` to `/api/play`, replace the board with
the returned `html`, remain interactive for the next move, and show failures in an element with `data-testid="error"`. The page must be
usable at 1280x900 without external assets.

Validate the Compose file, start the Worker, inspect all four registered functions, exercise the
rules, and inspect the generated view HTML and interaction contract before reporting completion.
The runner will proxy and drive the page after you finish."#,
                root = root.display(),
                compose = root.join("worker-compose.yaml").display(),
                worker = contract.worker,
                legal = contract.functions["legal_moves"],
                perft = contract.functions["perft"],
                play = contract.functions["play"],
                view = contract.functions["view"],
            ),
            filesystem_root: Some(root),
            execution: ExecutionPolicy {
                max_turns: Some(64),
                max_output_tokens: Some(65_536),
                max_total_tokens: Some(1_200_000),
                stuck_timeout_seconds: 900,
                max_validation_retries: None,
            },
            denied_functions: &["http::*", "browser::*", "github::*"],
            criteria: assessment::criteria(ASSESSMENTS),
        }
    }

    async fn setup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        for function in [
            "coder::read-file",
            "coder::update-file",
            "shell::exec",
            "compose::validate",
            "compose::up",
            "compose::status",
            "compose::down",
            "engine::functions::info",
            "browser::sessions::start",
            "browser::navigate",
            "browser::resize",
            "browser::execute",
            "browser::screenshot",
            "browser::sessions::stop",
        ] {
            if !context.function_exists(function).await? {
                bail!("required chess Worker validation mechanism '{function}' is unavailable");
            }
        }
        prepare_workspace(run_id).await
    }

    async fn capture(
        &self,
        context: &E2eContext,
        _observation: &ScenarioObservation,
        run_id: &str,
    ) -> Result<Vec<CapturedDeliverable>> {
        let evidence = validate_candidate(context, run_id).await?;
        let invariants = ["runtime_contract", "playable_ui", "evidence_complete"]
            .into_iter()
            .map(|id| CapturedInvariant {
                id: id.to_string(),
                passed: passed(&evidence, id),
                reason: reason(&evidence, id),
            })
            .collect();
        Ok(vec![CapturedDeliverable {
            id: EVIDENCE_ID.into(),
            kind: "chess_worker_audit".into(),
            content: CapturedDeliverableContent::Json(evidence),
            invariants,
            provenance: vec![ProvenanceEvidence {
                kind: "filesystem_path".into(),
                source_id: workspace_root(run_id).display().to_string(),
                relation: "validated_before_cleanup".into(),
            }],
        }])
    }

    async fn evaluate(
        &self,
        _context: &E2eContext,
        observation: &ScenarioObservation,
        _run_id: &str,
    ) -> Result<ObjectiveEvaluation> {
        let evidence = observation
            .deliverables
            .iter()
            .find(|item| item.id == EVIDENCE_ID)
            .and_then(|item| item.content.as_json())
            .context("chess Worker evidence deliverable is missing")?;
        let specs = [
            RUNTIME_CONTRACT,
            START_RULES,
            CASTLING_RULES,
            EN_PASSANT_RULES,
            PROMOTION_RULES,
            CHECK_RULES,
            PLAY_RULES,
            INVALID_INPUTS,
            PLAYABLE_UI,
            EVIDENCE_COMPLETE,
        ];
        Ok(assessment::build_evaluation(
            if observation.metrics.complete {
                CompletionState::Completed
            } else {
                CompletionState::TaskIncomplete
            },
            specs.map(|spec| {
                spec.full_or_zero(passed(evidence, spec.id()), reason(evidence, spec.id()))
            }),
        ))
    }

    async fn cleanup(&self, context: &E2eContext, run_id: &str) -> Result<()> {
        let root = workspace_root(run_id);
        let compose = root.join("worker-compose.yaml");
        if compose.is_file() {
            match context
                .trigger_value("compose::validate", json!({"file": compose}))
                .await
            {
                Ok(_) => {
                    context
                        .trigger_value("compose::down", json!({"file": compose}))
                        .await
                        .context("stop run-scoped chess Worker before removing its workspace")?;
                }
                Err(error) if is_remote_failure(&error) => {
                    let functions = context
                        .trigger_value(
                            "engine::functions::info",
                            json!({"function_ids":worker_contract(run_id).functions.values().collect::<Vec<_>>()}),
                        )
                        .await
                        .context("inspect chess functions after invalid Compose delivery")?;
                    if functions["functions"].as_array().is_some_and(|registered| {
                        registered.iter().any(|item| item.get("error").is_none())
                    }) {
                        bail!("invalid chess Compose file has live functions that cannot be safely stopped");
                    }
                }
                Err(error) => {
                    return Err(error.context("validate chess Compose file before cleanup"));
                }
            }
        }
        remove_workspace(&root)
    }
}

#[derive(Clone)]
struct WorkerContract {
    worker: String,
    functions: BTreeMap<&'static str, String>,
}

fn worker_contract(run_id: &str) -> WorkerContract {
    let suffix: String = run_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(16)
        .collect();
    let worker = format!("chess_{}", if suffix.is_empty() { "run" } else { &suffix });
    let functions = ["legal_moves", "perft", "play", "view"]
        .into_iter()
        .map(|name| (name, format!("{worker}::{name}")))
        .collect();
    WorkerContract { worker, functions }
}

fn deliverable_contract() -> DeliverableContract {
    DeliverableContract {
        artifacts: vec![ArtifactExpectation {
            id: EVIDENCE_ID.into(),
            kind: "chess_worker_audit".into(),
            media_type: "application/json".into(),
            schema: json!({
                "type": "object",
                "required": ["identity", "checks", "files"],
                "properties": {
                    "identity": {"type": "object"},
                    "checks": {"type": "object"},
                    "files": {"type": "object"}
                }
            }),
            max_size_bytes: EVIDENCE_LIMIT,
        }],
        invariants: vec![
            InvariantSpec {
                id: "runtime_contract".into(),
                description: "The run-scoped Worker is live with the required function surface."
                    .into(),
            },
            InvariantSpec {
                id: "playable_ui".into(),
                description: "A browser completes e2-e4 and e7-e5 through Worker-authored UI."
                    .into(),
            },
            InvariantSpec {
                id: "evidence_complete".into(),
                description: "Real before and after screenshots are portable and identity-bound."
                    .into(),
            },
        ],
        provenance_required: true,
        capture_before_cleanup: true,
    }
}

async fn validate_candidate(context: &E2eContext, run_id: &str) -> Result<Value> {
    let root = workspace_root(run_id);
    let compose = root.join("worker-compose.yaml");
    let contract = worker_contract(run_id);
    let source_sha256 = directory_sha256(&root).ok();
    let compose_sha256 = fs::read(&compose)
        .ok()
        .map(|bytes| crate::artifact::sha256_bytes(&bytes));
    let mut checks = serde_json::Map::new();

    let compose_present = compose.is_file();
    let local_contract = fs::read_to_string(&compose)
        .ok()
        .and_then(|text| serde_yaml::from_str::<Value>(&text).ok())
        .is_some_and(|yaml| {
            let Some(containers) = yaml["containers"].as_object() else {
                return false;
            };
            containers.len() == 1
                && containers[&contract.worker]
                    .get("worker")
                    .and_then(Value::as_str)
                    == Some("path://.")
                && containers[&contract.worker]
                    .pointer("/scripts/run")
                    .and_then(Value::as_str)
                    == Some("npm start")
                && containers[&contract.worker].get("start_after").is_none()
                && containers[&contract.worker].get("working_dir").is_none()
                && yaml.get("engine").is_none()
        });
    let validate = if compose_present {
        context
            .trigger_value("compose::validate", json!({"file": compose}))
            .await
    } else {
        Err(anyhow::anyhow!("worker-compose.yaml is missing"))
    };
    if compose_present {
        ensure_remote_or_success(&validate, "validate candidate Compose file")?;
    }
    let compose_valid = validate.is_ok() && local_contract;
    checks.insert(
        "compose_valid".into(),
        json!({"passed":compose_valid,"reason":if compose_valid {"Compose validates and declares the exact run-scoped container"} else {"Compose validation or run-scoped container contract failed"},"local_contract":local_contract,"observed":result_value(validate)}),
    );

    let up = if compose_valid {
        context
            .trigger_value(
                "compose::up",
                json!({"file": compose, "container": contract.worker}),
            )
            .await
    } else {
        Err(anyhow::anyhow!("compose validation failed"))
    };
    if compose_valid {
        ensure_remote_or_success(&up, "start candidate Compose container")?;
    }
    let mut ready = false;
    let mut last_status = Value::Null;
    if up.is_ok() {
        for _ in 0..120 {
            match context
                .trigger_value("compose::status", json!({"file": compose}))
                .await
            {
                Ok(status) => {
                    ready = status["containers"].as_array().is_some_and(|items| {
                        items.iter().any(|item| {
                            item["container"] == contract.worker && item["state"] == "ready"
                        })
                    });
                    last_status = status;
                    if ready {
                        break;
                    }
                }
                Err(error) if is_remote_failure(&error) => {
                    last_status = json!({"error": format!("{error:#}")})
                }
                Err(error) => return Err(error.context("query candidate Compose status")),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    checks.insert(
        "worker_live".into(),
        json!({"passed":ready,"reason":if ready {"run-scoped container is ready"} else {"run-scoped container did not become ready"},"observed":last_status}),
    );

    let function_ids = contract.functions.values().cloned().collect::<Vec<_>>();
    let info = context
        .trigger_value(
            "engine::functions::info",
            json!({"function_ids":function_ids}),
        )
        .await;
    ensure_remote_or_success(&info, "inspect candidate function surface")?;
    let surface = info
        .as_ref()
        .ok()
        .and_then(|value| value["functions"].as_array())
        .is_some_and(|functions| {
            contract.functions.iter().all(|(operation, id)| {
                functions.iter().any(|function| {
                    function["function_id"] == *id
                        && function["description"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                        && function_schema_matches(operation, function)
                })
            })
        });
    checks.insert(
        "function_surface".into(),
        json!({"passed":surface,"reason":if surface {"all four described functions are registered"} else {"function surface is incomplete or malformed"},"observed":result_value(info)}),
    );
    checks.insert(
        "runtime_contract".into(),
        json!({"passed":compose_valid && ready && surface,"reason":format!("compose_valid={compose_valid}, worker_ready={ready}, function_surface={surface}")}),
    );

    let families = [
        ("rules_start", START_FEN, 4),
        ("rules_castling", KIWIPETE_FEN, 2),
        ("rules_en_passant", EN_PASSANT_FEN, 1),
        ("rules_promotion", PROMOTION_FEN, 1),
        ("rules_check_evasion", CHECK_EVASION_FEN, 1),
    ];
    for (id, fen, depth) in families {
        let expected_moves = chess_engine::legal_moves(fen)?;
        let expected_nodes = chess_engine::perft(fen, depth)?;
        let legal = invoke(
            context.client(),
            &contract.functions["legal_moves"],
            json!({"fen":fen}),
        )
        .await;
        let perft = invoke(
            context.client(),
            &contract.functions["perft"],
            json!({"fen":fen,"depth":depth}),
        )
        .await;
        ensure_remote_or_success(&legal, "invoke legal_moves probe")?;
        ensure_remote_or_success(&perft, "invoke perft probe")?;
        let actual_moves = legal
            .as_ref()
            .ok()
            .and_then(|value| value["moves"].as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
        let actual_nodes = perft
            .as_ref()
            .ok()
            .and_then(|value| value["nodes"].as_u64());
        let ok = ready
            && surface
            && actual_moves.as_ref() == Some(&expected_moves)
            && actual_nodes == Some(expected_nodes);
        checks.insert(id.into(), json!({
            "passed":ok,
            "reason":if ok {"legal moves and perft match the oracle"} else {"Worker result differs from the oracle"},
            "fen":fen,"depth":depth,"expected":{"moves":expected_moves,"nodes":expected_nodes},
            "observed":{"legal_moves":result_value(legal),"perft":result_value(perft)}
        }));
    }

    let pinned_expected_moves = chess_engine::legal_moves(PINNED_FEN)?;
    let pinned_expected_nodes = chess_engine::perft(PINNED_FEN, 1)?;
    let pinned_legal = invoke(
        context.client(),
        &contract.functions["legal_moves"],
        json!({"fen":PINNED_FEN}),
    )
    .await;
    let pinned_perft = invoke(
        context.client(),
        &contract.functions["perft"],
        json!({"fen":PINNED_FEN,"depth":1}),
    )
    .await;
    ensure_remote_or_success(&pinned_legal, "invoke pinned legal_moves probe")?;
    ensure_remote_or_success(&pinned_perft, "invoke pinned perft probe")?;
    let pinned_moves = pinned_legal
        .as_ref()
        .ok()
        .and_then(|value| value["moves"].as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    let pin_ok = pinned_moves.as_ref() == Some(&pinned_expected_moves)
        && pinned_perft
            .as_ref()
            .ok()
            .and_then(|value| value["nodes"].as_u64())
            == Some(pinned_expected_nodes);
    let check_ok = checks["rules_check_evasion"]["passed"] == true;
    checks.insert(
        "rules_check_evasion".into(),
        json!({
            "passed":check_ok && pin_ok,
            "reason":if check_ok && pin_ok {"check evasion and absolute pin match the oracle"} else {"check-evasion or pin result differs from the oracle"},
            "check_evasion":checks["rules_check_evasion"],
            "absolute_pin":{"fen":PINNED_FEN,"expected":{"moves":pinned_expected_moves,"nodes":pinned_expected_nodes},"observed":{"legal_moves":result_value(pinned_legal),"perft":result_value(pinned_perft)}}
        }),
    );

    let expected_after = chess_engine::apply_move(START_FEN, "e2e4")?.new_fen;
    let expected_final = chess_engine::apply_move(&expected_after, "e7e5")?.new_fen;
    let legal_play = invoke(
        context.client(),
        &contract.functions["play"],
        json!({"fen":START_FEN,"move":"e2e4"}),
    )
    .await;
    let illegal_play = invoke(
        context.client(),
        &contract.functions["play"],
        json!({"fen":START_FEN,"move":"e2e5"}),
    )
    .await;
    ensure_remote_or_success(&legal_play, "invoke legal play probe")?;
    ensure_remote_or_success(&illegal_play, "invoke illegal play probe")?;
    let actual_after = legal_play
        .as_ref()
        .ok()
        .and_then(|value| value["fen"].as_str())
        .map(str::to_string);
    let second_play = if let Some(actual_after) = &actual_after {
        Some(
            invoke(
                context.client(),
                &contract.functions["play"],
                json!({"fen":actual_after,"move":"e7e5"}),
            )
            .await,
        )
    } else {
        None
    };
    if let Some(second_play) = &second_play {
        ensure_remote_or_success(second_play, "invoke second legal play probe")?;
    }
    let actual_final = second_play
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|value| value["fen"].as_str())
        .map(str::to_string);
    let play_ok = actual_after
        .as_deref()
        .is_some_and(|actual| fen_equivalent(actual, &expected_after))
        && actual_final
            .as_deref()
            .is_some_and(|actual| fen_equivalent(actual, &expected_final))
        && illegal_play.as_ref().err().is_some_and(is_remote_failure);
    checks.insert("play_contract".into(), json!({
        "passed":play_ok,
        "reason":if play_ok {"both legal moves are position-equivalent to the oracle and the illegal move is rejected"} else {"play contract failed"},
        "oracle":{"after_e2e4":expected_after,"after_e7e5":expected_final},
        "actual":{"after_e2e4":actual_after,"after_e7e5":actual_final},
        "first":result_value(legal_play),
        "second":second_play.map(result_value),
        "illegal":result_value(illegal_play)
    }));

    let invalid_fen = invoke(
        context.client(),
        &contract.functions["legal_moves"],
        json!({"fen":"not a fen"}),
    )
    .await;
    let invalid_depth = invoke(
        context.client(),
        &contract.functions["perft"],
        json!({"fen":START_FEN,"depth":5}),
    )
    .await;
    let invalid_move = invoke(
        context.client(),
        &contract.functions["play"],
        json!({"fen":START_FEN,"move":"wat"}),
    )
    .await;
    ensure_remote_or_success(&invalid_fen, "invoke malformed FEN probe")?;
    ensure_remote_or_success(&invalid_depth, "invoke invalid depth probe")?;
    ensure_remote_or_success(&invalid_move, "invoke malformed move probe")?;
    let health_after_rejections = invoke(
        context.client(),
        &contract.functions["legal_moves"],
        json!({"fen":START_FEN}),
    )
    .await;
    ensure_remote_or_success(
        &health_after_rejections,
        "verify Worker health after invalid inputs",
    )?;
    let healthy = health_after_rejections
        .as_ref()
        .ok()
        .and_then(|value| value["moves"].as_array())
        .is_some_and(|moves| moves.len() == 20);
    let invalid_ok = ready
        && surface
        && invalid_fen.as_ref().err().is_some_and(is_remote_failure)
        && invalid_depth.as_ref().err().is_some_and(is_remote_failure)
        && invalid_move.as_ref().err().is_some_and(is_remote_failure)
        && healthy;
    checks.insert("invalid_inputs".into(), json!({"passed":invalid_ok,"reason":if invalid_ok {"invalid requests were rejected and Worker stayed healthy"} else {"invalid-input rejection or post-rejection health failed"},"observed":[result_value(invalid_fen),result_value(invalid_depth),result_value(invalid_move)],"health_after_rejections":result_value(health_after_rejections)}));

    let identity = json!({
        "worker":contract.worker,"functions":contract.functions,
        "source_sha256":source_sha256,"compose_sha256":compose_sha256,
        "fen":{"start":START_FEN,"oracle":{"after_e2e4":expected_after,"after_e7e5":expected_final},"actual":{"after_e2e4":actual_after,"after_e7e5":actual_final}}
    });
    let browser = if ready && surface && play_ok {
        capture_browser(
            context,
            &contract,
            &identity,
            actual_after
                .as_deref()
                .expect("play_ok requires first actual FEN"),
            actual_final
                .as_deref()
                .expect("play_ok requires second actual FEN"),
        )
        .await?
    } else {
        json!({"passed":false,"reason":"runtime and play contract are prerequisites","captures":[]})
    };
    checks.insert(
        "playable_ui".into(),
        json!({"passed":browser["passed"],"reason":browser["reason"],"observed":{"interaction":browser["interaction"],"proxy_audit":browser["proxy_audit"]}}),
    );

    let mut files = serde_json::Map::new();
    let captures = browser["captures"].clone();
    let capture_document = json!({"identity":identity,"viewport":{"width":1280,"height":900},"captures":captures,"proxy_audit":browser["proxy_audit"]});
    insert_text_file(
        &mut files,
        "screenshots/captures.json",
        &serde_json::to_string_pretty(&capture_document)?,
    )?;
    for name in ["before", "after"] {
        if let Some(data) = browser[name]["data"].as_str() {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
            insert_binary_file(&mut files, &format!("screenshots/{name}.png"), &bytes);
        }
    }
    let evidence_ok = files.contains_key("screenshots/before.png")
        && files.contains_key("screenshots/after.png")
        && browser["passed"] == true
        && source_sha256.is_some()
        && compose_sha256.is_some();
    checks.insert("evidence_complete".into(), json!({"passed":evidence_ok,"reason":if evidence_ok {"portable screenshots are bound to candidate and checked FENs"} else {"screenshot or identity evidence is incomplete"}}));
    Ok(json!({"identity":identity,"checks":checks,"files":files}))
}

fn result_value(result: Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"value":bounded_value(value)}),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    }
}

fn bounded_value(value: Value) -> Value {
    let encoded = value.to_string();
    if encoded.len() <= 16 * 1024 {
        value
    } else {
        json!({"omitted":"response exceeded 16 KiB","sha256":crate::artifact::sha256_bytes(encoded.as_bytes()),"size_bytes":encoded.len()})
    }
}

fn normalize_fen(fen: &str) -> Result<String> {
    let parsed: Fen = fen
        .parse()
        .with_context(|| format!("parse candidate FEN `{fen}`"))?;
    let position: Chess = parsed
        .into_position(CastlingMode::Standard)
        .with_context(|| format!("validate candidate FEN `{fen}`"))?;
    Ok(Fen::from_position(&position, EnPassantMode::Legal).to_string())
}

fn fen_equivalent(left: &str, right: &str) -> bool {
    normalize_fen(left)
        .and_then(|left| normalize_fen(right).map(|right| left == right))
        .unwrap_or(false)
}

fn function_schema_matches(operation: &str, function: &Value) -> bool {
    let request = &function["request_schema"];
    let response = &function["response_schema"];
    let required = |schema: &Value, field: &str| {
        schema["required"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == field))
    };
    let typed =
        |schema: &Value, field: &str, kind: &str| schema["properties"][field]["type"] == kind;
    if request["type"] != "object" || response["type"] != "object" {
        return false;
    }
    match operation {
        "legal_moves" => {
            required(request, "fen")
                && typed(request, "fen", "string")
                && required(response, "moves")
                && typed(response, "moves", "array")
                && response["properties"]["moves"]["items"]["type"] == "string"
        }
        "perft" => {
            required(request, "fen")
                && required(request, "depth")
                && typed(request, "fen", "string")
                && typed(request, "depth", "integer")
                && request["properties"]["depth"]["minimum"] == 0
                && request["properties"]["depth"]["maximum"] == 4
                && required(response, "nodes")
                && typed(response, "nodes", "integer")
        }
        "play" => {
            required(request, "fen")
                && required(request, "move")
                && typed(request, "fen", "string")
                && typed(request, "move", "string")
                && required(response, "fen")
                && typed(response, "fen", "string")
        }
        "view" => {
            required(request, "fen")
                && typed(request, "fen", "string")
                && required(response, "html")
                && typed(response, "html", "string")
        }
        _ => false,
    }
}

fn ensure_remote_or_success(result: &Result<Value>, action: &str) -> Result<()> {
    if let Err(error) = result {
        if !is_remote_failure(error) {
            bail!("{action} infrastructure failure: {error:#}");
        }
    }
    Ok(())
}

fn is_remote_failure(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<iii_sdk::errors::Error>(),
        Some(iii_sdk::errors::Error::Remote { .. })
    )
}

async fn invoke(client: &IIIClient, function_id: &str, payload: Value) -> Result<Value> {
    client
        .trigger(TriggerRequest {
            function_id: function_id.into(),
            payload,
            action: None,
            timeout_ms: Some(30_000),
        })
        .await
        .map_err(anyhow::Error::new)
        .with_context(|| format!("invoke {function_id}"))
}

#[derive(Clone)]
struct ProxyState {
    client: IIIClient,
    view: String,
    play: String,
    audit: Arc<Mutex<Vec<Value>>>,
}

async fn proxy_home(State(state): State<ProxyState>) -> Response {
    match invoke(&state.client, &state.view, json!({"fen":START_FEN})).await {
        Ok(value) => value["html"]
            .as_str()
            .map(|html| Html(html.to_string()).into_response())
            .unwrap_or_else(|| {
                (StatusCode::BAD_GATEWAY, "view response omitted html").into_response()
            }),
        Err(error) => (StatusCode::BAD_GATEWAY, format!("{error:#}")).into_response(),
    }
}

async fn proxy_play(State(state): State<ProxyState>, Json(payload): Json<Value>) -> Response {
    let request = payload.clone();
    match invoke(&state.client, &state.play, payload).await {
        Ok(play) => {
            let Some(fen) = play["fen"].as_str() else {
                return (StatusCode::BAD_GATEWAY, "play response omitted fen").into_response();
            };
            match invoke(&state.client, &state.view, json!({"fen":fen})).await {
                Ok(view) if view["html"].is_string() => {
                    state.audit.lock().await.push(json!({
                        "request": request,
                        "play_fen": fen,
                        "view_returned": true
                    }));
                    Json(json!({"fen":fen,"html":view["html"]})).into_response()
                }
                Ok(_) => (StatusCode::BAD_GATEWAY, "view response omitted html").into_response(),
                Err(error) => (StatusCode::BAD_GATEWAY, format!("{error:#}")).into_response(),
            }
        }
        Err(error) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{error:#}")).into_response(),
    }
}

async fn capture_browser(
    context: &E2eContext,
    contract: &WorkerContract,
    identity: &Value,
    actual_after: &str,
    actual_final: &str,
) -> Result<Value> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let audit = Arc::new(Mutex::new(Vec::new()));
    let state = ProxyState {
        client: context.client().clone(),
        view: contract.functions["view"].clone(),
        play: contract.functions["play"].clone(),
        audit: audit.clone(),
    };
    let router = Router::new()
        .route("/", get(proxy_home))
        .route("/api/play", post(proxy_play))
        .with_state(state);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let started = context
        .trigger_value(
            "browser::sessions::start",
            json!({"incognito":true,"ttl_ms":300000}),
        )
        .await?;
    let session = started["session_id"]
        .as_str()
        .context("browser session omitted session_id")?
        .to_string();
    let result = capture_browser_session(
        context,
        &session,
        &format!("http://{address}"),
        identity,
        actual_after,
        actual_final,
    )
    .await;
    let stop = context
        .trigger_value("browser::sessions::stop", json!({"session_id":session}))
        .await;
    let _ = shutdown_tx.send(());
    let _ = proxy.await;
    stop.context("stop chess evidence browser session")?;
    let mut evidence = result?;
    let calls = audit.lock().await.clone();
    let routed = calls.iter().any(|call| {
        call["request"]["fen"] == START_FEN
            && call["request"]["move"] == "e2e4"
            && call["play_fen"] == actual_after
            && call["view_returned"] == true
    });
    let routed_second = calls.iter().any(|call| {
        call["request"]["fen"] == actual_after
            && call["request"]["move"] == "e7e5"
            && call["play_fen"] == actual_final
            && call["view_returned"] == true
    });
    evidence["proxy_audit"] = json!(calls);
    evidence["passed"] = json!(evidence["passed"] == true && routed && routed_second);
    if !routed || !routed_second {
        evidence["reason"] =
            json!("browser did not complete both moves through the runner transport proxy");
    }
    Ok(evidence)
}

async fn capture_browser_session(
    context: &E2eContext,
    session: &str,
    url: &str,
    identity: &Value,
    actual_after: &str,
    actual_final: &str,
) -> Result<Value> {
    context
        .trigger_value(
            "browser::resize",
            json!({"session_id":session,"width":1280,"height":900}),
        )
        .await?;
    let navigation = context
        .trigger_value(
            "browser::navigate",
            json!({"session_id":session,"url":url,"timeout_ms":30000}),
        )
        .await?;
    if navigation["ok"] != true || navigation["timed_out"] == true {
        return Ok(
            json!({"passed":false,"reason":format!("Worker-authored page could not be rendered: {navigation}"),"captures":[]}),
        );
    }
    let before_state = inspect_ui(context, session, START_FEN, false).await?;
    let before = screenshot_png(context, session).await?;
    let interaction_code = format!(
        "const expectedFirst = {}; const expectedFinal = {};\n{}",
        serde_json::to_string(actual_after)?,
        serde_json::to_string(actual_final)?,
        r#"return await (async () => {
          const click = square => {
            const element = document.querySelector(`[data-square="${square}"]`);
            if (!element) throw new Error(`missing ${square}`);
            element.click();
          };
          const waitFor = async expected => { for (let i = 0; i < 100; i++) {
            const fen = document.querySelector('[data-testid="fen"]')?.textContent?.trim();
            if (fen === expected) return {fen, squares: document.querySelectorAll('[data-square]').length};
            await new Promise(resolve => setTimeout(resolve, 50));
          } return null; };
          click('e2'); click('e4');
          const first = await waitFor(expectedFirst);
          if (!first) return {error:'e2e4 did not render', fen:document.querySelector('[data-testid="fen"]')?.textContent?.trim()};
          click('e7'); click('e5');
          const second = await waitFor(expectedFinal);
          return second ? {fen:second.fen, transitions:[first.fen,second.fen]} : {error:'e7e5 did not render',fen:document.querySelector('[data-testid="fen"]')?.textContent?.trim()};
        })();"#
    );
    let interaction = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"timeout_ms":30000,"code":interaction_code}),
        )
        .await?;
    let after_state = inspect_ui(context, session, actual_final, true).await?;
    let after = screenshot_png(context, session).await?;
    let passed = before_state["passed"] == true
        && after_state["passed"] == true
        && before["data"].is_string()
        && after["data"].is_string()
        && interaction["result"]["fen"] == actual_final;
    let captures = json!([
        {"id":"before","caption":"Playable chess Worker before e2-e4","url":url,"status":"captured","screenshot":"before.png","session_id":session,"details":before["details"],"identity":identity,"fen":START_FEN,"sha256":before["sha256"]},
        {"id":"after","caption":"Playable chess Worker after e2-e4 and e7-e5","url":url,"status":"captured","screenshot":"after.png","session_id":session,"details":after["details"],"identity":identity,"fen":actual_final,"transitions":[actual_after,actual_final],"sha256":after["sha256"]}
    ]);
    Ok(
        json!({"passed":passed,"reason":if passed {"browser rendered a visible board and completed two moves"} else {"browser interaction, visibility, pieces, or checked FEN failed"},"captures":captures,"before":before,"after":after,"interaction":interaction["result"]}),
    )
}

async fn inspect_ui(
    context: &E2eContext,
    session: &str,
    expected_fen: &str,
    after_move: bool,
) -> Result<Value> {
    let expected = serde_json::to_string(expected_fen)?;
    let code = format!(
        "const expected = {expected}; const afterMove = {after_move};\n{}",
        r#"const fen = document.querySelector('[data-testid="fen"]')?.textContent?.trim();
        const elements = [...document.querySelectorAll('[data-square]')];
        const squares = elements.map(element => element.dataset.square);
        const expectedSquares = [...'abcdefgh'].flatMap(file => [...'12345678'].map(rank => file + rank));
        const visible = element => { const rect=element.getBoundingClientRect(); const style=getComputedStyle(element); return rect.width>=20 && rect.height>=20 && rect.bottom>0 && rect.right>0 && rect.top<innerHeight && rect.left<innerWidth && style.visibility!=='hidden' && style.display!=='none' && Number(style.opacity)>0; };
        const hit = element => { const rect=element.getBoundingClientRect(); return document.elementFromPoint(rect.left+rect.width/2,rect.top+rect.height/2)===element || element.contains(document.elementFromPoint(rect.left+rect.width/2,rect.top+rect.height/2)); };
        const piece = square => document.querySelector(`[data-square="${square}"]`)?.dataset.piece || '';
        const e2=piece('e2'), e4=piece('e4'), e7=piece('e7'), e5=piece('e5');
        const piecesOk = afterMove ? e2==='' && e4==='P' && e7==='' && e5==='p' : e2==='P' && e4==='' && e7==='p' && e5==='';
        const occupiedVisible = elements.filter(element=>element.dataset.piece).every(element=>visible(element) && ((element.textContent||'').trim()!=='' || getComputedStyle(element).backgroundImage!=='none'));
        const targets = afterMove ? ['e7','e5'] : ['e2','e4'];
        const targetsHit = targets.every(square => { const element=document.querySelector(`[data-square="${square}"]`); return element && visible(element) && hit(element); });
        return {passed: fen===expected && elements.length===64 && new Set(squares).size===64 && expectedSquares.every(square => squares.includes(square)) && elements.every(visible) && occupiedVisible && piecesOk && targetsHit,fen,square_count:squares.length,e2,e4,e7,e5,targets_hit:targetsHit};"#
    );
    let value = context
        .trigger_value(
            "browser::execute",
            json!({"session_id":session,"code":code,"timeout_ms":30000}),
        )
        .await?;
    Ok(value["result"].clone())
}

async fn screenshot_png(context: &E2eContext, session: &str) -> Result<Value> {
    let value = context
        .trigger_value(
            "browser::screenshot",
            json!({"session_id":session,"full_page":true,"format":"png"}),
        )
        .await?;
    if value["details"]["session_id"] != session {
        bail!("browser screenshot session identity mismatch");
    }
    let block = value["content"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"] == "image" && item["mime"] == "image/png")
        })
        .context("browser screenshot omitted PNG")?;
    let data = block["data"].as_str().context("browser PNG data missing")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    if bytes.len() > MAX_SCREENSHOT_BYTES {
        return Ok(
            json!({"oversized":true,"size_bytes":bytes.len(),"maximum_bytes":MAX_SCREENSHOT_BYTES,"details":value["details"]}),
        );
    }
    Ok(
        json!({"data":data,"sha256":crate::artifact::sha256_bytes(&bytes),"details":value["details"]}),
    )
}

fn passed(evidence: &Value, id: &str) -> bool {
    evidence["checks"][id]["passed"] == true
}

fn reason(evidence: &Value, id: &str) -> String {
    evidence["checks"][id]["reason"]
        .as_str()
        .unwrap_or("check did not produce a reason")
        .to_string()
}

fn insert_text_file(
    files: &mut serde_json::Map<String, Value>,
    name: &str,
    text: &str,
) -> Result<()> {
    let bytes = text.as_bytes();
    files.insert(name.into(), json!({"encoding":"utf8","content":text,"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(bytes)}));
    Ok(())
}

fn insert_binary_file(files: &mut serde_json::Map<String, Value>, name: &str, bytes: &[u8]) {
    files.insert(name.into(), json!({"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(bytes),"size_bytes":bytes.len(),"sha256":crate::artifact::sha256_bytes(bytes)}));
}

async fn prepare_workspace(run_id: &str) -> Result<()> {
    let fixture = super::fixture::prepare(super::fixture::SHARED_BUNDLE, FIXTURE_REVISION).await?;
    let chess = fixture.root.join(CHESS_SUBTREE);
    if fixture_manifest_sha256(&chess)? != CHESS_MANIFEST_SHA256 {
        bail!("frozen chess fixture manifest differs from pinned identity");
    }
    let workspace = workspace_root(run_id);
    remove_workspace(&workspace)?;
    fs::create_dir_all(&workspace)?;
    copy_subtree(&chess, &workspace)?;
    let original_readme = workspace.join("README.md");
    if original_readme.is_file() {
        fs::rename(&original_readme, workspace.join("FIXTURE_README.md"))?;
    }
    let original_protocol = workspace.join("PROTOCOL.md");
    if original_protocol.is_file() {
        fs::rename(&original_protocol, workspace.join("FIXTURE_PROTOCOL.md"))?;
    }
    let contract = worker_contract(run_id);
    fs::write(
        &original_readme,
        format!(
            "# Chess Worker task\n\nThe scenario prompt is authoritative. Build a run-scoped iii Worker and playable HTML board. The frozen `engine/` directory contains a CLI skeleton and public tests; the CLI is not scored as a standalone program. Implement chess logic behind the Worker functions. Chess libraries are forbidden. Harness already materialized the exact iii-sdk integration dependency and lockfile; do not change dependencies. Compose must run `npm start` and use the existing Engine without an `engine:` section.\n\nMinimal registration shape:\n\n```js\nimport {{ registerWorker }} from 'iii-sdk'\nconst iii = registerWorker(process.env.III_ENGINE_URL ?? process.env.III_URL, {{ workerName: '{}' }})\niii.registerFunction('{}', async (payload) => {{ /* implement */ }}, {{ description: '...', request_format: {{ type: 'object', properties: {{}} }}, response_format: {{ type: 'object', properties: {{}} }} }})\n```\n",
            contract.worker, contract.functions["legal_moves"]
        ),
    )?;
    fs::write(
        workspace.join("package.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "harness-chess-worker",
            "private": true,
            "type": "module",
            "scripts": {"start": "node src/index.mjs"},
            "dependencies": {"iii-sdk": "0.23.1-rc.6"}
        }))?,
    )?;
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chess-worker/package-lock.json"),
        workspace.join("package-lock.json"),
    )?;
    let mut install = Command::new("npm");
    install
        .args([
            "ci",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--prefer-offline",
        ])
        .current_dir(&workspace)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(120), install.output())
        .await
        .context("materialize pinned iii-sdk timed out after 120 seconds")??;
    if !output.status.success() {
        bail!(
            "could not materialize pinned iii-sdk: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn directory_sha256(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut bytes = Vec::new();
    let mut total_source_bytes = 0_u64;
    for relative in files {
        let content = fs::read(root.join(&relative))?;
        total_source_bytes = total_source_bytes.saturating_add(content.len() as u64);
        if total_source_bytes > 8 * 1024 * 1024 {
            bail!("candidate source fingerprint exceeds 8 MiB");
        }
        bytes.extend_from_slice(relative.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(
            crate::artifact::sha256_bytes(&content)
                .trim_start_matches("sha256:")
                .as_bytes(),
        );
        bytes.push(b'\n');
    }
    Ok(crate::artifact::sha256_bytes(&bytes))
}

fn fixture_manifest_sha256(root: &Path) -> Result<String> {
    fn collect(root: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                if entry.file_name() != OsStr::new("__pycache__") {
                    collect(root, &path, files)?;
                }
            } else if path.extension().and_then(OsStr::to_str) != Some("pyc") {
                files.push(
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort();
    let mut bytes = Vec::new();
    for relative in files {
        let content = fs::read(root.join(&relative))?;
        bytes.extend_from_slice(relative.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(
            crate::artifact::sha256_bytes(&content)
                .trim_start_matches("sha256:")
                .as_bytes(),
        );
        bytes.push(b'\n');
    }
    Ok(crate::artifact::sha256_bytes(&bytes))
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_symlink() {
            bail!(
                "candidate source fingerprint rejects symlink {}",
                path.display()
            );
        }
        if entry.file_type()?.is_dir() {
            if !matches!(
                entry.file_name().to_str(),
                Some("__pycache__" | "node_modules" | ".git" | ".iii" | "target" | ".harness-e2e")
            ) {
                collect_files(root, &path, files)?;
            }
        } else if entry.file_type()?.is_file() {
            files.push(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            if files.len() > 256 {
                bail!("candidate source fingerprint exceeds 256 files");
            }
        }
    }
    Ok(())
}

fn copy_subtree(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let into = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            if entry.file_name() == OsStr::new("__pycache__") {
                continue;
            }
            fs::create_dir_all(&into)?;
            copy_subtree(&from, &into)?;
        } else if from.extension().and_then(OsStr::to_str) != Some("pyc") {
            fs::copy(from, into)?;
        }
    }
    Ok(())
}

fn workspace_root(run_id: &str) -> PathBuf {
    let base = std::env::var_os("HARNESS_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::canonicalize(&base)
        .unwrap_or(base)
        .join("scenario-workspaces")
        .join(format!("{ID}-{run_id}"))
}

fn remove_workspace(root: &Path) -> Result<()> {
    let parent = root.parent().context("workspace path has no parent")?;
    let name = root
        .file_name()
        .and_then(OsStr::to_str)
        .context("workspace path has no name")?;
    if parent.file_name() != Some(OsStr::new("scenario-workspaces"))
        || !name.starts_with(&format!("{ID}-"))
    {
        bail!("refusing to remove workspace outside the scenario-owned base");
    }
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_is_run_scoped_and_weights_total_one_hundred() {
        let first = worker_contract("run-a");
        let second = worker_contract("run-b");
        assert_ne!(first.worker, second.worker);
        assert!(first
            .functions
            .values()
            .all(|id| id.starts_with(&first.worker)));
        let materialized = crate::scenarios::ScenarioId::ChessEngineBuild
            .materialize("run-a", 7)
            .unwrap();
        materialized.spec.validate().unwrap();
        assert_eq!(
            materialized
                .spec
                .criteria
                .iter()
                .map(|item| u16::from(item.weight))
                .sum::<u16>(),
            100
        );
    }

    #[test]
    fn evidence_gate_rejects_missing_or_failed_screenshots() {
        let evidence =
            json!({"checks":{"evidence_complete":{"passed":false,"reason":"missing after.png"}}});
        assert!(!passed(&evidence, "evidence_complete"));
        assert_eq!(reason(&evidence, "evidence_complete"), "missing after.png");
    }

    #[test]
    fn family_positions_cover_special_rules_and_have_oracle_answers() {
        for fen in [
            START_FEN,
            KIWIPETE_FEN,
            EN_PASSANT_FEN,
            PROMOTION_FEN,
            CHECK_EVASION_FEN,
        ] {
            assert!(!chess_engine::legal_moves(fen).unwrap().is_empty());
            assert!(chess_engine::perft(fen, 1).unwrap() > 0);
        }
        assert!(chess_engine::legal_moves(EN_PASSANT_FEN)
            .unwrap()
            .contains(&"e5d6".into()));
        assert!(chess_engine::legal_moves(PROMOTION_FEN)
            .unwrap()
            .iter()
            .any(|mv| mv.ends_with('q')));
    }

    #[test]
    fn portable_files_include_bytes_hash_and_encoding() {
        let mut files = serde_json::Map::new();
        insert_text_file(&mut files, "screenshots/captures.json", "{}").unwrap();
        insert_binary_file(&mut files, "screenshots/before.png", &[137, 80, 78, 71]);
        assert_eq!(files["screenshots/captures.json"]["encoding"], "utf8");
        assert_eq!(files["screenshots/before.png"]["encoding"], "base64");
        assert!(files["screenshots/before.png"]["sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn perft_surface_rejects_a_depth_schema_that_exceeds_the_contract() {
        let mut function = json!({
            "request_schema":{"type":"object","required":["fen","depth"],"properties":{"fen":{"type":"string"},"depth":{"type":"integer","minimum":0,"maximum":4}}},
            "response_schema":{"type":"object","required":["nodes"],"properties":{"nodes":{"type":"integer"}}}
        });
        assert!(function_schema_matches("perft", &function));
        function["request_schema"]["properties"]["depth"]["maximum"] = json!(99);
        assert!(!function_schema_matches("perft", &function));
    }

    #[test]
    fn sdk_lock_pins_the_worker_integration_package_and_integrities() {
        let lock: Value = serde_json::from_slice(
            &fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/chess-worker/package-lock.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            lock["packages"]["node_modules/iii-sdk"]["version"],
            "0.23.1-rc.6"
        );
        assert!(lock["packages"]
            .as_object()
            .unwrap()
            .values()
            .filter(|package| package["resolved"].is_string())
            .all(|package| package["integrity"].is_string()));
    }

    #[test]
    fn fen_equivalence_accepts_conventional_irrelevant_en_passant_targets() {
        let oracle_after = chess_engine::apply_move(START_FEN, "e2e4").unwrap().new_fen;
        let conventional_after = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1";
        assert!(fen_equivalent(conventional_after, &oracle_after));

        let oracle_final = chess_engine::apply_move(&oracle_after, "e7e5")
            .unwrap()
            .new_fen;
        let conventional_final = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2";
        assert!(fen_equivalent(conventional_final, &oracle_final));
        assert!(!fen_equivalent(START_FEN, conventional_after));
    }
}
