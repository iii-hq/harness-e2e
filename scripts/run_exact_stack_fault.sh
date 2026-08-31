#!/usr/bin/env bash
set -Eeuo pipefail

: "${HARNESS_E2E_STACK_LOCK:?HARNESS_E2E_STACK_LOCK is required}"
: "${HARNESS_E2E_CAMPAIGN_GROUP_ID:?HARNESS_E2E_CAMPAIGN_GROUP_ID is required}"
: "${HARNESS_E2E_HARNESS_ROOT:?HARNESS_E2E_HARNESS_ROOT is required}"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/.." && pwd)
contract_tool="$repo_root/scripts/exact_stack_campaign.py"
artifact_dir=${HARNESS_E2E_ARTIFACTS_DIR:-"$repo_root/target/harness-e2e-shadow"}
supervisor=${HARNESS_E2E_FAULT_SUPERVISOR:-/opt/iii-harness-e2e/run-weekly-stress}
e2e_bin="$HARNESS_E2E_HARNESS_ROOT/target/release/harness-e2e"
contract_path="$artifact_dir/stack-lock.json"

case "$artifact_dir" in
  "$repo_root"/target/*) ;;
  *) echo "HARNESS_E2E_ARTIFACTS_DIR must be below $repo_root/target" >&2; exit 2 ;;
esac
mkdir -p "$artifact_dir"
printf '%s\n' "$HARNESS_E2E_STACK_LOCK" >"$contract_path"
python3 "$contract_tool" validate --contract "$contract_path" >/dev/null

group=$(jq -ce --arg id "$HARNESS_E2E_CAMPAIGN_GROUP_ID" '
  .suite.groups
  | map(select(.id == $id and .execution_kind == "fault_injection"))
  | if length == 1 then .[0] else error("fault group not found") end
' "$contract_path")
profile=$(jq -r '.fault_profile' <<<"$group")
scenario=$(jq -r '.fault_scenario' <<<"$group")
runs=$(jq -r '.runs' <<<"$group")
soak_minutes=$(jq -r '.soakMinutes' <<<"$group")
profile_path="$HARNESS_E2E_HARNESS_ROOT/config/profiles/${profile}.json"
plan="$artifact_dir/fault-plan.json"

failure_phase=preflight
failure_reason=""
cleanup() {
  local status=$?
  trap - EXIT ERR
  if ((status != 0)); then
    [[ -n "$failure_reason" ]] || failure_reason="fault execution failed during $failure_phase"
    jq -n --arg phase "$failure_phase" --arg error "$failure_reason" --argjson exit_code "$status" \
      '{phase:$phase,outcome:"infra_failed",error:$error,exit_code:$exit_code}' \
      >"$artifact_dir/failure.json"
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'failure_reason="command failed at line $LINENO"' ERR

test -x "$supervisor"
test -x "$e2e_bin"
test -f "$profile_path"
observed_version=$($e2e_bin --version | awk '{print $2}')
expected_version=$(jq -r '.orchestration.roots[] | select(.role == "runner") | .version' "$contract_path")
[[ "$observed_version" == "$expected_version" ]]
python3 "$contract_tool" manifest --contract "$contract_path" \
  --output "$artifact_dir/campaign-manifest.json"
python3 "$HARNESS_E2E_HARNESS_ROOT/scripts/run_e2e_campaign.py" \
  "$artifact_dir/campaign-manifest.json" --validate-only >/dev/null

failure_phase=plan
"$e2e_bin" fault-plan --profile "$profile_path" --output "$plan"

failure_phase=execution
deadline=$((SECONDS + soak_minutes * 60))
iteration=0
while ((iteration < runs || SECONDS < deadline)); do
  iteration=$((iteration + 1))
  output="$artifact_dir/run-${iteration}"
  mkdir -p "$output"
  "$supervisor" \
    --contract "$contract_path" \
    --profile "$profile_path" \
    --plan "$plan" \
    --scenario "$scenario" \
    --iteration "$iteration" \
    --output "$output"
  evaluation=(
    "$e2e_bin" fault-evaluate
    --profile "$profile_path"
    --plan "$plan"
    --journal "$output/fault-journal.json"
    --output "$output/fault-evaluation.json"
  )
  if [[ -e "$output/results" ]]; then
    evaluation+=(--results "$output/results")
  fi
  "${evaluation[@]}"
done

failure_phase=complete
