#!/usr/bin/env bash
# Run one phase of an execution in the executor image.
#
#   scripts/run_in_image.sh [--env-file FILE] prepare [materialize|assemble]
#   scripts/run_in_image.sh [--env-file FILE] group
#   scripts/run_in_image.sh [--env-file FILE] finalize
#   scripts/run_in_image.sh image    print the image this checkout runs in
#
# The image holds only tools (the Dockerfile beside scripts/). The scripts are
# this checkout's: its root is mounted at the same path, with the target/
# directory every phase reads and writes. No container can gain privileges,
# each has a network of its own (a group starts its engine on 49134 there),
# and every phase runs as the calling user. Nothing reaches the host's
# Docker: a `group` container is privileged and starts as root, with a volume
# at /var/lib/docker, for the Docker daemon scripts/executor.sh starts in it
# before it runs the group as the calling user. The containers its scenarios
# start are that daemon's and go with the group's container and its volume.
# Its user reaches root in that container through the daemon's socket, and
# a privileged container is root on the host: what the host's socket gave a
# group before, and no more.
#
# The environment the phases read passes through by name, never by value on
# the command line: HARNESS_E2E_*, DISPATCH_*, the git configuration that
# GIT_CONFIG_COUNT states, CI, the execution key, the provider credentials
# and, to `prepare` alone, GITHUB_TOKEN; --env-file adds a file of them.
#
# The container is labelled harness-e2e.execution=$EXECUTION_KEY,
# harness-e2e.phase and harness-e2e.group. Interrupted, the wrapper stops it.
set -Eeuo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
repository=ghcr.io/iii-hq/harness-e2e
image="$repository:tools-$(sha256sum "$root/Dockerfile" | cut -c1-12)"

env_file=""
if [[ "${1:-}" == --env-file ]]; then
  env_file=${2:?--env-file needs a file}
  shift 2
fi
phase=${1:?usage: run_in_image.sh [--env-file FILE] prepare|group|finalize [args...] | image}
shift
if [[ "$phase" == image ]]; then
  printf '%s\n' "$image"
  exit 0
fi

warn() { printf '::warning::%s\n' "$*" >&2; }

# A group that fails before its container ran leaves its failure where the
# launcher would have, so it reports an infrastructure failure, not nothing.
record_failure() {
  local artifacts=${HARNESS_E2E_ARTIFACTS_DIR:-} error=${3//\\/\\\\}
  [[ -n "$artifacts" && ! -e "$artifacts/failure.json" ]] || return 0
  mkdir -p "$artifacts"
  printf '{"phase":"%s","outcome":"infra_failed","error":"%s","exit_code":%d}\n' \
    "$1" "${error//\"/\\\"}" "$2" >"$artifacts/failure.json"
}

# The published image, else one this host built before, else build it now: a
# Dockerfile nobody has published yet (a branch that changed it) still runs.
# The execution records the registry's digest when the image came from it,
# else the tag alone.
reference=$image
if docker pull --quiet "$image" >/dev/null 2>&1; then
  reference=$(docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$image" \
    | grep -m1 "^$repository@" || printf '%s' "$image")
elif docker image inspect "$image" >/dev/null 2>&1; then
  warn "$image is not published; running the one this host built"
else
  warn "$image is not published; building it from $root/Dockerfile"
  docker build --tag "$image" - <"$root/Dockerfile" >&2 || {
    status=$?
    record_failure executor_image "$status" "could not pull or build $image"
    exit "$status"
  }
fi

cidfile=$(mktemp -u "${TMPDIR:-/tmp}/harness-e2e-executor.XXXXXX.cid")
trap 'rm -f "$cidfile"' EXIT

labels=(--label "harness-e2e.execution=${EXECUTION_KEY:-}" --label "harness-e2e.phase=$phase"
  --label "harness-e2e.group=${HARNESS_E2E_CAMPAIGN_GROUP_ID:-}")
args=(run --rm --init --cidfile "$cidfile" "${labels[@]}"
  --security-opt no-new-privileges
  --volume "$root:$root" --workdir "$root"
  --env "HARNESS_E2E_EXECUTOR_IMAGE=$reference")
if [[ "$phase" == group ]]; then
  # Anonymous, so --rm removes it; labelled like the container.
  volume=type=volume,dst=/var/lib/docker
  for label in "${labels[@]}"; do
    [[ "$label" == --label ]] || volume+=",volume-label=$label"
  done
  # A cgroup namespace of its own whatever the host's default: the daemon
  # rearranges the cgroups it sees.
  args+=(--privileged --cgroupns private --user 0:0 --mount "$volume"
    --env "HARNESS_E2E_EXECUTOR_USER=$(id -u):$(id -g)")
else
  args+=(--user "$(id -u):$(id -g)")
fi
[[ -z "$env_file" ]] || args+=(--env-file "$env_file")
for name in $(compgen -e); do
  case "$name" in
    HARNESS_E2E_EXECUTOR_IMAGE | HARNESS_E2E_EXECUTOR_USER) ;;
    # Only prepare calls GitHub; a group's subject has a shell.
    GITHUB_TOKEN) [[ "$phase" != prepare ]] || args+=(--env "$name") ;;
    HARNESS_E2E_* | DISPATCH_* | GIT_CONFIG_COUNT | GIT_CONFIG_KEY_* | GIT_CONFIG_VALUE_* | CI | EXECUTION_KEY | \
      RELEASE_CONTROL_OIDC_AUDIENCE | DEEPSEEK_API_KEY | ZAI_API_KEY | TYPESAFE_API_KEY)
      args+=(--env "$name") ;;
  esac
done

# In the background, so a signal reaches the trap now rather than when the
# container ends: an interrupted phase stops its container (whose launcher
# still takes its stack down) instead of leaving it writing evidence.
docker "${args[@]}" "$image" bash scripts/executor.sh "$phase" "$@" &
container=$!
stop() {
  trap - INT TERM
  [[ ! -s "$cidfile" ]] || docker stop --time 30 "$(<"$cidfile")" >/dev/null 2>&1 || true
  wait "$container" 2>/dev/null || true
  exit "$1"
}
trap 'stop 130' INT
trap 'stop 143' TERM
status=0
wait "$container" || status=$?
if ((status != 0)) && [[ ! -s "$cidfile" ]]; then
  record_failure executor_start "$status" "the executor container did not start"
elif ((status != 0)) && [[ "$phase" == group ]]; then
  # Before the launcher ran (its Docker daemon did not start, say) or
  # without it writing one.
  record_failure executor "$status" "the group's executor container exited $status before the group recorded a failure"
fi
exit "$status"
