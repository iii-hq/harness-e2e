#!/usr/bin/env bash
# One phase of an execution, run from the checkout's root inside the executor
# image (scripts/run_in_image.sh starts it there). Every phase reads and writes
# below target/, as the workflow always has; GitHub's jobs and the Console's
# Docker executions run the same phases:
#
#   prepare [materialize|assemble|fixtures]
#       materialize  read the dispatch (DISPATCH_*), resolve iii and the
#                    template, fetch the stack's runner and materialize the
#                    suite with it.
#       assemble     write one contract per campaign (EXECUTION_KEY), assemble
#                    and lock the stack once, and lock every contract to it.
#       fixtures     check out below target/ what the groups start from and
#                    no package brings: the Kanban fixture, the Linkly
#                    templates, the stack's template, the Registry sources and
#                    the trending topics fixture. For the group
#                    HARNESS_E2E_CAMPAIGN_GROUP_ID names, else for every group
#                    of the execution. The private ones read GITHUB_TOKEN.
#       Without an argument, materialize and assemble. GitHub reports the
#       materialized suite to Release Control between the two, before
#       anything is assembled.
#   group     start one group's frozen stack and run its scenarios
#             (HARNESS_E2E_CONTRACT, HARNESS_E2E_CAMPAIGN_GROUP_ID, ...), with
#             the fixture repositories it clones read from those checkouts.
#   package WORKFLOW ROOT...
#             check that each ROOT (below target/, its contract in
#             stack-lock.json) holds nothing unsafe and hash it into its
#             bundle-manifest.json; WORKFLOW is the JSON naming who ran it.
#   finalize [restore|aggregate]
#       restore    lay every campaign's groups out under
#                  target/harness-e2e-campaign/ from the group bundles
#                  target/selected-group-artifacts.json names in
#                  ${HARNESS_E2E_GROUP_ARTIFACTS:-target/downloaded-groups};
#                  a group without one reads as never observed.
#       aggregate  aggregate every campaign there with the stack's runner
#                  into its execution-summary.json.
#       Without an argument, both.
set -Eeuo pipefail

usage() {
  echo "usage: executor.sh prepare [materialize|assemble|fixtures] | group | package WORKFLOW ROOT... | finalize [restore|aggregate]" >&2
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

# The revisions the scenarios expect of their fixture sources.
REGISTRY_REVISION=662eb87c1bdbb395f36264d5d26bf823e2ace783
TRENDING_TOPICS_REVISION=3ee24f7ace3c014db35423f14939ad3f6ce0c3d2
LINKLY_TEMPLATES_REVISION=ba1dfd95d4f4120705c8b0cc95d9a2ef86a0290d
PRIVATE_REPOSITORIES=" iii-hq/registry iii-hq/e2e-fixture "

# checkout REPOSITORY REF DIRECTORY [full]: REF is a commit, a branch, or
# empty for the default branch, one commit deep unless `full`. A directory
# that already holds that commit is kept. The token of a private repository
# goes in the environment of the git calls, never on disk or a command line.
checkout() {
  local repository=$1 ref=$2 directory=$3 depth=(--depth 1)
  [[ "${4:-}" != full ]] || depth=()
  if [[ -d "$directory/.git" ]]; then
    if [[ "$ref" =~ ^[0-9a-f]{40}$ && "$(git -C "$directory" rev-parse HEAD 2>/dev/null)" == "$ref" ]]; then
      return 0
    fi
    rm -rf "$directory"
  fi
  (
    if [[ "$PRIVATE_REPOSITORIES" == *" $repository "* && -n "${GITHUB_TOKEN:-}" ]]; then
      export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.https://github.com/.extraheader
      GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $(printf 'x-access-token:%s' "$GITHUB_TOKEN" | base64 -w0)"
      export GIT_CONFIG_VALUE_0
    fi
    url=https://github.com/$repository.git
    if [[ "$ref" =~ ^[0-9a-f]{40}$ ]]; then
      git init -q "$directory"
      git -C "$directory" fetch -q "${depth[@]}" "$url" "$ref"
      git -C "$directory" checkout -q --detach FETCH_HEAD
    else
      git clone -q "${depth[@]}" ${ref:+--branch "$ref"} "$url" "$directory"
    fi
  )
}

fixtures() {
  local groups group template
  if [[ -n "${HARNESS_E2E_CAMPAIGN_GROUP_ID:-}" ]]; then
    groups=$HARNESS_E2E_CAMPAIGN_GROUP_ID
  else
    groups=$(jq -r '[.matrix.include[].group_id] | unique | .[]' "$contracts/resolution.json")
  fi
  for group in $groups; do
    case "$group" in
      case-kanban-*) checkout iii-hq/kanban-e2e-fixture main target/kanban-fixture full ;;
      case-registry-*)
        checkout iii-hq/registry "$REGISTRY_REVISION" target/registry-sources/registry
        checkout iii-hq/e2e-fixture "" target/registry-sources/e2e-fixture
        ;;
      case-trending-topics-build)
        checkout iii-hq/e2e-fixture "$TRENDING_TOPICS_REVISION" target/trending-topics-fixture
        ;;
      case-linkly-tutorial) checkout iii-hq/templates "$LINKLY_TEMPLATES_REVISION" target/linkly-templates ;;
    esac
  done
  template=$(jq -r '.template.revision // empty' "$contracts/resolution.json")
  [[ -z "$template" ]] || checkout iii-hq/templates "$template" target/execution-template
}

