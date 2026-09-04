#!/usr/bin/env bash
set -Eeuo pipefail

: "${HARNESS_E2E_STACK_LOCK:?HARNESS_E2E_STACK_LOCK is required}"
: "${HARNESS_E2E_CAMPAIGN_GROUP_ID:?HARNESS_E2E_CAMPAIGN_GROUP_ID is required}"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/.." && pwd)
contract_tool="$repo_root/scripts/exact_stack_campaign.py"
artifact_dir=${HARNESS_E2E_ARTIFACTS_DIR:-"$repo_root/target/harness-e2e-shadow"}
engine_port=${HARNESS_E2E_ENGINE_PORT:-49134}
wait_seconds=${HARNESS_E2E_WAIT_SECONDS:-300}
admission_timeout_seconds=${HARNESS_E2E_ADMISSION_TIMEOUT_SECONDS:-180}
compose_add_timeout_seconds=${HARNESS_E2E_COMPOSE_ADD_TIMEOUT_SECONDS:-600}
run_timeout_seconds=${HARNESS_E2E_RUN_TIMEOUT_SECONDS:-10800}
fixture_launcher=${HARNESS_E2E_FIXTURE_LAUNCHER:-"$repo_root/scripts/engineering_ticket_fixture.py"}
fixture_source_root=${HARNESS_E2E_FIXTURE_SOURCE_ROOT:-"$repo_root/tests/fixtures/campaign"}
engineering_fixture_revision=7a6b25b3cd12d66af74a358ae86e0d2b846bd384
shared_fixture_revision=16f6b9e05e34e09c824191eed0631d77f85be6a9

