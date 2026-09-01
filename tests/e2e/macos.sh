#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
temp_base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
test_root="$(mktemp -d "${temp_base%/}/eli-e2e.XXXXXX")"
disk_image="$test_root/bootdisk.dmg"
mount_point="$test_root/mount"
version='eli-e2e-macos'
mounted=false

cleanup() {
    if [[ "$mounted" == true ]]; then
        hdiutil detach -quiet "$mount_point" 2>/dev/null || true
    fi
    rm -f "$disk_image"
    rmdir "$mount_point" 2>/dev/null || true
    rmdir "$test_root" 2>/dev/null || true
}
trap cleanup EXIT

mkdir "$mount_point"
hdiutil create -quiet -size 16m -fs HFS+ -volname ELI_E2E "$disk_image"
hdiutil attach -quiet -nobrowse -mountpoint "$mount_point" "$disk_image"
mounted=true
mkdir "$mount_point/Edgeless"
printf '%s' "$version" > "$mount_point/Edgeless/version.txt"

cd "$repo_root"
output="$(cargo +stable run --quiet --package eli-cli -- bootdisk list)"
expected="${mount_point}"$'\t'"${version}"
grep -Fqx "$expected" <<< "$output"
