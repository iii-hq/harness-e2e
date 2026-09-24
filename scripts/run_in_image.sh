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
# files on both sides. The container runs as the calling user and, by
# default without the host's network, starts its engine on 49134 in a
# namespace of its own. HARNESS_E2E_DOCKER_NETWORK=host puts it on the host's
# network instead, for a host that runs one phase at a time: fixtures that
# publish a port on the host's loopback (Registry, for its screenshots) are
# only reachable from the phase there.
#
# The environment the phases read passes through by name, never by value on
# the command line: HARNESS_E2E_*, DISPATCH_*, the git configuration that
# GIT_CONFIG_COUNT states, the execution key, a GitHub token and the provider
# credentials; --env-file adds a file of them.
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
  docker build --tag "$image" - <"$root/Dockerfile" >&2
fi

socket=${DOCKER_HOST:-unix:///var/run/docker.sock}
socket=${socket#unix://}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/harness-e2e-executor.XXXXXX")
# Fixtures a scenario ran as root through the socket may leave files behind.
trap 'rm -rf "$tmp" 2>/dev/null || warn "could not remove all of $tmp"' EXIT
# Chromium opens a socket under TMPDIR, and a socket path holds 107 bytes.
((${#tmp} <= 60)) || warn "TMPDIR $tmp is too long for Chromium's socket: the browser worker will not start"

args=(run --rm --init
  --user "$(id -u):$(id -g)" --group-add "$(stat -c %g "$socket")"
  --volume "$socket:/var/run/docker.sock"
  --volume "$root:$root" --workdir "$root"
  --volume "$tmp:$tmp" --env "TMPDIR=$tmp"
  --env "HARNESS_E2E_EXECUTOR_IMAGE=$reference")
[[ -z "${HARNESS_E2E_DOCKER_NETWORK:-}" ]] || args+=(--network "$HARNESS_E2E_DOCKER_NETWORK")
[[ -z "$env_file" ]] || args+=(--env-file "$env_file")
for name in $(compgen -e); do
  case "$name" in
    HARNESS_E2E_EXECUTOR_IMAGE | HARNESS_E2E_DOCKER_NETWORK) ;;
    HARNESS_E2E_* | DISPATCH_* | GIT_CONFIG_COUNT | GIT_CONFIG_KEY_* | GIT_CONFIG_VALUE_* | EXECUTION_KEY | \
      RELEASE_CONTROL_OIDC_AUDIENCE | GITHUB_TOKEN | DEEPSEEK_API_KEY | ZAI_API_KEY | TYPESAFE_API_KEY)
      args+=(--env "$name") ;;
  esac
done
docker "${args[@]}" "$image" bash scripts/executor.sh "$phase" "$@"
