#!/usr/bin/env bash
set -Eeuo pipefail
export III_TELEMETRY_ENABLED=false

: "${HARNESS_E2E_CONTRACT:?HARNESS_E2E_CONTRACT (the campaign contract file) is required}"
: "${HARNESS_E2E_CAMPAIGN_GROUP_ID:?HARNESS_E2E_CAMPAIGN_GROUP_ID is required}"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
launcher="$script_dir/${BASH_SOURCE[0]##*/}"
repo_root=$(cd -- "$script_dir/.." && pwd)
contract_tool="$repo_root/scripts/exact_stack_campaign.py"
# Set by the execution's preparation: assemble the stack once through
# compose::add, export only its manifest and worker-compose.lock to the
# artifact directory, and stop there. Groups then start that stack frozen.
assemble_only=${HARNESS_E2E_ASSEMBLE_ONLY:-}
artifact_dir=${HARNESS_E2E_ARTIFACTS_DIR:-"$repo_root/target/harness-e2e-shadow"}
engine_port=${HARNESS_E2E_ENGINE_PORT:-49134}
# How long the engine, Compose and the runner each have to become ready, in
# seconds of wall clock. Each readiness probe answers within probe_ms or
# counts as not ready yet, so a probe that hangs never stretches the wait.
wait_seconds=${HARNESS_E2E_WAIT_SECONDS:-300}
probe_ms=5000
# A group that fails before any run started is tried once more, this long
# after the first attempt; HARNESS_E2E_GROUP_ATTEMPT is the launcher's own.
retry_delay_seconds=60
group_attempt=${HARNESS_E2E_GROUP_ATTEMPT:-1}
admission_timeout_seconds=${HARNESS_E2E_ADMISSION_TIMEOUT_SECONDS:-180}
compose_add_timeout_seconds=${HARNESS_E2E_COMPOSE_ADD_TIMEOUT_SECONDS:-600}
run_timeout_seconds=${HARNESS_E2E_RUN_TIMEOUT_SECONDS:-10800}
kanban_bootstrap=${HARNESS_E2E_KANBAN_BOOTSTRAP:-"$repo_root/scripts/kanban_eval/bootstrap.py"}

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
# A file, not a variable: the contract carries the assembled stack and its
# lock, which outgrow what one environment string may hold.
cp "$HARNESS_E2E_CONTRACT" "$contract_path"
python3 "$contract_tool" validate --contract "$contract_path" >/dev/null

campaign_group_id=$HARNESS_E2E_CAMPAIGN_GROUP_ID
jq -e --arg group "$campaign_group_id" \
  '.suite.groups | any(.id == $group)' \
  "$contract_path" >/dev/null
project_template=$(jq -r '.runtime.template.id // empty' "$contract_path")
if [[ -n "$assemble_only" ]]; then
  project_template=""
fi
# A stack the execution already assembled starts from its lock; a template
# project is assembled here, pinned to the versions that lock resolved.
frozen=false
if [[ -z "$project_template" ]] && jq -e '.runtime.lock != null' "$contract_path" >/dev/null; then
  frozen=true
