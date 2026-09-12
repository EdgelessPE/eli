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
nespak_source="$test_root/NesPak.7z"
printf '%s' 'nespak' > "$nespak_source"

loadscreen_source="$test_root/loadscreen.png"
loadscreen_output="$test_root/loadscreen.tar"
loadscreen_extracted="$test_root/loadscreen-extracted"
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=' |
    base64 -D > "$loadscreen_source"
"$eli" loadscreen play --help > "$stdout_path" 2> "$stderr_path"
grep -Fq -- '--demo <IMAGE>' "$stdout_path"
if "$eli" loadscreen play --demo "$loadscreen_source" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Loadscreen play unexpectedly started outside Windows.' >&2
    exit 1
fi
grep -Fq 'requires a Windows environment' "$stderr_path"
"$eli" loadscreen bake "$loadscreen_source" -o "$loadscreen_output" > "$stdout_path" 2> "$stderr_path"
grep -Fq 'quality 90' "$stdout_path"
expected_loadscreen_files=(
    lsbp_0000.webp lsbp_0040.webp lsbp_0080.webp lsbp_0120.webp
    lsbp_0160.webp lsbp_0200.webp lsbp_0240.webp lsbp_0280.webp
    lsbp_0320.webp lsbp_0360.webp lsbp_0400.webp lsbp_0440.webp
    lsbp_0480.webp lsbp_0520.webp lsbp_0560.webp lsbp_0600.webp
    lsbp_0640.webp lsbp_0680.webp lsbp_0720.webp lsbp_0760.webp
    lsbp_0800.webp lsbp_0840.webp lsbp_0880.webp lsbp_0920.webp
    lsbp_0960.webp lsbp_1000.webp
)
mkdir "$loadscreen_extracted"
tar -xf "$loadscreen_output" -C "$loadscreen_extracted"
[[ "$(find "$loadscreen_extracted" -maxdepth 1 -type f | wc -l)" -eq 26 ]]
for file_name in "${expected_loadscreen_files[@]}"; do
    output_file="$loadscreen_extracted/$file_name"
    [[ -f "$output_file" ]]
    [[ "$(head -c 4 "$output_file")" == 'RIFF' ]]
    [[ "$(dd if="$output_file" bs=1 skip=8 count=4 2>/dev/null)" == 'WEBP' ]]
done
grep -Eq 'Baking 26 loadscreen images with [1-9][0-9]* jobs?' "$stderr_path"
grep -Fq 'lsbp_1000.webp completed' "$stderr_path"
grep -Fq '正在读取并解码输入图片' "$stderr_path"
grep -Fq '正在准备输出文件和临时工作区' "$stderr_path"
grep -Fq '正在检查透明度并准备像素缓冲区' "$stderr_path"
[[ ! -e "$test_root/.loadscreen.tar.eli-loadscreen-bake.lock" ]]

single_job_output="$test_root/loadscreen-single-job.tar"
"$eli" loadscreen bake "$loadscreen_source" -o "$single_job_output" -s 1 -j 1 -q 75 > "$stdout_path" 2> "$stderr_path"
grep -Fq 'Baking 2 loadscreen images with 1 job' "$stderr_path"
grep -Fq 'quality 75' "$stdout_path"

large_loadscreen_source="$test_root/loadscreen-large.bmp"
printf 'BM\x3a\x40\x00\x00\x00\x00\x00\x00\x36\x00\x00\x00\x28\x00\x00\x00\x01\x10\x00\x00\x01\x00\x00\x00\x01\x00\x20\x00\x00\x00\x00\x00\x04\x40\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00' > "$large_loadscreen_source"
dd if=/dev/zero bs=16388 count=1 2>/dev/null >> "$large_loadscreen_source"
downsampled_output="$test_root/loadscreen-downsampled.tar"
"$eli" loadscreen bake "$large_loadscreen_source" -o "$downsampled_output" -s 1 -j 1 -q 0 > "$stdout_path" 2> "$stderr_path"
grep -Fq '(4096x1,' "$stdout_path"
grep -Fq '4097×1' "$stderr_path"
grep -Fq '4096×1' "$stderr_path"

original_size_output="$test_root/loadscreen-original-size.tar"
"$eli" loadscreen bake "$large_loadscreen_source" -o "$original_size_output" -s 1 -j 1 -q 0 \
    --no-downsample > "$stdout_path" 2> "$stderr_path"
