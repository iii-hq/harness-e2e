#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
III_URL="${III_URL:-ws://127.0.0.1:49134}"
E2E_MODEL="${E2E_MODEL:-codex/gpt-5.6-luna}"
E2E_PROVIDER="${E2E_PROVIDER:-openai-codex}"
E2E_SCENARIO="${E2E_SCENARIO:-coordination.1}"
E2E_SEED="${E2E_SEED:-4404}"
E2E_TIMEOUT_SECONDS="${E2E_TIMEOUT_SECONDS:-1800}"
E2E_POLL_SECONDS="${E2E_POLL_SECONDS:-2}"
E2E_ALLOW_LEGACY="${E2E_ALLOW_LEGACY:-false}"
E2E_DEMO_ROOT="${E2E_DEMO_ROOT:-${ROOT_DIR}/target/e2e-demo}"
E2E_WORKERS_REPOSITORY="${HARNESS_E2E_WORKERS_REPOSITORY:-}"
E2E_WORKERS_REVISION="${HARNESS_E2E_WORKERS_REVISION:-}"
CATALOG_ONLY=false

usage() {
  printf '%s\n' \
    'Usage: scripts/demo_e2e_control_plane.sh [--catalog-only] [--allow-legacy-control-plane]' \
    '' \
    'Environment overrides:' \
    '  III_URL, E2E_MODEL, E2E_PROVIDER, E2E_SCENARIO, E2E_SEED' \
    '  E2E_TIMEOUT_SECONDS, E2E_POLL_SECONDS, E2E_DEMO_ROOT' \
    '  HARNESS_E2E_WORKERS_REPOSITORY, HARNESS_E2E_WORKERS_REVISION'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --catalog-only)
      CATALOG_ONLY=true
      ;;
    --allow-legacy-control-plane)
      E2E_ALLOW_LEGACY=true
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

for command in cargo iii jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command not found: %s\n' "$command" >&2
    exit 1
  fi
done

endpoint="${III_URL#ws://}"
endpoint="${endpoint#wss://}"
endpoint="${endpoint%%/*}"
III_ADDRESS="${endpoint%:*}"
III_PORT="${endpoint##*:}"
if [[ -z "$III_ADDRESS" || -z "$III_PORT" || "$III_ADDRESS" == "$III_PORT" ]]; then
  printf 'III_URL must include a host and port: %s\n' "$III_URL" >&2
  exit 2
fi

if [[ "$CATALOG_ONLY" != true ]]; then
  if [[ -z "$E2E_WORKERS_REPOSITORY" ]]; then
    printf 'HARNESS_E2E_WORKERS_REPOSITORY is required for an attributable run\n' >&2
    exit 2
  fi
  if [[ ! "$E2E_WORKERS_REVISION" =~ ^[0-9a-fA-F]{40}$ ]]; then
    printf 'HARNESS_E2E_WORKERS_REVISION must be the full 40-character subject SHA\n' >&2
    exit 2
  fi
fi

mkdir -p "$E2E_DEMO_ROOT"
WORKER_LOG="${E2E_DEMO_ROOT}/worker.log"
WORKER_PID=''

