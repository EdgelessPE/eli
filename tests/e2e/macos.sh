#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
temp_base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
test_root="$(mktemp -d "${temp_base%/}/eli-e2e.XXXXXX")"
mount_a="$test_root/mount-a"
mount_z="$test_root/mount-z"
image_a="$test_root/bootdisk-a.dmg"
image_z="$test_root/bootdisk-z.dmg"
mounted=()

cleanup() {
    for ((index = ${#mounted[@]} - 1; index >= 0; index--)); do
        hdiutil detach -quiet "${mounted[$index]}" 2>/dev/null || true
    done
    rm -f "$image_a" "$image_z"
    rmdir "$mount_a" "$mount_z" 2>/dev/null || true
    rmdir "$test_root" 2>/dev/null || true
}
trap cleanup EXIT

create_boot_disk() {
    local image="$1"
    local mount_point="$2"
    local version="$3"
    local volume_name="$4"
    mkdir "$mount_point"
    hdiutil create -quiet -size 16m -fs HFS+ -volname "$volume_name" "$image"
    hdiutil attach -quiet -nobrowse -mountpoint "$mount_point" "$image"
    mounted+=("$mount_point")
    mkdir "$mount_point/Edgeless"
    printf '%s' "$version" > "$mount_point/Edgeless/version.txt"
}

create_boot_disk "$image_a" "$mount_a" 'eli-e2e-macos-a' 'ELI_E2E_A'
create_boot_disk "$image_z" "$mount_z" 'eli-e2e-macos-z' 'ELI_E2E_Z'

find_device() {
    local target="$1"
    local line
    while IFS= read -r line; do
        if [[ "$line" == *" on $target ("* ]]; then
            printf '%s' "${line%% on *}"
            return 0
        fi
    done < <(/sbin/mount)
    return 1
}

device_a="$(find_device "$mount_a")"
device_z="$(find_device "$mount_z")"
[[ "$device_a" == /dev/* && "$device_z" == /dev/* ]]

cd "$repo_root"
cargo +stable test --quiet --package eli-lib --test version_identifier
cargo +stable build --quiet --package eli-cli
eli="$repo_root/target/debug/eli"
stdout_path="$test_root/stdout.txt"
stderr_path="$test_root/stderr.txt"

"$eli" bootdisk get > "$stdout_path" 2> "$stderr_path"
grep -Fqx "$device_z" "$stdout_path"
grep -Eq '^warning: found [2-9][0-9]* Edgeless boot disks;' "$stderr_path"

"$eli" --bootdisk "$device_a/" bootdisk get > "$stdout_path" 2> "$stderr_path"
grep -Fqx "$device_a" "$stdout_path"
[[ ! -s "$stderr_path" ]]
