#!/usr/bin/env bash
set -Eeuo pipefail

: "${HARNESS_E2E_CONTRACT:?HARNESS_E2E_CONTRACT (the campaign contract file) is required}"
: "${HARNESS_E2E_CAMPAIGN_GROUP_ID:?HARNESS_E2E_CAMPAIGN_GROUP_ID is required}"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/.." && pwd)
contract_tool="$repo_root/scripts/exact_stack_campaign.py"
# Set by the execution's preparation: assemble the stack once through
# compose::add, leave it and its worker-compose.lock under the artifact
# directory, and stop there. Groups then start that stack frozen.
assemble_only=${HARNESS_E2E_ASSEMBLE_ONLY:-}
artifact_dir=${HARNESS_E2E_ARTIFACTS_DIR:-"$repo_root/target/harness-e2e-shadow"}
engine_port=${HARNESS_E2E_ENGINE_PORT:-49134}
wait_seconds=${HARNESS_E2E_WAIT_SECONDS:-300}
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
project_template=$(python3 "$contract_tool" group-template --contract "$contract_path" --group-id "$campaign_group_id")
execution_template=$(jq -r '.runtime.template.id // empty' "$contract_path")
linkly_fixture=$(jq -r --arg id "$campaign_group_id" '.suite.groups[] | select(.id == $id) | any(.scenarios[]?; . == "linkly_tutorial")' "$contract_path")
if [[ -n "$assemble_only" ]]; then
  project_template=""
  execution_template=""
  linkly_fixture=false
fi
# A stack the execution already assembled starts from its lock; a template
# project is assembled here, pinned to the versions that lock resolved.
frozen=false
if [[ -z "$project_template" ]] && jq -e '.runtime.lock != null' "$contract_path" >/dev/null; then
  frozen=true
fi
profile_assets=$(jq -r '.runtime.template != null or .suite.agent_profile != null' "$contract_path")
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
# TMPDIR is configurable; reject an uploaded runtime/secret tree before any
# provider credential or Compose state is written into it.
if ! python3 "$contract_tool" validate-layout \
  --artifact-root "$artifact_dir" --runtime-root "$run_root" --allowed-root "$repo_root/target"; then
  rmdir -- "$run_root"
  exit 2
fi
project_dir="$run_root/project"
evaluation_dir="$run_root/evaluation"
engine_config="$project_dir/iii.config.yaml"
compose_file="$artifact_dir/stack/worker-compose.yaml"
compose_working_dir="$repo_root"
if [[ -n "$project_template" ]]; then
  compose_file="$project_dir/worker-compose.yaml"
  compose_working_dir="$project_dir"
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

iii_bin="$tools_dir/iii"
engine_pid=""
compose_pid=""
compose_started=false
compose_down=false
failure_phase=cli_install
failure_reason=""

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
    --engine "$engine_url" \
    --namespace "$namespace" \
    --timeout-ms 600000 \
    "$@"
}

