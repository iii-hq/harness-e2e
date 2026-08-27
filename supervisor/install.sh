#!/usr/bin/env bash
set -Eeuo pipefail

source_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
install_root=/opt/iii-harness-e2e
fault_driver=""
while (($#)); do
  case "$1" in
    --fault-driver) fault_driver=$2; shift 2 ;;
    --root) install_root=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ $EUID -eq 0 ]] || { echo 'protected supervisor installation must run as root' >&2; exit 1; }
[[ -n "$fault_driver" && -x "$fault_driver" ]] || {
  echo '--fault-driver must name a protected Compose-compatible fault driver' >&2
  exit 2
}
"$fault_driver" --protocol-check | grep -Fx 'compose-fault-driver' >/dev/null

iii_version=0.23.0-rc.4
iii_target=x86_64-unknown-linux-gnu
archive="iii-${iii_target}.tar.gz"
archive_sha=d9ab056f17daefc2f04ed892092a3df2fe76ffde5587335918606048047cf40a
parent=$(dirname "$install_root")
name=$(basename "$install_root")
bundle_root="$install_root/compose-supervisor"
retired="$install_root/.compose-supervisor.retired"
mkdir -p "$parent" "$install_root" "$install_root/secrets"
chmod 700 "$install_root/secrets"
stage=$(mktemp -d "$install_root/.compose-supervisor.install.XXXXXX")
trap 'find "$stage" -depth -mindepth 1 -delete 2>/dev/null || true; rmdir "$stage" 2>/dev/null || true' EXIT
mkdir -p "$stage/bin" "$stage/checksums" "$stage/lib" "$stage/state"
chmod 700 "$stage/state"

curl --fail --location --retry 3 \
  "https://github.com/iii-hq/iii/releases/download/iii/v${iii_version}/${archive}" \
  --output "$stage/$archive"
printf '%s  %s\n' "$archive_sha" "$stage/$archive" | sha256sum --check --strict
tar -xzf "$stage/$archive" -C "$stage/bin" iii
install -m 0755 "$fault_driver" "$stage/bin/compose-fault-driver"
install -m 0644 "$source_root/scripts/release_control_campaign.py" "$stage/lib/release_control_campaign.py"
sha256sum "$stage/bin/iii" >"$stage/checksums/iii.sha256"
sha256sum "$stage/bin/compose-fault-driver" >"$stage/checksums/compose-fault-driver.sha256"
"$stage/bin/iii" --version | grep -F "$iii_version" >/dev/null
python3 "$stage/lib/release_control_campaign.py" --help >/dev/null

if [[ -e "$retired" ]]; then
  find "$retired" -depth -mindepth 1 -delete
  rmdir "$retired"
fi
if [[ -e "$bundle_root" ]]; then
  mv "$bundle_root" "$retired"
fi
mv "$stage" "$bundle_root"
stage="$bundle_root/.installation-complete"
printf '%s\n' "$iii_version" >"$stage"
chmod 0444 "$stage"
launcher="$install_root/.run-weekly-stress.new"
install -m 0755 "$source_root/supervisor/run-weekly-stress" "$launcher"
mv "$launcher" "$install_root/run-weekly-stress"

# Remove only history and binaries owned by the retired lifecycle. Other
# protected utilities in the shared installation root remain intact.
legacy_state="$install_root/state"
if [[ -d "$legacy_state" ]]; then
  find "$legacy_state" -depth -mindepth 1 -delete
  rmdir "$legacy_state"
fi
removed_helper="iii""-worker"
while IFS= read -r obsolete; do
  rm -f -- "$obsolete"
done < <(find "$install_root" -type f -name "$removed_helper" -print)
if [[ -e "$retired" ]]; then
  find "$retired" -depth -mindepth 1 -delete
  rmdir "$retired"
fi
trap - EXIT

"$install_root/run-weekly-stress" --help >/dev/null 2>&1 && {
  echo 'supervisor unexpectedly accepted an incomplete invocation' >&2
  exit 1
}
printf 'installed Compose-only protected supervisor at %s\n' "$bundle_root"