# The fixture repositories a group's scenarios clone, read from the
# checkouts `prepare fixtures` left; git reads this from the environment. A
# checkout that is missing fails the clone, and the scenario says so.
route_fixtures() {
  local routes=()
  case "${HARNESS_E2E_CAMPAIGN_GROUP_ID:-}" in
    case-registry-*)
      routes=(target/registry-sources/registry https://github.com/iii-hq/registry.git
        target/registry-sources/e2e-fixture https://github.com/iii-hq/e2e-fixture.git)
      ;;
    case-trending-topics-build) routes=(target/trending-topics-fixture git@github.com:iii-hq/e2e-fixture.git) ;;
  esac
  local index
  for ((index = 0; index < ${#routes[@]} / 2; index++)); do
    export "GIT_CONFIG_KEY_$index=url.file://$PWD/${routes[index * 2]}.insteadOf"
    export "GIT_CONFIG_VALUE_$index=${routes[index * 2 + 1]}"
  done
  ((${#routes[@]} == 0)) || export GIT_CONFIG_COUNT=$((${#routes[@]} / 2))
}

package() {
  local workflow=${1:?package needs the workflow identity} root
  shift
  (($# > 0)) || usage
  for root in "$@"; do
    python3 scripts/exact_stack_campaign.py package \
      --root "$root" \
      --contract "$root/stack-lock.json" \
      --workflow "$workflow" \
      --output "$root/bundle-manifest.json"
  done
}

restore() {
  local bundles=${HARNESS_E2E_GROUP_ARTIFACTS:-target/downloaded-groups}
  local selected=target/selected-group-artifacts.json contract campaign root group name destination
  rm -rf target/harness-e2e-campaign
  while IFS= read -r contract; do
    campaign=$(basename "$contract" .json)
    root=target/harness-e2e-campaign/$campaign
    mkdir -p "$root/groups"
    cp "$contract" "$root/stack-lock.json"
    while IFS= read -r group; do
      name=""
      [[ ! -f "$selected" ]] || name=$(jq -r --arg job "$campaign · $group" '.[$job].name // empty' "$selected")
      destination=$root/groups/$group
      if [[ -n "$name" && -d "$bundles/$name" ]]; then
        # Copied: a group bundle stays where the next attempt looks for it.
        cp -a "$bundles/$name" "$destination"
      else
        mkdir -p "$destination"
        jq -n --arg group_id "$group" --arg campaign_id "$campaign" \
          '{phase:"workflow_artifact_download",outcome:"infra_failed",campaign_id:$campaign_id,group_id:$group_id,error:"group observation artifact was not available"}' \
          >"$destination/failure.json"
      fi
    done < <(python3 scripts/exact_stack_campaign.py groups --contract "$contract")
  done < <(find "$contracts" -name '*.json' ! -name resolution.json | sort)
}

aggregate() {
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
      fixtures) fixtures ;;
      "")
        materialize
        assemble
        ;;
      *) usage ;;
    esac
    ;;
  group)
    route_fixtures
    exec bash scripts/run_exact_stack_group.sh
    ;;
  package)
    shift
    package "$@"
    ;;
  finalize)
    case "${2:-}" in
      restore) restore ;;
      aggregate) aggregate ;;
      "")
        restore
        aggregate
        ;;
      *) usage ;;
    esac
    ;;
  *) usage ;;
esac