project_trigger() {
  local function_id=$1
  local payload=$2
  local timeout_ms=${3:-30000}
  "$iii_bin" trigger "$function_id" \
    --engine "$engine_url" \
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
  if [[ -n "$project_template" && -f "$compose_file" ]]; then
    cp "$compose_file" "$artifact_dir/stack/worker-compose-final.yaml"
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

wait_for_engine() {
  local response
  for ((attempt = 0; attempt < wait_seconds; attempt++)); do
    kill -0 "$engine_pid" 2>/dev/null || fail "iii engine exited before becoming ready"
    response=$("$iii_bin" trigger engine::workers::list --engine "$engine_url" --json '{}' 2>/dev/null || true)
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
export PATH="$tools_dir:$PATH"
observed_cli_version=$("$iii_bin" --version 2>&1)
printf '%s\n' "$observed_cli_version" >"$artifact_dir/iii-version.txt"
[[ "$observed_cli_version" == *"$cli_version"* ]] || fail \
  "iii CLI version mismatch: expected $cli_version, observed $observed_cli_version"
forbidden_name="iii""-worker"
if find "$tools_dir" -type f -name "$forbidden_name" -print -quit | grep -q .; then
  fail "forbidden lifecycle helper was installed"
fi

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

template_project="$project_dir"
if [[ -n "$execution_template" ]]; then
  failure_phase=template_scaffold
  template_root="$repo_root/target/execution-template"
  template_revision=$(jq -er '.runtime.template.revision' "$contract_path")
  [[ "$(git -C "$template_root" rev-parse HEAD)" == "$template_revision" ]] || fail "Execution template revision mismatch"
  if [[ "$linkly_fixture" == true ]]; then
    template_project="$run_root/execution-template"
  fi
  "$iii_bin" project init --directory "$template_project" --template "$execution_template" \
    --template-dir "$template_root/iii" --skip-iii >"$artifact_dir/logs/template-scaffold.log" 2>&1
  jq '.runtime.template' "$contract_path" >"$artifact_dir/stack/template.json"
  mkdir -p "$run_root/template-assets"
  for folder in agents skills; do
    if [[ -d "$template_project/$folder" ]]; then
      cp -R "$template_project/$folder" "$run_root/template-assets/$folder"
    fi
  done
fi

if [[ "$linkly_fixture" == true ]]; then
  failure_phase=template_scaffold
  template_root="$repo_root/target/linkly-templates"
  template_revision=ba1dfd95d4f4120705c8b0cc95d9a2ef86a0290d
  [[ "$(git -C "$template_root" rev-parse HEAD)" == "$template_revision" ]] || fail "Linkly template revision mismatch"
  "$iii_bin" project init --directory "$project_dir" --template "$project_template" \
    --template-dir "$template_root/iii" --skip-iii >"$artifact_dir/logs/fixture-scaffold.log" 2>&1
  jq -n --arg template "$project_template" --arg revision "$template_revision" \
    '{repository:"iii-hq/templates",revision:$revision,template:$template}' >"$artifact_dir/stack/fixture-template.json"
fi

# One env file for the whole project, written after any template scaffold so
# it replaces the placeholder the template ships. A provider without its key
# is said out loud and the group still runs; TYPESAFE_API_KEY is optional.
: >"$env_file"
chmod 600 "$env_file"
for variable in DEEPSEEK_API_KEY ZAI_API_KEY TYPESAFE_API_KEY; do
  if [[ -z "${!variable:-}" ]]; then
    if [[ "$variable" != TYPESAFE_API_KEY ]]; then
      log "[WARN] $variable is not set; its provider starts without a credential"
    fi
    continue
  fi
  printf '%s=%s\n' "$variable" "${!variable}" >>"$env_file"
done

project_args=(
  --contract "$contract_path"
  --env-file "$env_file"
  --namespace "$namespace"
  --data-dir "$e2e_data"
  --environment "harness-e2e.HARNESS_E2E_RUN_DIR=$evaluation_dir"
  --environment "harness-e2e.HARNESS_E2E_LANE=$(jq -r '.suite.lane' "$contract_path")"
  --environment "harness-e2e.HARNESS_E2E_CAMPAIGN_GROUP=$campaign_group_id"
  --output "$compose_file"
  --engine-config "$engine_config"
  --engine-port "$engine_port"
)
# Without a group the scaffold is the stack the whole suite shares.
if [[ -n "$assemble_only" ]]; then
  project_args+=(--assemble)
else
  project_args+=(--group-id "$campaign_group_id")
fi
if [[ -n "$project_template" ]]; then
  project_args+=(--template-compose "$template_project/worker-compose.yaml"
    --template-package shell=ide --template-package console=ade)
  if [[ "$template_project" != "$project_dir" ]]; then
    project_args+=(--fixture-compose "$compose_file")
  fi
fi
# The Directory's configuration is per group; the assembled stack has none.
if [[ "$profile_assets" == true && -z "$assemble_only" ]]; then
  project_args+=(--profile-root "$project_dir")
fi
if [[ -n "${HARNESS_E2E_KANBAN_RUNTIME:-}" ]]; then
  project_args+=(--environment "harness-e2e.HARNESS_E2E_KANBAN_RUNTIME=$HARNESS_E2E_KANBAN_RUNTIME")
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
    add_args+=("worker=$(python3 "$contract_tool" roots --compose "$compose_file" | grep '^harness-e2e@')")
  else
    # Except what the executor only needs to exist: the model's provider and
    # the Directory. Harness's graph brings them pinned, and asking for one as
    # well is a second, conflicting spec. Declared, one keeps its pin unasked.
    ensured=" provider-$(jq -r '.suite.subject.provider' "$contract_path") iii-directory "
    while IFS= read -r root; do
      [[ "$ensured" == *" ${root%@*} "* ]] || add_args+=("worker=$root")
    done < <(python3 "$contract_tool" roots --compose "$compose_file")
  fi
  compose_trigger compose::add "${add_args[@]}" >"$artifact_dir/stack/add.json"
  await_compose_add "$artifact_dir/stack/add.json" "$artifact_dir/stack/add-operation.json"
  [[ -z "$project_template" ]] || cp "$compose_file" "$artifact_dir/stack/worker-compose.yaml"
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

python3 "$contract_tool" roots --compose "$compose_file" \
  | jq -Rc 'split("@") | {worker: .[0], version: .[1]}' \
  | jq -sc '.' >"$artifact_dir/stack/declared-workers.json"

failure_phase=project_start
compose_trigger compose::up --json "$(jq -cn --arg file "$compose_file" --argjson frozen "$frozen" '{file:$file,frozen:$frozen}')" \
  >"$artifact_dir/stack/up.json"
jq -e '.status == "ok"' "$artifact_dir/stack/up.json" >/dev/null
compose_trigger compose::status "file=$compose_file" >"$artifact_dir/stack/status.json"
"$iii_bin" trigger engine::workers::list --engine "$engine_url" --json '{}' \
  >"$artifact_dir/stack/workers.json"
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
for ((attempt = 0; attempt < wait_seconds; attempt++)); do
  kill -0 "$engine_pid" 2>/dev/null || fail "iii engine exited before the E2E runner became ready"
  kill -0 "$compose_pid" 2>/dev/null || fail "iii compose exited before the E2E runner became ready"
  if project_trigger e2e::scenarios-list "$catalog_payload" 120000 \
    >"$artifact_dir/catalog.json" 2>"$artifact_dir/logs/runner-readiness.log"; then
    runner_ready=true
    break
  fi
  sleep 1
done
[[ "$runner_ready" == true ]] || fail "E2E runner did not register e2e::scenarios-list within ${wait_seconds}s"

failure_phase=materialization
python3 "$contract_tool" materialize \
  --contract "$contract_path" \
  --catalog "$artifact_dir/catalog.json" \
  --workers "$artifact_dir/stack/workers.json" \
  --namespace "$namespace" \
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
[[ -z "$terminal_failure" ]] || fail "$terminal_failure"

failure_phase=compose_down
compose_trigger compose::down "file=$compose_file" >"$artifact_dir/stack/down.json"
compose_down=true
failure_phase=complete