fi
# The Directory serves the project's own agents and skills when the suite
# names an agent profile or the template ships some (the template scaffold
# says). Otherwise it runs as the stack declares it and downloads the skills of
# every worker installed, those the subject adds too (Linkly).
profile_assets=$(jq -r '.suite.agent_profile != null' "$contract_path")
seed=$(jq -r '.suite.seed' "$contract_path")
execution_id=$(jq -r '.execution_id' "$contract_path")
short_execution=${execution_id%%-*}
# Compose derives every configuration id it creates itself (dependency
# containers it adds) as `<namespace>-<container>` and refuses anything over
# 64 characters, as does the engine. The longest worker name it adds today is
# 21 characters (`provider-openai-codex`), so the namespace stays within 40:
# the bare group id when it fits, else a stable prefix plus a digest.
namespace="e2e-${short_execution}-${campaign_group_id#case-}"
if (( ${#namespace} > 40 )); then
  namespace="${namespace:0:33}-$(printf '%s' "$campaign_group_id" | sha256sum | cut -c1-6)"
fi

run_root=$(mktemp -d "${TMPDIR:-/tmp}/harness-e2e-compose.XXXXXX")
project_dir="$run_root/project"
evaluation_dir="$run_root/evaluation"
engine_config="$project_dir/iii.config.yaml"
# Compose injects the manifest's parent as III_COMPOSE_DIR and uses it as
# package containers' default cwd. Moving only daemon state is insufficient:
# data/secrets and other project-relative runtime would still be uploaded.
compose_file="$project_dir/worker-compose.yaml"
compose_working_dir="$project_dir"
# TMPDIR is configurable; check the actual manifest as well as both roots
# before any provider credential or Compose state is written into them.
if ! python3 "$contract_tool" validate-layout \
  --artifact-root "$artifact_dir" --runtime-root "$run_root" --allowed-root "$repo_root/target" \
  --compose "$compose_file"; then
  rmdir -- "$run_root"
  exit 2
fi
compose_state="$run_root/compose-state"
tools_dir="$run_root/bin"
env_file="$project_dir/.env"
# The worker's native execution tree is evidence, not disposable runtime state.
# Keep it below the uploaded artifact root so a failed results-get or process
# cleanup cannot erase already committed runs and journal events.
e2e_data="$artifact_dir/native"
engine_url="ws://127.0.0.1:${engine_port}"
mkdir -p "$project_dir" "$compose_state" "$tools_dir" "$e2e_data" \
  "$evaluation_dir" "$artifact_dir/logs" "$artifact_dir/stack"
chmod 700 "$compose_state"
# A second attempt says so where a reader of the group looks first: the
# attempt, and why the first one failed (its failure.json, under attempts/1).
if ((group_attempt > 1)); then
  { cat "$artifact_dir/attempts/1/failure.json" 2>/dev/null || true; } \
    | jq -s --argjson attempt "$group_attempt" '{attempt: $attempt, earlier_attempts: .}' \
    >"$artifact_dir/stack/attempt.json" \
    || jq -n --argjson attempt "$group_attempt" '{attempt: $attempt}' >"$artifact_dir/stack/attempt.json"
fi

iii_bin="$tools_dir/iii"
# The stderr of the last `iii trigger`, and the function it called when that
# call failed: what iii answered is the cause a failure record gives.
iii_stderr="$run_root/iii-stderr.log"
iii_failed="$run_root/iii-failed-function"
engine_pid=""
compose_pid=""
compose_started=false
compose_down=false
run_started=false
failure_phase=cli_install
failure_reason=""
failure_line=""

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
  ps -eo pid=,ppid=,comm=,args= --no-headers | jq -Rn \
    '[inputs | capture("^\\s*(?<pid>[0-9]+)\\s+(?<ppid>[0-9]+)\\s+(?<comm>\\S+)\\s*(?<args>.*)$") | {pid:(.pid|tonumber),ppid:(.ppid|tonumber),comm,args}]' \
    >"$output"
}

# iii_trigger TIMEOUT_MS FUNCTION [ARGS...]: one call into the engine. A CLI
# that hangs past its own timeout is stopped a second later; in the
# foreground, so an interrupt reaches it as it reaches the launcher. Its
# stderr is passed on as it came, after the call; when the call fails, the
# function stays named in $iii_failed until a later call succeeds or the
# launcher tolerates the failure.
iii_trigger() {
  local timeout_ms=$1 function_id=$2 status=0
  shift 2
  timeout --foreground -k 1 $((timeout_ms / 1000 + 1)) "$iii_bin" trigger "$function_id" \
    --engine "$engine_url" --timeout-ms "$timeout_ms" "$@" 2>"$iii_stderr" || status=$?
  cat "$iii_stderr" >&2
  if ((status == 0)); then
    rm -f "$iii_failed"
  else
    printf '%s' "$function_id" >"$iii_failed"
  fi
  return "$status"
}

compose_trigger() {
  local function_id=$1
  shift
  iii_trigger 600000 "$function_id" --namespace "$namespace" "$@"
}

start_compose() {
  local payload=$1 attempt receipt error_log status delay
  for attempt in 1 2 3; do
    receipt="$artifact_dir/stack/up-attempt-$attempt.json"
    error_log="$artifact_dir/logs/compose-up-attempt-$attempt.log"
    if compose_trigger compose::up --json "$payload" >"$receipt" 2>"$error_log"; then
      cat "$error_log" >&2
      cp "$receipt" "$artifact_dir/stack/up.json"
      return 0
    else
      status=$?
    fi
    cat "$error_log" >&2
    cp "$receipt" "$artifact_dir/stack/up.json"
    if ! grep -q 'PACKAGE_DOWNLOAD_FAILED' "$error_log" ||
       ! grep -Eqi 'HTTP([^0-9]|status)*(429|5[0-9]{2})\b|status([^0-9])*(429|5[0-9]{2})\b|timed?[ -]?out|timeout|connection (reset|refused|closed|aborted)|error sending request|dns error|tcp connect error' "$error_log"; then
      failure_reason="compose::up failed on attempt $attempt; see logs/compose-up-attempt-$attempt.log"
      return "$status"
    fi
    if ((attempt == 3)); then
      failure_reason="compose::up exhausted 3 attempts after transient package download failures; see logs/compose-up-attempt-3.log"
      return "$status"
    fi
    delay=$((attempt * 5))
    log "compose::up package download failed transiently on attempt $attempt/3; retrying in ${delay}s"
    sleep "$delay"
  done
}

project_trigger() {
  local function_id=$1
  local payload=$2
  local timeout_ms=${3:-30000}
  iii_trigger "$timeout_ms" "$function_id" --namespace "$namespace" --json "$payload"
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

export_compose() {
  python3 "$contract_tool" export-compose \
    --compose "$compose_file" --runtime-root "$run_root" \
    --artifact-root "$artifact_dir" --allowed-root "$repo_root/target" "$@"
}

cleanup() {
  local status=$?
  local exited=$status
  trap - EXIT INT TERM ERR
  set +e
  # The call that failed, before taking the stack down makes others.
  local failed_call=""
  if [[ -f "$iii_failed" ]]; then
    failed_call=$(<"$iii_failed")
    mv -f "$iii_stderr" "$run_root/failed-call.log"
  fi
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
  if ((status != 0)); then
    # The phase, and what iii answered when one of its calls failed: its code
    # and the start of its message, not the line of this script.
    local record=(--output "$artifact_dir/failure.json" --phase "$failure_phase" --exit-code "$status"
      --attempt "$group_attempt")
    [[ -z "$failure_reason" ]] || record+=(--reason "$failure_reason")
    [[ -z "$failure_line" ]] || record+=(--line "$failure_line")
    [[ -z "$failed_call" ]] || record+=(--iii-function "$failed_call" --iii-stderr "$run_root/failed-call.log")
    [[ ! -f "$artifact_dir/attempts/1/failure.json" ]] || record+=(--earlier "$artifact_dir/attempts/1/failure.json")
    python3 "$contract_tool" failure "${record[@]}"
  fi
  # Export named regular files, never the runtime tree. Keep the pre-up
  # manifest immutable for compose-evidence; a final snapshot records any
  # later changes, and the adjacent lock is retained byte for byte.
  export_compose --final || status=1
  if [[ -f "$compose_file" && -f "$artifact_dir/stack/add.json" && -f "$artifact_dir/stack/up.json" \
        && -f "$artifact_dir/stack/status.json" && -f "$artifact_dir/stack/workers.json" \
        && -f "$artifact_dir/stack/processes-before.json" && -f "$artifact_dir/stack/processes-during.json" ]]; then
    python3 "$contract_tool" compose-evidence \
      --contract "$contract_path" \
      --compose "$artifact_dir/stack/worker-compose.yaml" \
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
  local clean=true
  stop_leftovers || clean=false
  rm -rf "$run_root"
  # Before e2e::run nothing of the subject has run: a passing fault of the
  # stack (GitHub answering 500 for an artifact, an engine slow to come up)
  # gets the group one more attempt from a clean start, a minute later. Once
  # e2e::run was called, never: the subject's result is not drawn again.
  # Never after an interruption, never with anything of this attempt still
  # running, and not for preparation, which assembles with tries of its own
  # (scripts/executor.sh). The pause comes first: interrupted during it, the
  # group ends with this attempt's evidence where it is. Then that evidence
  # moves under attempts/1.
  if ((exited != 0 && exited != 130 && exited != 143 && group_attempt == 1)) \
    && [[ "$run_started" == false && -z "$assemble_only" ]]; then
    if [[ "$clean" != true ]]; then
      printf '::warning::%s failed during %s and left processes or containers it could not stop; not trying it again\n' \
        "$campaign_group_id" "$failure_phase" >&2
    else
      printf '::warning::%s failed during %s before any run started; trying the group once more in %ss\n' \
        "$campaign_group_id" "$failure_phase" "$retry_delay_seconds" >&2
      sleep "$retry_delay_seconds"
      mkdir -p "$artifact_dir/attempts/1"
      find "$artifact_dir" -mindepth 1 -maxdepth 1 ! -name attempts -exec mv -t "$artifact_dir/attempts/1" -- {} +
      exec env -u HARNESS_E2E_KANBAN_RUNTIME HARNESS_E2E_GROUP_ATTEMPT=2 bash "$launcher"
    fi
  fi
  exit "$status"
}

# The processes of this attempt, by their working directory or command line
# under the private runtime root. awk reads the root from its environment so
# that its own command line does not name it.
runtime_processes() {
  {
    find /proc -mindepth 2 -maxdepth 2 -name cwd \( -lname "$run_root" -o -lname "$run_root/*" \) 2>/dev/null \
      | cut -d/ -f3
    ps -eo pid=,args= | RUNTIME_ROOT=$run_root/ awk 'index($0, ENVIRON["RUNTIME_ROOT"]) {print $1}'
  } | sort -un
}

# What the attempt started and outlived it. Compose starts each worker in a
# process group of its own, without a parent-death signal, so killing the
# engine's and Compose's groups can leave workers running that reconnect to
# whatever engine listens next on the port, under the same namespace. Each
# process of the runtime root is killed, and every container when the Docker
# daemon is the group's own (HARNESS_E2E_OWN_DOCKER, set by
# scripts/executor.sh). Fails when something is still there.
stop_leftovers() {
  local pids containers=""
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    pids=$(runtime_processes)
    if [[ "${HARNESS_E2E_OWN_DOCKER:-}" == 1 ]]; then
      containers=$(docker ps -aq) || return 1
    fi
    [[ -n "$pids$containers" ]] || return 0
    # shellcheck disable=SC2086 # one pid or container id per word
    { [[ -z "$pids" ]] || kill -KILL $pids; [[ -z "$containers" ]] || docker rm -f $containers; } >/dev/null 2>&1
    sleep 0.5
  done
  return 1
}

on_error() {
  local status=$? line=$1
  [[ -n "$failure_line" ]] || failure_line=$line
  return "$status"
}

trap 'on_error "$LINENO"' ERR
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A process that exits before it is ready says why in its own log, not in the
# probe that then found it gone: the cause is its last error line (its last
# line when none reads as an error), and the probe is not recorded as the call
# that failed.
exited() {
  local said
  said=$(grep -E '^(error|Error)\b|\[(ERROR|FATAL)\]|panicked' "$2" 2>/dev/null | tail -n 1 || true)
  [[ -n "$said" ]] || said=$(grep -v '^[[:space:]]*$' "$2" 2>/dev/null | tail -n 1 || true)
  rm -f "$iii_failed"
  fail "$1 exited before becoming ready${said:+: ${said:0:300}}"
}

wait_for_engine() {
  local response started=$SECONDS
  while ((SECONDS - started < wait_seconds)); do
    kill -0 "$engine_pid" 2>/dev/null || exited "iii engine" "$artifact_dir/logs/engine.log"
    response=$(iii_trigger "$probe_ms" engine::workers::list --json '{}' 2>/dev/null || true)
    jq -e '.workers != null' <<<"$response" >/dev/null 2>&1 && return 0
    sleep 1
  done
  fail "iii engine did not become ready within ${wait_seconds}s (gave up after $((SECONDS - started))s)"
}

wait_for_compose() {
  local started=$SECONDS
  while ((SECONDS - started < wait_seconds)); do
    kill -0 "$compose_pid" 2>/dev/null || exited "iii compose" "$artifact_dir/logs/compose.log"
    if iii_trigger "$probe_ms" compose::list --namespace "$namespace" --json '{}' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  fail "iii compose did not become ready within ${wait_seconds}s (gave up after $((SECONDS - started))s)"
}

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
# What Compose boots a VM worker with, from the same release and beside iii,
# as the installer puts them. Only Compose runs them.
while IFS=$'\t' read -r companion companion_sha; do
  curl -fsSL --retry 3 --retry-all-errors --retry-delay 5 "${cli_url%/*}/$companion" -o "$run_root/$companion"
  printf '%s  %s\n' "${companion_sha#sha256:}" "$run_root/$companion" | sha256sum --check --status
  tar -xzf "$run_root/$companion" -C "$tools_dir"
done < <(jq -r '.runtime.cli.companions[]? | [.asset, .sha256] | @tsv' "$contract_path")
chmod +x "$iii_bin"
export PATH="$tools_dir:$PATH"
observed_cli_version=$("$iii_bin" --version 2>&1)
printf '%s\n' "$observed_cli_version" >"$artifact_dir/iii-version.txt"
[[ "$observed_cli_version" == *"$cli_version"* ]] || fail \
  "iii CLI version mismatch: expected $cli_version, observed $observed_cli_version"

if [[ "$campaign_group_id" == case-kanban-* ]] && [[ -z "$assemble_only" ]]; then
  fixture_root=${HARNESS_E2E_KANBAN_FIXTURE_ROOT:-"$repo_root/target/kanban-fixture"}
  [[ -f "$kanban_bootstrap" ]] || fail "Kanban bootstrap is unavailable: $kanban_bootstrap"
  [[ -d "$fixture_root/.git" ]] || fail "Kanban fixture checkout is unavailable: $fixture_root"
  kanban_runtime="$run_root/kanban-runtime.json"
  failure_phase=kanban_bootstrap
  python3 "$kanban_bootstrap" \
    --fixture "$fixture_root" --iii "$iii_bin" --runtime-root "$run_root/kanban-runtime" \
    --output "$kanban_runtime" --node "$(realpath "$(command -v node)")" \
    --npm "$(realpath "$(command -v npm)")"
  jq -e 'keys == ["browser-dependencies","browsers","dependencies","fixture","iii","image","node","playwright-module","pnpm"]' \
    "$kanban_runtime" >/dev/null
  export HARNESS_E2E_KANBAN_RUNTIME="$kanban_runtime"
fi

if [[ -n "$project_template" ]]; then
  failure_phase=template_scaffold
  template_root="$repo_root/target/execution-template"
  template_revision=$(jq -er '.runtime.template.revision' "$contract_path")
  [[ "$(git -C "$template_root" rev-parse HEAD)" == "$template_revision" ]] || fail "Execution template revision mismatch"
  "$iii_bin" project init --directory "$project_dir" --template "$project_template" \
    --template-dir "$template_root/iii" --skip-iii >"$artifact_dir/logs/template-scaffold.log" 2>&1
  jq '.runtime.template' "$contract_path" >"$artifact_dir/stack/template.json"
  mkdir -p "$run_root/template-assets"
  for folder in agents skills; do
    if [[ -d "$project_dir/$folder" ]]; then
      cp -R "$project_dir/$folder" "$run_root/template-assets/$folder"
      profile_assets=true
    fi
  done
fi

# One env file for the whole project, written after any template scaffold so
# it replaces the placeholder the template ships: every credential the group
# received (those HARNESS_E2E_CREDENTIALS names, which run_in_image.sh sets
# from its --env-file, and DEEPSEEK_API_KEY, ZAI_API_KEY and TYPESAFE_API_KEY
# from the environment), nothing else of it. The model's provider without its
# key is said out loud and the group still runs.
: >"$env_file"
chmod 600 "$env_file"
python3 "$contract_tool" credentials-env --contract "$contract_path" --output "$env_file"

# A declared path://./ worker's source sits beside the original manifest; a
# second attempt finds it where the first attempt's stack/ was kept.
local_source_root="$artifact_dir/stack"
if ((group_attempt > 1)) && [[ -d "$artifact_dir/attempts/1/stack" ]]; then
  local_source_root="$artifact_dir/attempts/1/stack"
fi
project_args=(
  --contract "$contract_path"
  --env-file "$env_file"
  --namespace "$namespace"
  --data-dir "$e2e_data"
  --environment "harness-e2e.HARNESS_E2E_RUN_DIR=$evaluation_dir"
  --environment "harness-e2e.HARNESS_E2E_LANE=$(jq -r '.suite.lane' "$contract_path")"
  --environment "harness-e2e.HARNESS_E2E_CAMPAIGN_GROUP=$campaign_group_id"
  --output "$compose_file"
  --local-source-root "$local_source_root"
  --engine-config "$engine_config"
  --engine-port "$engine_port"
)
# The runner replaces their values in every artifact before it hashes it,
# the names the catalog lists and these alike.
credential_names=$(cut -d= -f1 "$env_file" | paste -sd' ' -)
if [[ -n "$credential_names" ]]; then
  project_args+=(--environment "harness-e2e.HARNESS_E2E_CREDENTIALS=$credential_names")
fi
# Without a group the scaffold is the stack the whole suite shares.
if [[ -n "$assemble_only" ]]; then
  project_args+=(--assemble)
else
  project_args+=(--group-id "$campaign_group_id")
fi
if [[ -n "$project_template" ]]; then
  project_args+=(--template-compose "$project_dir/worker-compose.yaml"
    --template-package shell=ide --template-package console=ade)
fi
# The Directory's configuration is per group; the assembled stack has none.
# It also serves this repository's own profiles (config/agents: null-agent,
# the null-canary subject); a template file with the same id wins.
if [[ "$profile_assets" == true && -z "$assemble_only" ]]; then
  mkdir -p "$project_dir/agents"
  for profile in "$repo_root"/config/agents/*.md; do
    [[ -e "$project_dir/agents/${profile##*/}" ]] || cp "$profile" "$project_dir/agents/"
  done
  project_args+=(--profile-root "$project_dir")
fi
if [[ -n "${HARNESS_E2E_KANBAN_RUNTIME:-}" ]]; then
  project_args+=(--environment "harness-e2e.HARNESS_E2E_KANBAN_RUNTIME=$HARNESS_E2E_KANBAN_RUNTIME")
fi
# A subscription provider (openai-codex, claude-code) signs in with the
# access token the group received and nothing else, never a refresh or id
# token: its login is written from CODEX_ACCESS_TOKEN or
# CLAUDE_CODE_ACCESS_TOKEN into the runtime tree, the provider alone is
# pointed at it, and stack/credentials.json says when it expires, without it.
# A token already dead fails the group here; a missing one is a warning.
if [[ -z "$assemble_only" ]]; then
  login=$(python3 "$contract_tool" subscription-login --contract "$contract_path" \
    --root "$run_root/login" --evidence "$artifact_dir/stack/credentials.json") \
    || { failure_phase=credentials; fail "${login#error: }"; }
  [[ -z "$login" ]] || project_args+=(--environment "$login")
fi
python3 "$contract_tool" project "${project_args[@]}"

failure_phase=engine_start
(cd "$project_dir" && exec setsid "$iii_bin" -c "$engine_config" --no-update-check) \
  >"$artifact_dir/logs/engine.log" 2>&1 &
engine_pid=$!
wait_for_engine

failure_phase=compose_start
(cd "$compose_working_dir" && exec env III_COMPOSE_STATE_DIR="$compose_state" setsid "$iii_bin" compose \
  --engine "$engine_url" --namespace "$namespace") \
  >"$artifact_dir/logs/compose.log" 2>&1 &
compose_pid=$!
compose_started=true
wait_for_compose

failure_phase=project_assembly
if [[ "$frozen" == true ]]; then
  jq -n '{status:"skipped",reason:"the execution assembled this stack once; the group starts it from its worker-compose.lock"}' \
    >"$artifact_dir/stack/add.json"
else
  # Without a template every declared worker is asked for and the engine
  # expands each. With one, the template's roles must not be passed — renamed
  # ones would expand into duplicate packages — so only the runner is: it is
  # this repository's addition, and the engine installs what it needs with it.
  add_args=("file=$compose_file")
  if [[ -n "$project_template" ]]; then
    # A runner built from a commit is declared with its dependencies, and
    # then there is nothing to ask for: compose::up starts the project.
    runner_root=$(python3 "$contract_tool" roots --compose "$compose_file" | grep '^harness-e2e@' || true)
    [[ -z "$runner_root" ]] || add_args+=("worker=$runner_root")
  else
    # Except what the executor only needs to exist: the model's provider and
    # the Directory. Harness's graph brings them pinned, and asking for one as
    # well is a second, conflicting spec. Declared, one keeps its pin unasked.
    # So do the dependencies of a worker built from a commit, declared at the
    # versions of its newest release.
    held=" $(jq -r '[.runtime.commits // {} | .[].dependencies[]?] | join(" ")' "$contract_path") "
    # A dependency the stack pins itself (another declared worker starts
    # after it) is held too: asked for, it is a second spec that conflicts
    # with the version the graph that brings it resolves today.
    pinned=" $(python3 "$contract_tool" roots --compose "$compose_file" --dependencies | paste -sd' ' -) "
    ensured=" provider-$(jq -r '.suite.subject.provider' "$contract_path") iii-directory $held $pinned"
    roots=$(python3 "$contract_tool" roots --compose "$compose_file")
    while IFS= read -r root; do
      [[ -z "$root" || "$ensured" == *" ${root%@*} "* ]] || add_args+=("worker=$root")
    done <<<"$roots"
    # Every root built from a commit: its held dependencies are asked for at
    # the exact versions one release graph resolved together.
    if ((${#add_args[@]} == 1)); then
      while IFS= read -r root; do
        [[ -z "$root" || "$held" != *" ${root%@*} "* ]] || add_args+=("worker=$root")
      done <<<"$roots"
    fi
    ((${#add_args[@]} > 1)) || fail "the stack declares no package worker for compose::add to assemble"
  fi
  if ((${#add_args[@]} > 1)) || [[ -z "$project_template" ]]; then
    compose_trigger compose::add "${add_args[@]}" >"$artifact_dir/stack/add.json"
    await_compose_add "$artifact_dir/stack/add.json" "$artifact_dir/stack/add-operation.json"
  else
    jq -n '{status:"skipped",reason:"the runner is built from a commit and declared; compose::up starts the project"}' \
      >"$artifact_dir/stack/add.json"
  fi
fi
if [[ -n "$assemble_only" ]]; then
  # Ask on its own for whichever of those no graph brought.
  needed=("provider-$(jq -r '.suite.subject.provider' "$contract_path")")
  [[ "$profile_assets" != true ]] || needed+=(iii-directory)
  for worker in "${needed[@]}"; do
    if python3 "$contract_tool" roots --compose "$compose_file" | grep -q "^$worker@"; then
      continue
    fi
    compose_trigger compose::add "file=$compose_file" "worker=$worker" >"$artifact_dir/stack/add-$worker.json"
    await_compose_add "$artifact_dir/stack/add-$worker.json" "$artifact_dir/stack/add-$worker-operation.json"
  done
  # compose::add expanded every root into its graph and wrote the lock beside
  # the file; the exit handler takes the project down.
  log "Assembled $compose_file and its worker-compose.lock"
  failure_phase=complete
  exit 0
fi

export_compose

python3 "$contract_tool" roots --compose "$compose_file" \
  | jq -Rc 'split("@") | {worker: .[0], version: .[1]}' \
  | jq -sc '.' >"$artifact_dir/stack/declared-workers.json"

failure_phase=project_start
up_request=$(jq -cn --arg file "$compose_file" --argjson frozen "$frozen" '{file:$file,frozen:$frozen}')
start_compose "$up_request"
jq -e '.status == "ok"' "$artifact_dir/stack/up.json" >/dev/null
compose_trigger compose::status "file=$compose_file" >"$artifact_dir/stack/status.json"
iii_trigger 30000 engine::workers::list --json '{}' >"$artifact_dir/stack/workers.json"
capture_processes "$artifact_dir/stack/processes-during.json"

if [[ "$profile_assets" == true ]]; then
  failure_phase=profile_assets
  mkdir -p "$artifact_dir/stack/skills"
  while IFS= read -r package; do
    worker=$(jq -r '.worker' <<<"$package")
    request=$(jq -c '{worker,version}' <<<"$package")
    receipt="$artifact_dir/stack/skills/$worker.json"
    error_log="$artifact_dir/stack/skills/$worker.log"
    if project_trigger directory::skills::download_from_registry "$request" 60000 >"$receipt" 2>"$error_log"; then
      jq -e --arg version "$(jq -r '.version' <<<"$package")" '.source.version == $version' "$receipt" >/dev/null
    else
      # A worker may legitimately publish no skill bundle. Other failures must
      # not turn a requested profile into an execution with different assets.
      grep -q 'D310 not_found:.*has no published skills bundle' "$receipt" "$error_log" || fail "Could not load pinned skills for $worker"
      rm -f "$iii_failed"
    fi
  done < <(jq -c '.[]' "$artifact_dir/stack/declared-workers.json")
  # Registry bundles may also contain profiles. The explicitly selected
  # template owns collisions; restore its files after the versioned downloads.
  for folder in agents skills; do
    if [[ -d "$run_root/template-assets/$folder" ]]; then
      mkdir -p "$project_dir/$folder"
      cp -R "$run_root/template-assets/$folder/." "$project_dir/$folder/"
    fi
  done
fi

if jq -e '.suite.agent_profile != null' "$contract_path" >/dev/null; then
  failure_phase=agent_profile_resolution
  agent_request=$(jq -c '{id: .suite.agent_profile}' "$contract_path")
  agent_deadline=$((SECONDS + 120))
  # Directory downloads installed workers' profiles asynchronously after boot.
  until project_trigger directory::agents::get "$agent_request" 10000 \
    >"$artifact_dir/stack/agent-profile.json" 2>"$artifact_dir/stack/agent-profile-error.log"; do
    ((SECONDS < agent_deadline)) || fail "Directory profile $(jq -r '.suite.agent_profile' "$contract_path") is unavailable after 120s"
    sleep 2
  done
fi

failure_phase=runner_readiness
catalog_payload=$(jq -cn --argjson seed "$seed" '{seed:$seed}')
runner_ready=false
readiness_started=$SECONDS
while ((SECONDS - readiness_started < wait_seconds)); do
  kill -0 "$engine_pid" 2>/dev/null || fail "iii engine exited before the E2E runner became ready"
  kill -0 "$compose_pid" 2>/dev/null || fail "iii compose exited before the E2E runner became ready"
  if project_trigger e2e::scenarios-list "$catalog_payload" "$probe_ms" \
    >"$artifact_dir/catalog.json" 2>"$artifact_dir/logs/runner-readiness.log"; then
    runner_ready=true
    break
  fi
  sleep 1
done
[[ "$runner_ready" == true ]] || fail "E2E runner did not register e2e::scenarios-list within \
${wait_seconds}s (gave up after $((SECONDS - readiness_started))s)"

failure_phase=materialization
python3 "$contract_tool" materialize \
  --contract "$contract_path" \
  --catalog "$artifact_dir/catalog.json" \
  --workers "$artifact_dir/stack/workers.json" \
  --namespace "$namespace" \
  --output "$artifact_dir/run-request.json" \
  --group-id "$campaign_group_id"

failure_phase=execution
run_started=true
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

terminal_phase=$(jq -r '.phase' "$artifact_dir/status.json")
terminal_failure=""
if [[ "$terminal_phase" != completed ]]; then
  terminal_failure=$(jq -r '.error // empty' "$artifact_dir/status.json")
  [[ -n "$terminal_failure" ]] || terminal_failure="E2E execution ended in $terminal_phase"
fi

failure_phase=results
results_response="$artifact_dir/results-get.json"
project_trigger e2e::results-get \
  "$(jq -cn --arg execution_id "$remote_execution_id" '{execution_id:$execution_id}')" 120000 \
  >"$results_response"
jq -e --arg id "$remote_execution_id" '.execution_id == $id' "$results_response" >/dev/null
native_result_path=$(jq -r '.result_path | select(type == "string" and length > 0)' "$results_response")
[[ -n "$native_result_path" ]] || fail "${terminal_failure:-E2E execution produced no result artifact}"
case "$native_result_path" in
  /*|*".."*) fail "unsafe native result path: $native_result_path" ;;
esac
native_dir="$e2e_data/$(dirname -- "$native_result_path")"
for native_name in results.json manifest.json observation.json; do
  test -f "$native_dir/$native_name"
  cp -- "$native_dir/$native_name" "$artifact_dir/$native_name"
done
python3 "$repo_root/scripts/extract_kanban_reports.py" \
  --native-dir "$native_dir" --output-dir "$artifact_dir/deliverables" \
  >"$artifact_dir/kanban-deliverables.json"
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
# Archiving is best effort: whatever it answered is not why the group failed.
rm -f "$iii_failed"
[[ -z "$terminal_failure" ]] || fail "$terminal_failure"

failure_phase=compose_down
compose_trigger compose::down "file=$compose_file" >"$artifact_dir/stack/down.json"
compose_down=true
failure_phase=complete
