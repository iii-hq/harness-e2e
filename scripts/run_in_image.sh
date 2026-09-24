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
# directory every phase reads and writes, and so is a fresh TMPDIR, so a path
# a scenario hands the host's Docker daemon through the socket names the same
# files on both sides. Only `group` gets that socket. The container runs as
# the calling user, can never gain privileges, and by default, without the
# host's network, starts its engine on 49134 in a namespace of its own.
# HARNESS_E2E_DOCKER_NETWORK=host puts it on the host's network instead, for a
# host that runs one phase at a time: fixtures that publish a port on the
# host's loopback (Registry, for its screenshots) are only reachable from the
# phase there.
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

tmp=$(mktemp -d "${TMPDIR:-/tmp}/harness-e2e-executor.XXXXXX")
cidfile="$tmp.cid"
# Fixtures a scenario ran as root through the socket may leave files behind.
trap 'rm -rf "$tmp" "$cidfile" 2>/dev/null || warn "could not remove all of $tmp"' EXIT
# Chromium opens a socket under TMPDIR, and a socket path holds 107 bytes.
((${#tmp} <= 60)) || warn "TMPDIR $tmp is too long for Chromium's socket: the browser worker will not start"

args=(run --rm --init --cidfile "$cidfile"
  --label "harness-e2e.execution=${EXECUTION_KEY:-}" --label "harness-e2e.phase=$phase"
  --label "harness-e2e.group=${HARNESS_E2E_CAMPAIGN_GROUP_ID:-}"
  --user "$(id -u):$(id -g)" --security-opt no-new-privileges
  --volume "$root:$root" --workdir "$root"
  --volume "$tmp:$tmp" --env "TMPDIR=$tmp"
  --env "HARNESS_E2E_EXECUTOR_IMAGE=$reference")
if [[ "$phase" == group ]]; then
  socket=${DOCKER_HOST:-unix:///var/run/docker.sock}
  socket=${socket#unix://}
  args+=(--group-add "$(stat -c %g "$socket")" --volume "$socket:/var/run/docker.sock")
fi
[[ -z "${HARNESS_E2E_DOCKER_NETWORK:-}" ]] || args+=(--network "$HARNESS_E2E_DOCKER_NETWORK")
[[ -z "$env_file" ]] || args+=(--env-file "$env_file")
for name in $(compgen -e); do
  case "$name" in
    HARNESS_E2E_EXECUTOR_IMAGE | HARNESS_E2E_DOCKER_NETWORK) ;;
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
fi
exit "$status"
