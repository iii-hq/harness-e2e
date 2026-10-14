#!/usr/bin/env bash
set -euo pipefail

archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
mkdir -p "$2"

curl --fail --silent --show-error --location \
  --retry 3 --retry-connrefused --retry-delay 1 --connect-timeout 15 --max-time 60 \
  --header "Authorization: Bearer $GITHUB_TOKEN" \
  --header "X-GitHub-Api-Version: 2022-11-28" \
  "$GITHUB_API_URL/repos/$GITHUB_REPOSITORY/actions/artifacts/$1/zip" \
  --output "$archive"
unzip -q "$archive" -d "$2"