cleanup() {
  if [[ -n "$WORKER_PID" ]] && kill -0 "$WORKER_PID" 2>/dev/null; then
    kill "$WORKER_PID" 2>/dev/null || true
    wait "$WORKER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

trigger() {
  local function_id="$1"
  local payload="$2"
  local timeout_ms="${3:-30000}"
  iii trigger "$function_id" \
    --address "$III_ADDRESS" \
    --port "$III_PORT" \
    --timeout-ms "$timeout_ms" \
    --json "$payload"
}

printf 'Building the trusted e2e-worker from %s\n' "$ROOT_DIR"
cargo build --locked --manifest-path "${ROOT_DIR}/Cargo.toml" --bin e2e-worker

printf 'Starting e2e::* on %s\n' "$III_URL"
HARNESS_E2E_WORKERS_REPOSITORY="$E2E_WORKERS_REPOSITORY" \
HARNESS_E2E_WORKERS_REVISION="$E2E_WORKERS_REVISION" \
"${ROOT_DIR}/target/debug/e2e-worker" \
  --url "$III_URL" \
  --output-root "${E2E_DEMO_ROOT}/runs" \
  >"$WORKER_LOG" 2>&1 &
WORKER_PID=$!

catalog=''
for _ in $(seq 1 60); do
  if catalog="$(trigger e2e::scenarios-list "$(jq -cn --argjson seed "$E2E_SEED" '{seed: $seed}')" 1000 2>/dev/null)"; then
    break
  fi
  if ! kill -0 "$WORKER_PID" 2>/dev/null; then
    printf 'e2e-worker exited during startup; see %s\n' "$WORKER_LOG" >&2
    exit 1
  fi
  sleep 0.5
done

if [[ -z "$catalog" ]]; then
  printf 'e2e::* did not become ready; see %s\n' "$WORKER_LOG" >&2
  exit 1
fi

scenario_descriptor="$(jq -c --arg scenario "$E2E_SCENARIO" \
  '.scenarios[] | select(.scenario_id == $scenario)' <<<"$catalog")"
if [[ -z "$scenario_descriptor" ]]; then
  printf 'scenario is not registered: %s\n' "$E2E_SCENARIO" >&2
  exit 2
fi

printf '\nMaterialized scenario contract\n'
jq '{scenario_id, scenario_version, case_id, seed, complexity, required_capabilities, deliverable_contract}' \
  <<<"$scenario_descriptor"

if [[ "$CATALOG_ONLY" == true ]]; then
  printf '\nCatalog-only demo completed without invoking a model.\n'
  exit 0
fi

idempotency_key="demo-$(date -u +%Y%m%dT%H%M%SZ)-${E2E_SCENARIO//./-}-${E2E_SEED}"
request="$(jq -cn \
  --arg idempotency_key "$idempotency_key" \
  --arg model "$E2E_MODEL" \
  --arg provider "$E2E_PROVIDER" \
  --arg scenario "$E2E_SCENARIO" \
  --argjson seed "$E2E_SEED" \
  --argjson allow_legacy "$E2E_ALLOW_LEGACY" \
  '{
    idempotency_key: $idempotency_key,
    lane: "local-demo",
    model: $model,
    provider: $provider,
    scenarios: [$scenario],
    runs: 1,
    seed: $seed,
    rotating_seeds: [],
    technical_retries: 1,
    progress_interval_seconds: 2,
    allow_legacy_control_plane: $allow_legacy
  }')"

printf '\nSubmitting the asynchronous execution\n'
accepted="$(trigger e2e::run "$request")"
jq . <<<"$accepted"
execution_id="$(jq -er '.execution_id' <<<"$accepted")"

deadline=$((SECONDS + E2E_TIMEOUT_SECONDS))
last_signature=''
last_heartbeat=0
while true; do
  status="$(trigger e2e::status "$(jq -cn --arg execution_id "$execution_id" '{execution_id: $execution_id}')")"
  signature="$(jq -r '[.phase, (.active_attempt.scenario_id // "-"), (.active_attempt.session_id // "-")] | @tsv' <<<"$status")"
  if [[ "$signature" != "$last_signature" ]] || (( SECONDS - last_heartbeat >= 10 )); then
    printf '%ss\t%s\n' "$SECONDS" "$signature"
    last_signature="$signature"
    last_heartbeat=$SECONDS
  fi
  if [[ "$(jq -r '.terminal' <<<"$status")" == true ]]; then
    break
  fi
  if (( SECONDS >= deadline )); then
    printf 'execution timed out after %s seconds: %s\n' "$E2E_TIMEOUT_SECONDS" "$execution_id" >&2
    exit 1
  fi
  sleep "$E2E_POLL_SECONDS"
done

result="$(trigger e2e::results-get "$(jq -cn --arg execution_id "$execution_id" '{execution_id: $execution_id}')")"
result_file="${E2E_DEMO_ROOT}/${execution_id}-control-result.json"
printf '%s\n' "$result" >"$result_file"

printf '\nTerminal status\n'
jq '{execution_id, phase, error, transitions}' <<<"$status"

if [[ "$(jq -r '.phase' <<<"$status")" != completed ]]; then
  printf '\nExecution did not complete successfully. Full control result: %s\n' "$result_file" >&2
  exit 1
fi

printf '\nDurable result\n'
summary_file="${E2E_DEMO_ROOT}/${execution_id}-demo-summary.json"
jq '{
  execution_id,
  phase,
  result_path,
  archive,
  passed: .report.passed,
  execution: .report.execution,
  subject: .report.subject,
  system_under_test: .report.system_under_test,
  scenarios: [.report.scenarios[] | {
    scenario_id,
    tier: .case.complexity.tier,
    passed,
    aggregate,
    runs: [.runs[] | {
      run_id,
      attempt_id,
      session_id,
      wall_time_ms,
      score,
      status,
      hard_gates,
      deliverables,
      efficiency
    }]
  }]
}' <<<"$result" | tee "$summary_file"
printf '\nFull control result: %s\n' "$result_file"
printf 'Presentation summary: %s\n' "$summary_file"
printf 'Canonical run directory: %s\n' "${E2E_DEMO_ROOT}/runs/${execution_id}"
printf 'Worker log: %s\n' "$WORKER_LOG"

if [[ "$(jq -r '.report.passed' <<<"$result")" != true ]]; then
  printf 'The control-plane operation completed, but the evaluated E2E run did not pass.\n' >&2
  exit 1
fi
