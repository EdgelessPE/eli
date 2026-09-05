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
    mkdir "$mount_point/Edgeless/Resource"
    printf '%s' 'package' > "$mount_point/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z"
}

create_boot_disk "$image_a" "$mount_a" 'Edgeless_Alpa_4.1.2' 'ELI_E2E_A'
create_boot_disk "$image_z" "$mount_z" 'Edgeless_Beta_Ofial_4.1.0_2' 'ELI_E2E_Z'

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

if "$eli" plugin load "$test_root/plugin.7z" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Plugin loading unexpectedly accepted macOS.' >&2
    exit 1
fi
grep -Fq 'WindowsPE' "$stderr_path"
grep -Fq 'MacOS' "$stderr_path"

if "$eli" plugin localboost load "$test_root/plugin.7zl" > "$stdout_path" 2> "$stderr_path"; then
    echo 'LocalBoost loading unexpectedly accepted macOS.' >&2
    exit 1
fi
grep -Fq 'WindowsPE' "$stderr_path"
grep -Fq 'MacOS' "$stderr_path"

if "$eli" kernel version current > "$stdout_path" 2> "$stderr_path"; then
    echo 'Current kernel version unexpectedly accepted macOS.' >&2
    exit 1
fi
grep -Fq 'WindowsPE' "$stderr_path"
grep -Fq 'MacOS' "$stderr_path"

if "$eli" kernel download > "$stdout_path" 2> "$stderr_path"; then
    echo 'Kernel download unexpectedly accepted a missing directory.' >&2
    exit 1
fi
grep -Fq -- '--directory' "$stderr_path"

"$eli" bootdisk list > "$stdout_path" 2> "$stderr_path"
bootdisk_header='Bootdisk'
version_header='Version'
bootdisk_width=40
version_width=12
printf -v expected_header '%-*s%-*s%s' "$bootdisk_width" 'Bootdisk' "$version_width" 'Version' 'Release'
printf -v expected_alpha '%-*s%-*s%s' "$bootdisk_width" "$mount_a" "$version_width" '4.1.2' 'Alpha'
printf -v expected_beta '%-*s%-*s%s' "$bootdisk_width" "$mount_z" "$version_width" '4.1.0' 'Beta(Official)'
grep -Fqx "$expected_header" "$stdout_path"
grep -Fqx "$expected_alpha" "$stdout_path"
grep -Fqx "$expected_beta" "$stdout_path"
[[ ! -s "$stderr_path" ]]

"$eli" --bootdisk "$mount_a" kernel version bootdisk > "$stdout_path" 2> "$stderr_path"
printf -v expected_kernel_header '%-*s%s' "$version_width" 'Version' 'Release'
printf -v expected_kernel_version '%-*s%s' "$version_width" '4.1.2' 'Alpha'
grep -Fqx "$expected_kernel_header" "$stdout_path"
grep -Fqx "$expected_kernel_version" "$stdout_path"
[[ ! -s "$stderr_path" ]]

"$eli" bootdisk get > "$stdout_path" 2> "$stderr_path"
grep -Fqx "$device_z" "$stdout_path"
grep -Eq '^warning: found [2-9][0-9]* Edgeless boot disks;' "$stderr_path"

"$eli" --bootdisk "$device_a/" bootdisk get > "$stdout_path" 2> "$stderr_path"
grep -Fqx "$device_a" "$stdout_path"
[[ ! -s "$stderr_path" ]]

kernel_wim="$test_root/kernel-local.wim"
printf 'MSWIM\0\0\0kernel payload' > "$kernel_wim"
if "$eli" kernel store "$kernel_wim" --name 'Edgeless_Beta_Ofial_4.1.0_2.wim' > "$stdout_path" 2> "$stderr_path"; then
    echo 'Ambiguous kernel storage unexpectedly succeeded.' >&2
    exit 1
fi
[[ ! -e "$mount_a/kernel-local.wim" ]]
grep -Fq -- '--bootdisk' "$stderr_path"
"$eli" --bootdisk "$mount_a" kernel store "$kernel_wim" --name 'Edgeless_Beta_Ofial_4.1.0_2.wim' > "$stdout_path" 2> "$stderr_path"
cmp -s "$kernel_wim" "$mount_a/kernel-local.wim"

if "$eli" config set DisablePinBrowsers true > "$stdout_path" 2> "$stderr_path"; then
    echo 'Config modification without an explicit disk unexpectedly succeeded.' >&2
    exit 1
fi
[[ ! -e "$mount_a/Edgeless/Config/DisablePinBrowsers" ]]
grep -Fq -- '--bootdisk' "$stderr_path"

"$eli" --bootdisk "$mount_a" config set DisablePinBrowsers true > "$stdout_path" 2> "$stderr_path"
[[ -d "$mount_a/Edgeless/Config/DisablePinBrowsers" ]]
"$eli" --bootdisk "$mount_a" config list > "$stdout_path" 2> "$stderr_path"
grep -Eq '^DisablePinBrowsers +true +Yes$' "$stdout_path"
"$eli" --bootdisk "$mount_a" config set DisablePinBrowsers false > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$mount_a/Edgeless/Config/DisablePinBrowsers" ]]