case "$artifact_dir" in
  "$repo_root"/target/*) ;;
  *) echo "HARNESS_E2E_ARTIFACTS_DIR must be below $repo_root/target" >&2; exit 2 ;;
esac
mkdir -p "$artifact_dir"
artifact_dir=$(cd "$artifact_dir" && pwd -P)
case "$artifact_dir" in
  "$repo_root"/target/*) ;;
  *) echo "artifact directory escapes the canonical target directory" >&2; exit 2 ;;
esac
contract_path="$artifact_dir/stack-lock.json"
printf '%s\n' "$HARNESS_E2E_STACK_LOCK" >"$contract_path"
python3 "$contract_tool" validate --contract "$contract_path" >/dev/null

campaign_group_id=$HARNESS_E2E_CAMPAIGN_GROUP_ID
jq -e --arg group "$campaign_group_id" \
  '.suite.groups | any(.id == $group and .execution_kind != "fault_injection")' \
  "$contract_path" >/dev/null
seed=$(jq -r '.suite.seed' "$contract_path")
execution_id=$(jq -r '.execution_id' "$contract_path")
short_execution=${execution_id%%-*}
namespace="e2e-${short_execution}-${campaign_group_id}"

run_root=$(mktemp -d "${TMPDIR:-/tmp}/harness-e2e-compose.XXXXXX")
# TMPDIR is configurable; reject an uploaded runtime/secret tree before any
# provider credential or Compose state is written into it.
if ! python3 "$contract_tool" validate-layout \
  --artifact-root "$artifact_dir" --runtime-root "$run_root" --allowed-root "$repo_root/target"; then
  rmdir -- "$run_root"
  exit 2
fi
project_dir="$run_root/project"
engine_config="$project_dir/iii.config.yaml"
compose_file="$artifact_dir/stack/worker-compose.yaml"
compose_state="$run_root/compose-state"
tools_dir="$run_root/bin"
secrets_dir="$run_root/secrets"
# The worker's native execution tree is evidence, not disposable runtime state.
# Keep it below the uploaded artifact root so a failed results-get or process
# cleanup cannot erase already committed runs and journal events.
e2e_data="$artifact_dir/native"
engine_url="ws://127.0.0.1:${engine_port}"
mkdir -p "$project_dir" "$compose_state" "$tools_dir" "$secrets_dir" "$e2e_data" \
  "$artifact_dir/logs" "$artifact_dir/stack"
chmod 700 "$secrets_dir" "$compose_state"

iii_bin="$tools_dir/iii"
engine_pid=""
compose_pid=""
compose_started=false
compose_down=false
failure_phase=bootstrap
failure_reason=""
engineering_fixture_lease=""
shared_fixture_lease=""

log() {
  printf '\n[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2
}

fail() {
  failure_reason=$1
  printf '[FAIL] %s\n' "$failure_reason" >&2
  return 1
}

capture_processes() {
  local output=$1
  ps -eo pid=,comm=,args= --no-headers | jq -Rn \
    '[inputs | capture("^\\s*(?<pid>[0-9]+)\\s+(?<comm>\\S+)\\s*(?<args>.*)$") | {pid:(.pid|tonumber),comm,args}]' \
    >"$output"
}

compose_trigger() {
  local function_id=$1
  shift
  "$iii_bin" trigger "$function_id" \
    --address 127.0.0.1 \
    --port "$engine_port" \
    --namespace "$namespace" \
    --timeout-ms 600000 \
    "$@"
}

project_trigger() {
  local function_id=$1
  local payload=$2
  local timeout_ms=${3:-30000}
  "$iii_bin" trigger "$function_id" \
    --address 127.0.0.1 \
    --port "$engine_port" \
    --namespace "$namespace" \
    --timeout-ms "$timeout_ms" \
    --json "$payload"
}

# `compose::add` may either finish synchronously or return an asynchronous
# admission receipt. A receipt is not proof that project assembly succeeded,
# so follow the typed operation until it reaches a terminal state.
await_compose_add() {
  local receipt=$1
  local snapshot=$2
  local status
  status=$(jq -r '.status // empty' "$receipt")

  case "$status" in
    ok) return 0 ;;
    accepted) ;;
    *) fail "compose::add answered '${status:-no status}'" ;;
  esac

  local operation_id deadline detail
  operation_id=$(jq -er '.operation_id | select(type == "string" and length > 0)' "$receipt")
  deadline=$((SECONDS + compose_add_timeout_seconds))
  log "compose::add admitted operation $operation_id; waiting for it to settle"

  while ((SECONDS < deadline)); do
    compose_trigger compose::operation "operation_id=$operation_id" >"$snapshot"
    status=$(jq -r '.status // empty' "$snapshot")
    case "$status" in
      succeeded) return 0 ;;
      failed | cancelled)
        detail=$(jq -r '.last_event.detail // .phase // "no detail"' "$snapshot")
        fail "compose::add $status: $detail"
        ;;
      accepted | pending | running) ;;
      *) fail "compose::operation answered '${status:-no status}'" ;;
    esac
    sleep 5
  done

  fail "compose::add did not settle within ${compose_add_timeout_seconds}s"
}

cleanup() {
  local status=$?
  local fixture_cleanup_failed=0
  trap - EXIT INT TERM ERR
  set +e
  if [[ "$compose_started" == true && "$compose_down" != true ]] && kill -0 "$compose_pid" 2>/dev/null; then
    compose_trigger compose::down "file=$compose_file" >"$artifact_dir/stack/down.json" 2>>"$artifact_dir/logs/compose-commands.log"
    compose_down=true
  fi
  [[ -f "$artifact_dir/stack/down.json" ]] || jq -n \
    --arg phase "$failure_phase" '{status:"not_reached",phase:$phase}' >"$artifact_dir/stack/down.json"
  capture_processes "$artifact_dir/stack/processes-after.json"
  if [[ -n "$compose_pid" ]] && kill -0 "$compose_pid" 2>/dev/null; then
    kill -- "-$compose_pid" 2>/dev/null || kill "$compose_pid" 2>/dev/null || true
    wait "$compose_pid" 2>/dev/null || true
  fi
  if [[ -n "$engine_pid" ]] && kill -0 "$engine_pid" 2>/dev/null; then
    kill -- "-$engine_pid" 2>/dev/null || kill "$engine_pid" 2>/dev/null || true
    wait "$engine_pid" 2>/dev/null || true
  fi
  if [[ -n "$shared_fixture_lease" ]]; then
    "$fixture_launcher" cleanup --lease-id "$shared_fixture_lease" || fixture_cleanup_failed=1
  fi
  if [[ -n "$engineering_fixture_lease" ]]; then
    "$fixture_launcher" cleanup --lease-id "$engineering_fixture_lease" || fixture_cleanup_failed=1
  fi
  if ((status == 0 && fixture_cleanup_failed != 0)); then
    status=1
    failure_phase=fixture_cleanup
    failure_reason="disposable code fixture cleanup failed"
  fi
  if ((status != 0)); then
    [[ -n "$failure_reason" ]] || failure_reason="Compose execution failed during $failure_phase (exit $status)"
    jq -n --arg phase "$failure_phase" --arg error "$failure_reason" --argjson exit_code "$status" \
      '{phase:$phase,outcome:"infra_failed",error:$error,exit_code:$exit_code}' >"$artifact_dir/failure.json"
  fi
  if [[ -f "$compose_file" && -f "$artifact_dir/stack/add.json" && -f "$artifact_dir/stack/up.json" \
        && -f "$artifact_dir/stack/status.json" && -f "$artifact_dir/stack/workers.json" \
        && -f "$artifact_dir/stack/processes-before.json" && -f "$artifact_dir/stack/processes-during.json" ]]; then
    python3 "$contract_tool" compose-evidence \
      --contract "$contract_path" \
      --compose "$compose_file" \
      --namespace "$namespace" \
      --add "$artifact_dir/stack/add.json" \
      --up "$artifact_dir/stack/up.json" \
      --status "$artifact_dir/stack/status.json" \
      --down "$artifact_dir/stack/down.json" \
      --workers "$artifact_dir/stack/workers.json" \
      --process-before "$artifact_dir/stack/processes-before.json" \
      --process-during "$artifact_dir/stack/processes-during.json" \
      --process-after "$artifact_dir/stack/processes-after.json" \
      --output "$artifact_dir/compose-evidence.json" || status=1
  fi
  rm -rf "$run_root"
  exit "$status"
}

on_error() {
  local status=$? line=$1
  [[ -n "$failure_reason" ]] || failure_reason="command failed during $failure_phase at line $line (exit $status)"
  return "$status"
}

trap 'on_error "$LINENO"' ERR
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

prepare_code_fixtures() {
  local requires_engineering requires_shared fixture_json execution_prefix
  requires_shared=$(jq -r --arg group "$campaign_group_id" '
    .suite.groups[] | select(.id == $group) |
    any(.scenarios[]?; . == "shell_coder_sandbox" or . == "chess_engine_build" or . == "trend_blog")
  ' "$contract_path")
  requires_engineering=$(jq -r --arg group "$campaign_group_id" '
    .suite.groups[] | select(.id == $group) |
    any(.scenarios[]?; . == "engineering_ticket_git_handoff")
  ' "$contract_path")
  if [[ "$requires_shared" != true && "$requires_engineering" != true ]]; then
    return 0
  fi
  [[ -x "$fixture_launcher" ]] || fail "fixture launcher is unavailable: $fixture_launcher"
  export HARNESS_E2E_ENGINEERING_FIXTURE_ROOT="$run_root/fixture-leases"
  execution_prefix="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-${campaign_group_id}"
  if [[ "$requires_engineering" == true ]]; then
    export HARNESS_E2E_ENGINEERING_FIXTURE_REPOSITORY="$fixture_source_root/engineering-ticket.bundle"
    fixture_json="$run_root/engineering-fixture.json"
    "$fixture_launcher" prepare --execution-id "${execution_prefix}-engineering" \
      --revision "$engineering_fixture_revision" >"$fixture_json"
    engineering_fixture_lease=$(jq -er .lease_id "$fixture_json")
    HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH=$(jq -er .path "$fixture_json")
    export HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH
  fi
  if [[ "$requires_shared" == true ]]; then
    export HARNESS_E2E_ENGINEERING_FIXTURE_REPOSITORY="$fixture_source_root/shared-fixture.bundle"
    fixture_json="$run_root/shared-fixture.json"
    "$fixture_launcher" prepare --execution-id "${execution_prefix}-shared" \
      --revision "$shared_fixture_revision" >"$fixture_json"
    shared_fixture_lease=$(jq -er .lease_id "$fixture_json")
    HARNESS_E2E_FIXTURE_PATH=$(jq -er .path "$fixture_json")
    export HARNESS_E2E_FIXTURE_PATH
  fi
}

write_provider_secret() {
  local worker=$1 variable=$2 value=${!2:-}
  jq -e --arg worker "$worker" \
    '.orchestration.roots | any(.worker == $worker)' "$contract_path" >/dev/null || return 0
  [[ -n "$value" ]] || fail "$variable is required by orchestrated worker $worker"
  printf '%s=%s\n' "$variable" "$value" >"$secrets_dir/$worker.env"
  chmod 600 "$secrets_dir/$worker.env"
}

wait_for_engine() {
  local response
  for ((attempt = 0; attempt < wait_seconds; attempt++)); do
    kill -0 "$engine_pid" 2>/dev/null || fail "iii engine exited before becoming ready"
    response=$("$iii_bin" trigger engine::workers::list --address 127.0.0.1 --port "$engine_port" --json '{}' 2>/dev/null || true)
    jq -e '.workers != null' <<<"$response" >/dev/null 2>&1 && return 0
    sleep 1
  done
  fail "iii engine did not become ready within ${wait_seconds}s"
}

wait_for_compose() {
  for ((attempt = 0; attempt < wait_seconds; attempt++)); do
    kill -0 "$compose_pid" 2>/dev/null || fail "iii compose exited before becoming ready"
    if compose_trigger compose::list --json '{}' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  fail "iii compose did not become ready within ${wait_seconds}s"
}

failure_phase=fixture_setup
prepare_code_fixtures
write_provider_secret provider-deepseek DEEPSEEK_API_KEY
write_provider_secret provider-zai ZAI_API_KEY

capture_processes "$artifact_dir/stack/processes-before.json"

failure_phase=cli_install
cli_version=$(jq -r '.runtime.cli.version' "$contract_path")
cli_asset=$(jq -r '.runtime.cli.asset' "$contract_path")
cli_sha=$(jq -r '.runtime.cli.sha256' "$contract_path" | sed 's/^sha256://')
cli_archive="$run_root/$cli_asset"
cli_url="https://github.com/iii-hq/iii/releases/download/iii/v${cli_version}/${cli_asset}"
log "Downloading exact iii CLI $cli_version"
curl -fsSL --retry 3 --retry-all-errors --retry-delay 5 "$cli_url" -o "$cli_archive"
printf '%s  %s\n' "$cli_sha" "$cli_archive" | sha256sum --check --status
tar -xzf "$cli_archive" -C "$tools_dir"
chmod +x "$iii_bin"
observed_cli_version=$("$iii_bin" --version 2>&1)
printf '%s\n' "$observed_cli_version" >"$artifact_dir/iii-version.txt"
[[ "$observed_cli_version" == *"$cli_version"* ]] || fail \
  "iii CLI version mismatch: expected $cli_version, observed $observed_cli_version"
forbidden_name="iii""-worker"
if find "$tools_dir" -type f -name "$forbidden_name" -print -quit | grep -q .; then
  fail "forbidden lifecycle helper was installed"
fi

project_args=(
  --contract "$contract_path"
  --namespace "$namespace"
  --data-dir "$e2e_data"
  --environment "harness-e2e.HARNESS_E2E_RUN_DIR=$project_dir"
  --environment "harness-e2e.HARNESS_E2E_LANE=$(jq -r '.suite.lane' "$contract_path")"
  --environment "harness-e2e.HARNESS_E2E_CAMPAIGN_GROUP=$campaign_group_id"
  --output "$compose_file"
)
for secret_file in "$secrets_dir"/*.env; do
  [[ -f "$secret_file" ]] || continue
  project_args+=(--env-file "$(basename "$secret_file" .env)=$secret_file")
done
if [[ -n "${HARNESS_E2E_FIXTURE_PATH:-}" ]]; then
  project_args+=(--environment "harness-e2e.HARNESS_E2E_FIXTURE_PATH=$HARNESS_E2E_FIXTURE_PATH")
fi
if [[ -n "${HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH:-}" ]]; then
  project_args+=(--environment \
    "harness-e2e.HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH=$HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH")
fi
python3 "$contract_tool" project "${project_args[@]}"

failure_phase=engine_start
manager_name="iii""-worker-manager"
printf 'workers:\n  - name: %s\n    config:\n      host: 127.0.0.1\n      port: %s\n' \
  "$manager_name" "$engine_port" >"$engine_config"
(cd "$project_dir" && exec setsid "$iii_bin" -c "$engine_config" --no-update-check) \
  >"$artifact_dir/logs/engine.log" 2>&1 &
engine_pid=$!
wait_for_engine

failure_phase=compose_start
III_COMPOSE_STATE_DIR="$compose_state" setsid "$iii_bin" compose \
  --engine "$engine_url" --namespace "$namespace" \
  >"$artifact_dir/logs/compose.log" 2>&1 &
compose_pid=$!
compose_started=true
wait_for_compose

failure_phase=project_assembly
add_args=("file=$compose_file")
while IFS= read -r root; do
  add_args+=("worker=$root")
done < <(python3 "$contract_tool" roots --contract "$contract_path")
compose_trigger compose::add "${add_args[@]}" >"$artifact_dir/stack/add.json"
await_compose_add "$artifact_dir/stack/add.json" "$artifact_dir/stack/add-operation.json"

failure_phase=project_start
compose_trigger compose::up "file=$compose_file" >"$artifact_dir/stack/up.json"
jq -e '.status == "ok"' "$artifact_dir/stack/up.json" >/dev/null
compose_trigger compose::status "file=$compose_file" >"$artifact_dir/stack/status.json"
"$iii_bin" trigger engine::workers::list --address 127.0.0.1 --port "$engine_port" --json '{}' \
  >"$artifact_dir/stack/workers.json"
capture_processes "$artifact_dir/stack/processes-during.json"

failure_phase=materialization
project_trigger e2e::scenarios-list "$(jq -cn --argjson seed "$seed" '{seed:$seed}')" 120000 \
  >"$artifact_dir/catalog.json"
python3 "$contract_tool" materialize \
  --contract "$contract_path" \
  --catalog "$artifact_dir/catalog.json" \
  --output "$artifact_dir/run-request.json" \
  --group-id "$campaign_group_id"

failure_phase=execution
project_trigger e2e::run "$(jq -c . "$artifact_dir/run-request.json")" "$((admission_timeout_seconds * 1000))" \
  >"$artifact_dir/accepted.json"
remote_execution_id=$(jq -er '.execution_id | select(type == "string" and length > 0)' "$artifact_dir/accepted.json")
printf '%s\n' "$remote_execution_id" >"$artifact_dir/remote-execution-id.txt"

started_at=$SECONDS
poll_index=0
while true; do
  project_trigger e2e::status \
    "$(jq -cn --arg execution_id "$remote_execution_id" '{execution_id:$execution_id}')" 60000 \
    >"$artifact_dir/status.json"
  jq -e --arg id "$remote_execution_id" '.execution_id == $id' "$artifact_dir/status.json" >/dev/null
  [[ "$(jq -r '.terminal // false' "$artifact_dir/status.json")" == true ]] && break
  if ((SECONDS - started_at >= run_timeout_seconds)); then
    fail "E2E execution exceeded ${run_timeout_seconds}s"
  fi
  case "$poll_index" in 0) delay=2 ;; 1) delay=5 ;; 2) delay=10 ;; *) delay=30 ;; esac
  poll_index=$((poll_index + 1))
  sleep "$delay"
done

failure_phase=results
results_response="$artifact_dir/results-get.json"
project_trigger e2e::results-get \
  "$(jq -cn --arg execution_id "$remote_execution_id" '{execution_id:$execution_id}')" 120000 \
  >"$results_response"
jq -e --arg id "$remote_execution_id" '.execution_id == $id' "$results_response" >/dev/null
native_result_path=$(jq -er '.result_path | select(type == "string" and length > 0)' "$results_response")
case "$native_result_path" in
  /*|*".."*) fail "unsafe native result path: $native_result_path" ;;
esac
native_dir="$e2e_data/$(dirname -- "$native_result_path")"
for native_name in results.json manifest.json observation.json; do
  test -f "$native_dir/$native_name"
  cp -- "$native_dir/$native_name" "$artifact_dir/$native_name"
done
expected_results_sha=$(jq -er '.observation.evidence.results_sha256' "$results_response")
expected_manifest_sha=$(jq -er '.observation.evidence.manifest_sha256' "$results_response")
observed_results_sha="sha256:$(sha256sum "$artifact_dir/results.json" | cut -d ' ' -f1)"
observed_manifest_sha="sha256:$(sha256sum "$artifact_dir/manifest.json" | cut -d ' ' -f1)"
[[ "$observed_results_sha" == "$expected_results_sha" ]]
[[ "$observed_manifest_sha" == "$expected_manifest_sha" ]]

if project_trigger e2e::archive \
  "$(jq -cn --arg execution_id "$remote_execution_id" '{execution_id:$execution_id,retention_class:"longitudinal"}')" 120000 \
  >"$artifact_dir/archive.json" 2>"$artifact_dir/logs/archive.log"; then
  project_trigger e2e::archive-head \
    "$(jq -cn --arg execution_id "$remote_execution_id" '{execution_id:$execution_id}')" 120000 \
    >"$artifact_dir/archive-head.json" 2>>"$artifact_dir/logs/archive.log" || true
fi

failure_phase=compose_down
compose_trigger compose::down "file=$compose_file" >"$artifact_dir/stack/down.json"
compose_down=true
capture_processes "$artifact_dir/stack/processes-after.json"
failure_phase=complete
