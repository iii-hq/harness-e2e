#!/usr/bin/env bash
set -euo pipefail

version=0.11.1
archive="bubblewrap-${version}.tar.xz"
url="https://github.com/containers/bubblewrap/releases/download/v${version}/${archive}"
sha256="c1b7455a1283b1295879a46d5f001dfd088c0bb0f238abb5e128b3583a246f71"
build_dir="${RUNNER_TEMP:-/tmp}/bubblewrap-${version}"

sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends bubblewrap libcap-dev libselinux1-dev meson ninja-build
mkdir -p "$build_dir"
curl --fail --location --silent --show-error "$url" -o "$build_dir/$archive"
echo "$sha256  $build_dir/$archive" | sha256sum --check --status
tar -xf "$build_dir/$archive" -C "$build_dir"
meson setup "$build_dir/build" "$build_dir/bubblewrap-$version" --buildtype=release -Dman=disabled
meson compile -C "$build_dir/build"
sudo install -m 0755 "$build_dir/build/bwrap" /usr/bin/bwrap
/usr/bin/bwrap --version
