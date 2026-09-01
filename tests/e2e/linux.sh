#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
temp_base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
test_root="$(mktemp -d "${temp_base%/}/eli-e2e.XXXXXX")"
disk_image="$test_root/bootdisk.img"
mount_point="$test_root/mount"
version='eli-e2e-linux'
mounted=false

cleanup() {
    if [[ "$mounted" == true ]]; then
        sudo umount "$mount_point" 2>/dev/null || true
    fi
    rm -f "$disk_image"
    rmdir "$mount_point" 2>/dev/null || true
    rmdir "$test_root" 2>/dev/null || true
}
trap cleanup EXIT

mkdir "$mount_point"
truncate -s 16M "$disk_image"
/sbin/mkfs.ext4 -q "$disk_image"
sudo mount -o loop "$disk_image" "$mount_point"
mounted=true
sudo chown "$(id -u):$(id -g)" "$mount_point"
mkdir "$mount_point/Edgeless"
printf '%s' "$version" > "$mount_point/Edgeless/version.txt"

cd "$repo_root"
output="$(cargo +stable run --quiet --package eli-cli -- bootdisk list)"
expected="${mount_point}"$'\t'"${version}"
grep -Fqx "$expected" <<< "$output"