grep -Fq '(4097x1,' "$stdout_path"
if grep -Fq '正在等比降采样' "$stderr_path"; then
    echo 'Loadscreen baking ignored --no-downsample.' >&2
    exit 1
fi

concurrent_loadscreen_output="$test_root/loadscreen-concurrent.tar"
"$eli" loadscreen bake "$loadscreen_source" -o "$concurrent_loadscreen_output" -s 2 \
    > "$test_root/loadscreen-first.stdout" 2> "$test_root/loadscreen-first.stderr" &
first_bake_pid=$!
"$eli" loadscreen bake "$loadscreen_source" -o "$concurrent_loadscreen_output" -s 2 \
    > "$test_root/loadscreen-second.stdout" 2> "$test_root/loadscreen-second.stderr" &
second_bake_pid=$!
successful_bakes=0
if wait "$first_bake_pid"; then
    ((successful_bakes += 1))
fi
if wait "$second_bake_pid"; then
    ((successful_bakes += 1))
fi
[[ "$successful_bakes" -eq 1 ]]
concurrent_loadscreen_extracted="$test_root/loadscreen-concurrent-extracted"
mkdir "$concurrent_loadscreen_extracted"
tar -xf "$concurrent_loadscreen_output" -C "$concurrent_loadscreen_extracted"
[[ "$(find "$concurrent_loadscreen_extracted" -maxdepth 1 -type f | wc -l)" -eq 3 ]]
[[ ! -e "$test_root/.loadscreen-concurrent.tar.eli-loadscreen-bake.lock" ]]

if "$eli" loadscreen bake "$loadscreen_source" -o "$test_root/loadscreen.bin" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Loadscreen baking accepted an output path without a .tar extension.' >&2
    exit 1
fi
grep -Fq '.tar' "$stderr_path"
[[ ! -e "$test_root/loadscreen.bin" ]]

loadscreen_gif="$test_root/loadscreen.gif"
printf '%s' 'R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==' |
    base64 -D > "$loadscreen_gif"
if "$eli" loadscreen bake "$loadscreen_gif" -o "$test_root/gif-baked.tar" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Loadscreen baking unexpectedly accepted GIF input.' >&2
    exit 1
fi
grep -Fq 'GIF images are not supported' "$stderr_path"
[[ ! -e "$test_root/gif-baked.tar" ]]

if "$eli" nespak store "$nespak_source" > "$stdout_path" 2> "$stderr_path"; then
    echo 'NesPak storage unexpectedly selected one of multiple boot disks.' >&2
    exit 1
fi
grep -Fq -- '--bootdisk' "$stderr_path"

"$eli" --bootdisk "$mount_a" nespak store "$nespak_source" > "$stdout_path" 2> "$stderr_path"
[[ "$(cat "$mount_a/Edgeless/Nes_Inport.7z")" == 'nespak' ]]
printf '%s' 'updated nespak' > "$nespak_source"
printf '%s' 'stale backup' > "$mount_a/Edgeless/Nes_Inport.7zbak"
"$eli" --bootdisk "$mount_a" nespak store "$nespak_source" > "$stdout_path" 2> "$stderr_path"
[[ "$(cat "$mount_a/Edgeless/Nes_Inport.7z")" == 'updated nespak' ]]
[[ "$(cat "$mount_a/Edgeless/Nes_Inport.7zbak")" == 'nespak' ]]

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

if "$eli" plugin localboost startup > "$stdout_path" 2> "$stderr_path"; then
    echo 'LocalBoost startup unexpectedly accepted macOS.' >&2
    exit 1
fi
grep -Fq 'WindowsPE' "$stderr_path"
grep -Fq 'MacOS' "$stderr_path"

if "$eli" plugin localboost clean --all > "$stdout_path" 2> "$stderr_path"; then
    echo 'LocalBoost cleanup unexpectedly accepted macOS.' >&2
    exit 1
fi
grep -Fq 'WindowsPE' "$stderr_path"
grep -Fq 'MacOS' "$stderr_path"

if "$eli" nespak load "$test_root/NesPak.7z" > "$stdout_path" 2> "$stderr_path"; then
    echo 'NesPak loading unexpectedly accepted macOS.' >&2
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

