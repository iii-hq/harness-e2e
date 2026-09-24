#!/usr/bin/env bash
# One phase of an execution, run from the checkout's root inside the executor
# image (scripts/run_in_image.sh starts it there). Every phase reads and writes
# below target/, as the workflow always has:
#
#   prepare [materialize|assemble]
#       materialize  read the dispatch (DISPATCH_*), resolve iii and the
#                    template, fetch the stack's runner and materialize the
#                    suite with it.
#       assemble     write one contract per campaign (EXECUTION_KEY), assemble
#                    and lock the stack once, and lock every contract to it.
#       Without an argument, both. GitHub reports the materialized suite to
#       Release Control between the two, before anything is assembled.
#   group     start one group's frozen stack and run its scenarios
#             (HARNESS_E2E_CONTRACT, HARNESS_E2E_CAMPAIGN_GROUP_ID, ...).
#   finalize  aggregate every campaign under target/harness-e2e-campaign/ with
#             the stack's runner into its execution-summary.json.
set -Eeuo pipefail

usage() {
  echo "usage: executor.sh prepare [materialize|assemble] | group | finalize" >&2
  exit 2
}

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
contract_dir=target/harness-e2e-contract
contracts=$contract_dir/contracts

# The container runs as the caller's uid, which the image may not know; git,
# Node and Python ask the passwd database who that is.
if ! getent passwd "$(id -u)" >/dev/null && [[ -w /etc/passwd ]]; then
  printf 'executor:x:%s:%s::%s:/bin/bash\n' "$(id -u)" "$(id -g)" "$HOME" >>/etc/passwd
fi

materialize() {
  python3 scripts/prepare_execution.py dispatch --contract-dir "$contract_dir"
  python3 scripts/prepare_execution.py runtime --contract-dir "$contract_dir"
  # The suite is materialized by the runner the stack runs, never by a build
  # of this checkout: its master plan and scenario catalog are the ones every
  # group executes. suite.json is the snapshot; profile.json is the same file
  # under the name older Console imports and the ledger reports read.
  # `--profile` is the flag every runner release knows.
  local runner suite
  runner=$(python3 scripts/prepare_execution.py runner --contract-dir "$contract_dir" --work-dir "$(mktemp -d)")
  suite=$(jq -r '.suite | if type == "string" then . else tojson end' "$contract_dir/execution.json")
  "$runner" test-plan materialize --profile "$suite" >"$contract_dir/suite.json"
  jq -e '.campaigns | length > 0' "$contract_dir/suite.json" >/dev/null
  cp "$contract_dir/suite.json" "$contract_dir/profile.json"
}

assemble() {
  python3 scripts/prepare_execution.py contracts \
    --contract-dir "$contract_dir" \
    --execution-key "${EXECUTION_KEY:?EXECUTION_KEY names the execution}" \
    --oidc-audience "${RELEASE_CONTROL_OIDC_AUDIENCE:-release-control-harness-e2e}"
  # Once for the whole execution: compose::add expands every declared worker
  # into its graph and writes worker-compose.lock. The groups start exactly
  # that project, frozen, so they all run the same versions. It gets the
  # credentials the groups get, and one more try before the execution fails.
  HARNESS_E2E_CONTRACT="$contracts/$(jq -r '.campaign_ids[0]' "$contracts/resolution.json").json"
  HARNESS_E2E_CAMPAIGN_GROUP_ID=$(jq -r '.matrix.include[0].group_id' "$contracts/resolution.json")
  export HARNESS_E2E_CONTRACT HARNESS_E2E_CAMPAIGN_GROUP_ID HARNESS_E2E_ASSEMBLE_ONLY=1
  local attempt assembled=""
  for attempt in 1 2; do
    export HARNESS_E2E_ARTIFACTS_DIR="$PWD/target/harness-e2e-assembly/attempt-$attempt"
    if bash scripts/run_exact_stack_group.sh; then
      assembled=$HARNESS_E2E_ARTIFACTS_DIR/stack
      break
    fi
    echo "::warning::stack assembly attempt $attempt failed"
  done
  [[ -n "$assembled" ]] || { echo "the stack could not be assembled" >&2; return 1; }
  python3 scripts/prepare_execution.py lock --contract-dir "$contract_dir" --assembled "$assembled"
  # A missing or empty matrix means no group runs: say so here.
  jq -e '.matrix.include | length > 0' "$contracts/resolution.json" >/dev/null
  local contract
  for contract in "$contracts"/*.json; do
    [[ "$(basename "$contract")" == resolution.json ]] && continue
    python3 scripts/exact_stack_campaign.py validate --contract "$contract" >/dev/null
  done
}

finalize() {
  # Aggregated by the runner the stack ran, as the suite was materialized by
  # it: one release scores what it executed.
  local runner root
  runner=$(python3 scripts/prepare_execution.py runner-binary \
    --lock "$contract_dir/worker-compose.lock" --work-dir "$(mktemp -d)")
  for root in target/harness-e2e-campaign/*/; do
    python3 scripts/exact_stack_campaign.py manifest \
      --contract "$root/stack-lock.json" \
      --output "$root/campaign-manifest.json"
    python3 scripts/run_e2e_campaign.py \
      "$root/campaign-manifest.json" \
      --e2e-bin "$runner" \
      --aggregate-existing-root "$root/groups" \
      --execution-id "${EXECUTION_KEY:?EXECUTION_KEY names the execution}" \
      --summary "$root/campaign-summary.json" \
      --bundle "$root/campaign-bundle.json"
  done
  jq -s '{campaigns: .}' target/harness-e2e-campaign/*/campaign-summary.json \
    >target/harness-e2e-campaign/execution-summary.json
}

case "${1:-}" in
  prepare)
    case "${2:-}" in
      materialize) materialize ;;
      assemble) assemble ;;
      "")
        materialize
        assemble
        ;;
      *) usage ;;
    esac
    ;;
  group) exec bash scripts/run_exact_stack_group.sh ;;
  finalize) finalize ;;
  *) usage ;;
esac