if "$eli" --bootdisk "$mount_a" config set Developer true > "$stdout_path" 2> "$stderr_path"; then
    echo 'Version-incompatible config unexpectedly succeeded.' >&2
    exit 1
fi
grep -Fq 'unavailable' "$stderr_path"
"$eli" --bootdisk "$mount_a" config set resolution 'w1920 h1080 b32 f60' > "$stdout_path" 2> "$stderr_path"
[[ "$(cat "$mount_a/Edgeless/Config/分辨率.txt")" == 'w1920 h1080 b32 f60' ]]
"$eli" --bootdisk "$mount_a" config set resolution 'w1080 h1920 b32 f60' --skip-resolution-validation > "$stdout_path" 2> "$stderr_path"
[[ "$(cat "$mount_a/Edgeless/Config/分辨率.txt")" == 'w1080 h1920 b32 f60' ]]
"$eli" --bootdisk "$mount_a" config set homepage example.com > "$stdout_path" 2> "$stderr_path"
[[ "$(cat "$mount_a/Edgeless/Config/HomePage.txt")" == 'http://example.com' ]]
"$eli" --bootdisk "$mount_a" config set resolution auto > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$mount_a/Edgeless/Config/分辨率.txt" ]]
"$eli" --bootdisk "$mount_a" config set homepage false > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$mount_a/Edgeless/Config/HomePage.txt" ]]
wallpaper_path="$test_root/wallpaper.jpg"
printf '\xff\xd8\xff\xd9' > "$wallpaper_path"
"$eli" --bootdisk "$mount_a" config set wallpaper "$wallpaper_path" > "$stdout_path" 2> "$stderr_path"
cmp "$wallpaper_path" "$mount_a/Edgeless/wp.jpg"

package_path="$test_root/工具箱_1.0.0_Edgeless.7z"
printf '%s' 'stored-package' > "$package_path"
if "$eli" plugin store "$package_path" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Plugin storage without an explicit disk unexpectedly succeeded.' >&2
    exit 1
fi
[[ ! -e "$mount_a/Edgeless/Resource/工具箱_1.0.0_Edgeless.7z" ]]
[[ ! -e "$mount_z/Edgeless/Resource/工具箱_1.0.0_Edgeless.7z" ]]
grep -Fq -- '--bootdisk' "$stderr_path"

"$eli" --bootdisk "$mount_a" plugin store "$package_path" > "$stdout_path" 2> "$stderr_path"
cmp "$package_path" "$mount_a/Edgeless/Resource/工具箱_1.0.0_Edgeless.7z"
[[ ! -s "$stderr_path" ]]

"$eli" plugin outdate '工具箱_1.0.0_Edgeless' --bootdisk "$mount_a" > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$mount_a/Edgeless/Resource/工具箱_1.0.0_Edgeless.7z" ]]
[[ -f "$mount_a/Edgeless/Resource/过期插件包/工具箱_1.0.0_Edgeless.7zf" ]]
[[ ! -s "$stderr_path" ]]

"$eli" --bootdisk "$mount_a" plugin list > "$stdout_path" 2> "$stderr_path"
grep -Eq '^Name +Version +Author +Attribute +AutoBuild$' "$stdout_path"
grep -Eq '^搜狗拼音 +16\.4\.0\.0 +Cno +Normal +Yes$' "$stdout_path"
[[ ! -s "$stderr_path" ]]

"$eli" --bootdisk "$mount_a" plugin attr '搜狗拼音_16.4.0.0_Cno（bot）' Frozen > "$stdout_path" 2> "$stderr_path"
[[ -f "$mount_a/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7zf" ]]
[[ ! -e "$mount_a/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z" ]]
[[ ! -s "$stderr_path" ]]
"$eli" --bootdisk "$mount_a" plugin list > "$stdout_path" 2> "$stderr_path"
grep -Eq '^搜狗拼音 +16\.4\.0\.0 +Cno +Frozen +Yes$' "$stdout_path"

"$eli" plugin attr '搜狗拼音_16.4.0.0_Cno（bot）.7zf' Normal --bootdisk "$mount_a" > "$stdout_path" 2> "$stderr_path"
[[ -f "$mount_a/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z" ]]
[[ ! -e "$mount_a/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7zf" ]]
[[ ! -s "$stderr_path" ]]

if "$eli" plugin delete '搜狗拼音_16.4.0.0_Cno（bot）.7z' > "$stdout_path" 2> "$stderr_path"; then
    echo 'Plugin deletion without an explicit disk unexpectedly succeeded.' >&2
    exit 1
fi
[[ -f "$mount_a/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z" ]]
[[ -f "$mount_z/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z" ]]
grep -Fq -- '--bootdisk' "$stderr_path"

"$eli" --bootdisk "$mount_a" plugin delete '搜狗拼音_16.4.0.0_Cno（bot）.7z' > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$mount_a/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z" ]]
[[ ! -s "$stderr_path" ]]

"$eli" plugin delete '搜狗拼音_16.4.0.0_Cno（bot）' --bootdisk "$mount_z" > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$mount_z/Edgeless/Resource/搜狗拼音_16.4.0.0_Cno（bot）.7z" ]]
[[ ! -s "$stderr_path" ]]