printf 'MSWIM\0\0\0alpha payload' > "$mount_a/Edgeless_Alpha_4.1.3.wim"
"$eli" --bootdisk "$mount_a" kernel alpha version bootdisk > "$stdout_path" 2> "$stderr_path"
printf -v expected_alpha_kernel_version '%-*s%s' "$version_width" '4.1.3' 'Alpha'
grep -Fqx "$expected_kernel_header" "$stdout_path"
grep -Fqx "$expected_alpha_kernel_version" "$stdout_path"
[[ ! -s "$stderr_path" ]]

if "$eli" kernel alpha version latest > "$stdout_path" 2> "$stderr_path"; then
    echo 'Alpha latest version unexpectedly accepted a missing token.' >&2
    exit 1
fi
grep -Fq -- '--token' "$stderr_path"

if "$eli" kernel alpha --token test download > "$stdout_path" 2> "$stderr_path"; then
    echo 'Alpha download unexpectedly accepted a missing directory.' >&2
    exit 1
fi
grep -Fq -- '--directory' "$stderr_path"

"$eli" bootdisk get > "$stdout_path" 2> "$stderr_path"
grep -Fqx "$device_z" "$stdout_path"
grep -Eq '^warning: found [2-9][0-9]* Edgeless boot disks;' "$stderr_path"

"$eli" --bootdisk "$device_a/" bootdisk get > "$stdout_path" 2> "$stderr_path"
grep -Fqx "$device_a" "$stdout_path"
[[ ! -s "$stderr_path" ]]

hook_source="$test_root/save-state.cmd"
printf '%s' '@echo off' > "$hook_source"
if "$eli" hook add customStage "$hook_source" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Hook addition unexpectedly accepted an undocumented hook stage.' >&2
    exit 1
fi
grep -Fq "invalid value 'customStage'" "$stderr_path"
grep -Fq 'onDiskFound' "$stderr_path"
grep -Fq 'onExit' "$stderr_path"

if "$eli" hook add onExit "$hook_source" > "$stdout_path" 2> "$stderr_path"; then
    echo 'Hook addition unexpectedly selected one of multiple boot disks.' >&2
    exit 1
fi
grep -Fq -- '--bootdisk' "$stderr_path"

"$eli" --bootdisk "$mount_a" hook add onExit "$hook_source" > "$stdout_path" 2> "$stderr_path"
stored_hook="$mount_a/Edgeless/Hooks/onExit/save-state.cmd"
cmp -s "$hook_source" "$stored_hook"
[[ ! -s "$stderr_path" ]]
mkdir -p "$mount_a/Edgeless/Hooks/customStage"
printf '%s' '@echo off' > "$mount_a/Edgeless/Hooks/customStage/ignored.cmd"

"$eli" --bootdisk "$mount_a" hook list > "$stdout_path" 2> "$stderr_path"
grep -Eq '^onExit +save-state\.cmd$' "$stdout_path"
! grep -Fq 'customStage' "$stdout_path"
! grep -Fq 'ignored.cmd' "$stdout_path"
[[ ! -s "$stderr_path" ]]

if "$eli" hook remove onExit save-state.cmd > "$stdout_path" 2> "$stderr_path"; then
    echo 'Hook removal unexpectedly selected one of multiple boot disks.' >&2
    exit 1
fi
[[ -f "$stored_hook" ]]
grep -Fq -- '--bootdisk' "$stderr_path"

"$eli" --bootdisk "$mount_a" hook remove onExit save-state.cmd > "$stdout_path" 2> "$stderr_path"
[[ ! -e "$stored_hook" ]]
[[ ! -s "$stderr_path" ]]

if "$eli" hook call onExit --dictionary "$test_root" --policy async > "$stdout_path" 2> "$stderr_path"; then
    echo 'Hook calling unexpectedly accepted macOS.' >&2
    exit 1
fi
grep -Fq 'WindowsPE' "$stderr_path"
grep -Fq 'MacOS' "$stderr_path"

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
"$eli" --bootdisk "$mount_a" config set homepage 'https://EXAMPLE.technology/path' > "$stdout_path" 2> "$stderr_path"
[[ "$(cat "$mount_a/Edgeless/Config/HomePage.txt")" == 'https://EXAMPLE.technology/path' ]]
if "$eli" --bootdisk "$mount_a" config set homepage 'ftp://example.com' > "$stdout_path" 2> "$stderr_path"; then
    echo 'Unsupported homepage URL unexpectedly succeeded.' >&2
    exit 1
fi
[[ "$(cat "$mount_a/Edgeless/Config/HomePage.txt")" == 'https://EXAMPLE.technology/path' ]]
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
