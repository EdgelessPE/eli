$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$tempBase = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    [System.IO.Path]::GetTempPath()
}
else {
    $env:RUNNER_TEMP
}
$resolvedTempBase = [System.IO.Path]::GetFullPath($tempBase).TrimEnd('\') + '\'
$resolvedTestRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $resolvedTempBase "eli-e2e-$([guid]::NewGuid())")
)
if (-not $resolvedTestRoot.StartsWith(
        $resolvedTempBase,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
    throw "Unsafe E2E temporary path: $resolvedTestRoot"
}

$usedDrives = [System.IO.DriveInfo]::GetDrives().Name
$letters = @(90..65 |
        ForEach-Object { [char]$_ } |
        Where-Object { $usedDrives -notcontains "${_}:\" } |
        Select-Object -First 2)
if ($letters.Count -ne 2) {
    throw 'Two free drive letters are required for the E2E test.'
}

$drives = @($letters | ForEach-Object { "${_}:" })
$driveRoots = @($drives | ForEach-Object { "$_\" })
$versions = @('Edgeless_Alpa_4.1.2', 'Edgeless_Beta_Ofial_4.1.0_2')
$substDrives = [System.Collections.Generic.List[string]]::new()
New-Item -ItemType Directory -Path $resolvedTestRoot | Out-Null

Push-Location $repoRoot
try {
    cargo +stable test --quiet --package eli-lib --test version_identifier
    if ($LASTEXITCODE -ne 0) { throw 'Version identifier E2E test failed.' }

    for ($index = 0; $index -lt $drives.Count; $index++) {
        $backingPath = Join-Path $resolvedTestRoot "disk-$index"
        New-Item -ItemType Directory -Path $backingPath | Out-Null
        subst $drives[$index] $backingPath
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to create temporary drive $($drives[$index])."
        }
        $substDrives.Add($drives[$index])

        $edgelessPath = Join-Path $driveRoots[$index] 'Edgeless'
        New-Item -ItemType Directory -Path $edgelessPath | Out-Null
        Set-Content -NoNewline `
            -Path (Join-Path $edgelessPath 'version.txt') `
            -Value $versions[$index]
        $resourcePath = Join-Path $edgelessPath 'Resource'
        New-Item -ItemType Directory -Path $resourcePath | Out-Null
        Set-Content -NoNewline `
            -Path (Join-Path $resourcePath '搜狗拼音_16.4.0.0_Cno（bot）.7z') `
            -Value 'package'
    }

    cargo +stable build --quiet --package eli-cli
    if ($LASTEXITCODE -ne 0) { throw 'Failed to build eli.' }
    $eli = Join-Path $repoRoot 'target\debug\eli.exe'
    $stdoutPath = Join-Path $resolvedTestRoot 'stdout.txt'
    $stderrPath = Join-Path $resolvedTestRoot 'stderr.txt'
    $nesPakSource = Join-Path $resolvedTestRoot 'NesPak.7z'
    Set-Content -NoNewline -Path $nesPakSource -Value 'nespak'

    $loadscreenSource = Join-Path $resolvedTestRoot 'loadscreen.png'
    $loadscreenOutput = Join-Path $resolvedTestRoot 'loadscreen.tar'
    $loadscreenExtracted = Join-Path $resolvedTestRoot 'loadscreen-extracted'
    [System.IO.File]::WriteAllBytes(
        $loadscreenSource,
        [Convert]::FromBase64String(
            'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
        )
    )
    $playHelp = (& $eli loadscreen play --help 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0 -or -not $playHelp.Contains('--demo <IMAGE>')) {
        throw "Loadscreen play demo command is unavailable: '$playHelp'."
    }
    & $eli loadscreen bake $loadscreenSource -o $loadscreenOutput `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) {
        throw "Loadscreen baking failed: '$(Get-Content -Raw -LiteralPath $stderrPath)'."
    }
    if (-not (Get-Content -Raw -LiteralPath $stdoutPath).Contains('quality 90')) {
        throw 'Loadscreen baking did not use the default WebP quality 90.'
    }
    $expectedLoadscreenFiles = @(
        'lsbp_0000.webp', 'lsbp_0040.webp', 'lsbp_0080.webp',
        'lsbp_0120.webp', 'lsbp_0160.webp', 'lsbp_0200.webp',
        'lsbp_0240.webp', 'lsbp_0280.webp', 'lsbp_0320.webp',
        'lsbp_0360.webp', 'lsbp_0400.webp', 'lsbp_0440.webp',
        'lsbp_0480.webp', 'lsbp_0520.webp', 'lsbp_0560.webp',
        'lsbp_0600.webp', 'lsbp_0640.webp', 'lsbp_0680.webp',
        'lsbp_0720.webp', 'lsbp_0760.webp', 'lsbp_0800.webp',
        'lsbp_0840.webp', 'lsbp_0880.webp', 'lsbp_0920.webp',
        'lsbp_0960.webp', 'lsbp_1000.webp'
    )
    New-Item -ItemType Directory -Path $loadscreenExtracted | Out-Null
    & tar -xf $loadscreenOutput -C $loadscreenExtracted
    if ($LASTEXITCODE -ne 0) {
        throw 'Loadscreen output is not a valid tar archive.'
    }
    $actualLoadscreenFiles = @(Get-ChildItem -LiteralPath $loadscreenExtracted -File)
    if ($actualLoadscreenFiles.Count -ne 26) {
        throw "Expected 26 baked loadscreen images, got $($actualLoadscreenFiles.Count)."
    }
    foreach ($fileName in $expectedLoadscreenFiles) {
        $outputFile = Join-Path $loadscreenExtracted $fileName
        if (-not (Test-Path -LiteralPath $outputFile -PathType Leaf)) {
            throw "Missing baked loadscreen image $fileName."
        }
        $bytes = [System.IO.File]::ReadAllBytes($outputFile)
        if ([Text.Encoding]::ASCII.GetString($bytes, 0, 4) -ne 'RIFF' -or
                [Text.Encoding]::ASCII.GetString($bytes, 8, 4) -ne 'WEBP') {
            throw "Baked loadscreen image $fileName is not WebP."
        }
    }
    $loadscreenProgress = Get-Content -Raw -LiteralPath $stderrPath
    if (-not ([regex]::IsMatch(
                $loadscreenProgress,
                'Baking 26 loadscreen images with [1-9][0-9]* jobs?'
            ) -and
            $loadscreenProgress.Contains('lsbp_1000.webp completed') -and
            $loadscreenProgress.Contains('正在读取并解码输入图片') -and
            $loadscreenProgress.Contains('正在准备输出文件和临时工作区') -and
            $loadscreenProgress.Contains('正在检查透明度并准备像素缓冲区'))) {
        throw "Loadscreen baking did not report job progress: '$loadscreenProgress'."
    }
    if (Test-Path -LiteralPath (Join-Path $resolvedTestRoot '.loadscreen.tar.eli-loadscreen-bake.lock')) {
        throw 'Loadscreen baking left its destination lock file behind.'
    }

    $singleJobOutput = Join-Path $resolvedTestRoot 'loadscreen-single-job.tar'
    & $eli loadscreen bake $loadscreenSource -o $singleJobOutput -s 1 -j 1 -q 75 `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            -not (Get-Content -Raw -LiteralPath $stderrPath).Contains(
                'Baking 2 loadscreen images with 1 job'
            ) -or
            -not (Get-Content -Raw -LiteralPath $stdoutPath).Contains('quality 75')) {
        throw 'Loadscreen baking did not honor explicit job and quality options.'
    }

    $largeLoadscreenSource = Join-Path $resolvedTestRoot 'loadscreen-large.bmp'
    $largeBitmap = [byte[]]::new(16442)
    [Text.Encoding]::ASCII.GetBytes('BM').CopyTo($largeBitmap, 0)
    [BitConverter]::GetBytes([uint32]16442).CopyTo($largeBitmap, 2)
    [BitConverter]::GetBytes([uint32]54).CopyTo($largeBitmap, 10)
    [BitConverter]::GetBytes([uint32]40).CopyTo($largeBitmap, 14)
    [BitConverter]::GetBytes([int32]4097).CopyTo($largeBitmap, 18)
    [BitConverter]::GetBytes([int32]1).CopyTo($largeBitmap, 22)
    [BitConverter]::GetBytes([uint16]1).CopyTo($largeBitmap, 26)
    [BitConverter]::GetBytes([uint16]32).CopyTo($largeBitmap, 28)
    [BitConverter]::GetBytes([uint32]16388).CopyTo($largeBitmap, 34)
    [System.IO.File]::WriteAllBytes($largeLoadscreenSource, $largeBitmap)

    $downsampledOutput = Join-Path $resolvedTestRoot 'loadscreen-downsampled.tar'
    & $eli loadscreen bake $largeLoadscreenSource -o $downsampledOutput -s 1 -j 1 -q 0 `
        1> $stdoutPath 2> $stderrPath
    $downsampledStdout = Get-Content -Raw -LiteralPath $stdoutPath
    $downsampledStderr = Get-Content -Raw -LiteralPath $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            -not $downsampledStdout.Contains('(4096x1,') -or
            -not $downsampledStderr.Contains('4097×1') -or
            -not $downsampledStderr.Contains('4096×1')) {
        throw 'Loadscreen baking did not automatically downsample a large input.'
    }

    $originalSizeOutput = Join-Path $resolvedTestRoot 'loadscreen-original-size.tar'
    & $eli loadscreen bake $largeLoadscreenSource -o $originalSizeOutput -s 1 -j 1 -q 0 `
        --no-downsample 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            -not (Get-Content -Raw -LiteralPath $stdoutPath).Contains('(4097x1,') -or
            (Get-Content -Raw -LiteralPath $stderrPath).Contains('正在等比降采样')) {
        throw 'Loadscreen baking did not honor --no-downsample.'
    }

    $concurrentLoadscreenOutput = Join-Path $resolvedTestRoot 'loadscreen-concurrent.tar'
    $bakeProcesses = @(1..2 | ForEach-Object {
            $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
            $startInfo.FileName = $eli
            $startInfo.UseShellExecute = $false
            $startInfo.CreateNoWindow = $true
            $startInfo.RedirectStandardOutput = $true
            $startInfo.RedirectStandardError = $true
            foreach ($argument in @(
                    'loadscreen', 'bake', $loadscreenSource,
                    '-o', $concurrentLoadscreenOutput, '-s', '2'
                )) {
                [void]$startInfo.ArgumentList.Add($argument)
            }
            [System.Diagnostics.Process]::Start($startInfo)
        })
    $bakeExitCodes = @($bakeProcesses | ForEach-Object {
            $_.WaitForExit()
            $exitCode = $_.ExitCode
            $_.Dispose()
            $exitCode
        })
    if (@($bakeExitCodes | Where-Object { $_ -eq 0 }).Count -ne 1) {
        throw "Expected one concurrent loadscreen bake to succeed, got '$($bakeExitCodes -join ', ')'."
    }
    $concurrentExtracted = Join-Path $resolvedTestRoot 'loadscreen-concurrent-extracted'
    New-Item -ItemType Directory -Path $concurrentExtracted | Out-Null
    & tar -xf $concurrentLoadscreenOutput -C $concurrentExtracted
    if ($LASTEXITCODE -ne 0 -or @(Get-ChildItem -LiteralPath $concurrentExtracted -File).Count -ne 3) {
        throw 'Concurrent loadscreen baking published an incomplete result.'
    }
    if (Test-Path -LiteralPath (Join-Path $resolvedTestRoot '.loadscreen-concurrent.tar.eli-loadscreen-bake.lock')) {
        throw 'Concurrent loadscreen baking left its destination lock file behind.'
    }

    $invalidLoadscreenOutput = Join-Path $resolvedTestRoot 'loadscreen.bin'
    & $eli loadscreen bake $loadscreenSource -o $invalidLoadscreenOutput `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or
            -not (Get-Content -Raw -LiteralPath $stderrPath).Contains('.tar') -or
            (Test-Path -LiteralPath $invalidLoadscreenOutput)) {
        throw 'Loadscreen baking accepted an output path without a .tar extension.'
    }

    $loadscreenGif = Join-Path $resolvedTestRoot 'loadscreen.gif'
    [System.IO.File]::WriteAllBytes(
        $loadscreenGif,
        [Convert]::FromBase64String('R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==')
    )
    & $eli loadscreen bake $loadscreenGif -o (Join-Path $resolvedTestRoot 'gif-baked.tar') `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or
            -not (Get-Content -Raw -LiteralPath $stderrPath).Contains('GIF images are not supported')) {
        throw 'Loadscreen baking unexpectedly accepted GIF input.'
    }

    & $eli nespak store $nesPakSource 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'NesPak storage unexpectedly selected one of multiple boot disks.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--bootdisk')) {
        throw 'NesPak storage did not require an explicit boot disk selection.'
    }

    & $eli --bootdisk $driveRoots[0] nespak store $nesPakSource 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) {
        throw "NesPak storage failed: '$(Get-Content -Raw -LiteralPath $stderrPath)'."
    }
    $storedNesPak = Join-Path $driveRoots[0] 'Edgeless\Nes_Inport.7z'
    if ((Get-Content -Raw -LiteralPath $storedNesPak) -ne 'nespak') {
        throw 'NesPak storage did not write the expected component archive.'
    }
    Set-Content -NoNewline -Path $nesPakSource -Value 'updated nespak'
    Set-Content -NoNewline -Path "${storedNesPak}bak" -Value 'stale backup'
    & $eli --bootdisk $driveRoots[0] nespak store $nesPakSource 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) {
        throw "NesPak replacement storage failed: '$(Get-Content -Raw -LiteralPath $stderrPath)'."
    }
    if ((Get-Content -Raw -LiteralPath $storedNesPak) -ne 'updated nespak' -or
        (Get-Content -Raw -LiteralPath "${storedNesPak}bak") -ne 'nespak') {
        throw 'NesPak storage did not preserve the prior archive as its replacement backup.'
    }

    & $eli plugin load (Join-Path $resolvedTestRoot 'plugin.7z') 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Plugin loading unexpectedly accepted WindowsNormal.'
    }
    $loadEnvironmentError = Get-Content -Raw -LiteralPath $stderrPath
    if (-not ($loadEnvironmentError.Contains('WindowsPE') -and
            $loadEnvironmentError.Contains('WindowsNormal'))) {
        throw "Plugin loading did not report its environment dependency: '$loadEnvironmentError'."
    }

    & $eli plugin load --gui (Join-Path $resolvedTestRoot 'plugin.7z') `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Plugin loading GUI unexpectedly accepted WindowsNormal.'
    }
    $guiEnvironmentError = Get-Content -Raw -LiteralPath $stderrPath
    if (-not ($guiEnvironmentError.Contains('WindowsPE') -and
            $guiEnvironmentError.Contains('WindowsNormal'))) {
        throw "Plugin loading GUI did not report its environment dependency: '$guiEnvironmentError'."
    }

    & $eli plugin localboost load (Join-Path $resolvedTestRoot 'plugin.7zl') `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'LocalBoost loading unexpectedly accepted WindowsNormal.'
    }
    $localBoostEnvironmentError = Get-Content -Raw -LiteralPath $stderrPath
    if (-not ($localBoostEnvironmentError.Contains('WindowsPE') -and
            $localBoostEnvironmentError.Contains('WindowsNormal'))) {
        throw "LocalBoost loading did not report its environment dependency: '$localBoostEnvironmentError'."
    }

    foreach ($localBoostArguments in @(,@('startup'), @('clean', '--all'))) {
        & $eli plugin localboost @localBoostArguments 1> $stdoutPath 2> $stderrPath
        if ($LASTEXITCODE -eq 0) {
            throw "LocalBoost $($localBoostArguments -join ' ') unexpectedly accepted WindowsNormal."
        }
        $localBoostEnvironmentError = Get-Content -Raw -LiteralPath $stderrPath
        if (-not ($localBoostEnvironmentError.Contains('WindowsPE') -and
                $localBoostEnvironmentError.Contains('WindowsNormal'))) {
            throw "LocalBoost command did not report its environment dependency: '$localBoostEnvironmentError'."
        }
    }

    & $eli nespak load (Join-Path $resolvedTestRoot 'NesPak.7z') 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'NesPak loading unexpectedly accepted WindowsNormal.'
    }
    $nesPakEnvironmentError = Get-Content -Raw -LiteralPath $stderrPath
    if (-not ($nesPakEnvironmentError.Contains('WindowsPE') -and
            $nesPakEnvironmentError.Contains('WindowsNormal'))) {
        throw "NesPak loading did not report its environment dependency: '$nesPakEnvironmentError'."
    }

    & $eli kernel version current 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Current kernel version unexpectedly accepted WindowsNormal.'
    }
    $currentKernelError = Get-Content -Raw -LiteralPath $stderrPath
    if (-not ($currentKernelError.Contains('WindowsPE') -and
            $currentKernelError.Contains('WindowsNormal'))) {
        throw "Current kernel version did not report its environment dependency: '$currentKernelError'."
    }

    & $eli kernel download 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Kernel download unexpectedly accepted a missing directory.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--directory')) {
        throw 'Kernel download did not require --directory.'
    }

    & $eli bootdisk list 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Boot-disk listing failed.' }
    $listed = @(Get-Content -LiteralPath $stdoutPath)
    $bootdiskWidth = 40
    $versionWidth = 12
    $rowFormat = "{0,-$bootdiskWidth}{1,-$versionWidth}{2}"
    $expectedHeader = $rowFormat -f 'Bootdisk', 'Version', 'Release'
    $expectedAlpha = $rowFormat -f $driveRoots[0], '4.1.2', 'Alpha'
    $expectedBeta = $rowFormat -f $driveRoots[1], '4.1.0', 'Beta(Official)'
    if ($listed -notcontains $expectedHeader -or
            $listed -notcontains $expectedAlpha -or
            $listed -notcontains $expectedBeta) {
        throw "Expected normalized versions in boot-disk list, got '$($listed -join '; ')'."
    }
    if (-not [string]::IsNullOrWhiteSpace((Get-Content -Raw -LiteralPath $stderrPath))) {
        throw 'Boot-disk listing unexpectedly emitted an error.'
    }

    $hookSource = Join-Path $resolvedTestRoot 'save-state.cmd'
    Set-Content -NoNewline -LiteralPath $hookSource -Value '@echo off'
    & $eli hook add customStage $hookSource 1> $stdoutPath 2> $stderrPath
    $invalidHookError = Get-Content -Raw -LiteralPath $stderrPath
    if ($LASTEXITCODE -eq 0 -or
            -not $invalidHookError.Contains("invalid value 'customStage'") -or
            -not $invalidHookError.Contains('onDiskFound') -or
            -not $invalidHookError.Contains('onExit')) {
        throw 'Hook addition did not reject an undocumented hook stage with the documented choices.'
    }

    & $eli hook add onExit $hookSource 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Hook addition unexpectedly selected one of multiple boot disks.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--bootdisk')) {
        throw 'Hook addition did not require an explicit boot disk selection.'
    }

    & $eli --bootdisk $driveRoots[0] hook add onExit $hookSource `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Hook addition failed.' }
    $storedHook = Join-Path $driveRoots[0] 'Edgeless\Hooks\onExit\save-state.cmd'
    if ((Get-Content -Raw -LiteralPath $storedHook) -ne '@echo off') {
        throw 'Hook addition did not preserve the script.'
    }
    $undocumentedHookDirectory = Join-Path $driveRoots[0] 'Edgeless\Hooks\customStage'
    New-Item -ItemType Directory -Force -Path $undocumentedHookDirectory | Out-Null
    Set-Content -NoNewline -LiteralPath (Join-Path $undocumentedHookDirectory 'ignored.cmd') `
        -Value '@echo off'

    & $eli --bootdisk $driveRoots[0] hook list 1> $stdoutPath 2> $stderrPath
    $hookListOutput = Get-Content -Raw -LiteralPath $stdoutPath
    if ($LASTEXITCODE -ne 0 -or
            -not [regex]::IsMatch($hookListOutput, '(?m)^onExit +save-state\.cmd\r?$') -or
            $hookListOutput.Contains('customStage') -or
            $hookListOutput.Contains('ignored.cmd')) {
        throw 'Hook listing did not include valid scripts or exposed an undocumented hook stage.'
    }

    & $eli hook remove onExit save-state.cmd 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or -not (Test-Path -LiteralPath $storedHook)) {
        throw 'Hook removal unexpectedly selected one of multiple boot disks.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--bootdisk')) {
        throw 'Hook removal did not require an explicit boot disk selection.'
    }

    & $eli --bootdisk $driveRoots[0] hook remove onExit save-state.cmd `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath $storedHook)) {
        throw 'Hook removal failed.'
    }

    & $eli hook call onExit --dictionary $resolvedTestRoot --policy async `
        1> $stdoutPath 2> $stderrPath
    $hookCallError = Get-Content -Raw -LiteralPath $stderrPath
    if ($LASTEXITCODE -eq 0 -or
            -not ($hookCallError.Contains('WindowsPE') -and
                $hookCallError.Contains('WindowsNormal'))) {
        throw 'Hook call did not reject WindowsNormal before execution.'
    }

    & $eli --bootdisk $driveRoots[0] kernel version bootdisk 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            (@(Get-Content -LiteralPath $stdoutPath) -join "`n") -ne
            (("{0,-$versionWidth}{1}" -f 'Version', 'Release') + "`n" +
                ("{0,-$versionWidth}{1}" -f '4.1.2', 'Alpha')) -or
            -not [string]::IsNullOrWhiteSpace((Get-Content -Raw -LiteralPath $stderrPath))) {
        throw 'Boot-disk kernel version did not return the identified selected version.'
    }

    [System.IO.File]::WriteAllBytes(
        (Join-Path $driveRoots[0] 'Edgeless_Alpha_4.1.3.wim'),
        [byte[]](0x4d, 0x53, 0x57, 0x49, 0x4d, 0, 0, 0, 0x61, 0x6c, 0x70, 0x68, 0x61)
    )
    & $eli --bootdisk $driveRoots[0] kernel alpha version bootdisk 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            (@(Get-Content -LiteralPath $stdoutPath) -join "`n") -ne
            (("{0,-$versionWidth}{1}" -f 'Version', 'Release') + "`n" +
                ("{0,-$versionWidth}{1}" -f '4.1.3', 'Alpha')) -or
            -not [string]::IsNullOrWhiteSpace((Get-Content -Raw -LiteralPath $stderrPath))) {
        throw 'Alpha boot-disk kernel version did not return the highest Alpha WIM version.'
    }

    & $eli kernel alpha version latest 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or -not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--token')) {
        throw 'Alpha latest version did not require a token.'
    }

    & $eli kernel alpha --token test download 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or
            -not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--directory')) {
        throw 'Alpha download did not require a directory.'
    }

    & $eli bootdisk get 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Automatic boot-disk selection failed.' }
    $selected = (Get-Content -Raw -LiteralPath $stdoutPath).TrimEnd()
    $warning = Get-Content -Raw -LiteralPath $stderrPath
    if ($selected -ne $drives[0]) {
        throw "Expected automatic selection '$($drives[0])', got '$selected'."
    }
    if (-not ($warning.Contains('warning: found ') -and
            $warning.Contains(' Edgeless boot disks'))) {
        throw "Expected a multiple-candidate warning, got '$warning'."
    }

    & $eli bootdisk get -b $driveRoots[1] 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Explicit boot-disk selection failed.' }
    $selected = (Get-Content -Raw -LiteralPath $stdoutPath).TrimEnd()
    $warning = Get-Content -Raw -LiteralPath $stderrPath
    if ($selected -ne $drives[1]) {
        throw "Expected explicit selection '$($drives[1])', got '$selected'."
    }
    if (-not [string]::IsNullOrWhiteSpace($warning)) {
        throw "Explicit selection unexpectedly emitted a warning: '$warning'."
    }

    $kernelWimPath = Join-Path $resolvedTestRoot 'kernel-local.wim'
    [System.IO.File]::WriteAllBytes(
        $kernelWimPath,
        [byte[]](0x4d, 0x53, 0x57, 0x49, 0x4d, 0, 0, 0, 0x6b, 0x65, 0x72, 0x6e, 0x65, 0x6c)
    )
    & $eli kernel store $kernelWimPath --name 'Edgeless_Beta_Ofial_4.1.0_2.wim' `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Ambiguous kernel storage unexpectedly succeeded.'
    }
    $kernelDestination = Join-Path $driveRoots[0] 'kernel-local.wim'
    if (Test-Path -LiteralPath $kernelDestination) {
        throw 'Ambiguous kernel storage changed a boot disk.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--bootdisk')) {
        throw 'Ambiguous kernel storage did not suggest --bootdisk.'
    }
    & $eli --bootdisk $driveRoots[0] kernel store $kernelWimPath `
        --name 'Edgeless_Beta_Ofial_4.1.0_2.wim' 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($kernelWimPath)) -ne
            [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($kernelDestination))) {
        throw 'Kernel WIM storage did not preserve the source content.'
    }

    & $eli config set DisablePinBrowsers true 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Config modification without an explicit disk unexpectedly succeeded.'
    }
    if (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Config\DisablePinBrowsers')) {
        throw 'Ambiguous config modification changed a boot disk.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--bootdisk')) {
        throw 'Ambiguous config modification did not suggest --bootdisk.'
    }

    & $eli --bootdisk $driveRoots[0] config set DisablePinBrowsers true `
        1> $stdoutPath 2> $stderrPath
    $markerPath = Join-Path $driveRoots[0] 'Edgeless\Config\DisablePinBrowsers'
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $markerPath)) {
        throw 'Enabling a boolean config failed.'
    }
    & $eli --bootdisk $driveRoots[0] config list 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or -not ((Get-Content -Raw -LiteralPath $stdoutPath) -match 'DisablePinBrowsers\s+true\s+Yes')) {
        throw 'Config list did not report the enabled boolean config.'
    }
    & $eli --bootdisk $driveRoots[0] config set DisablePinBrowsers false `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath $markerPath)) {
        throw 'Disabling a boolean config failed.'
    }

    & $eli --bootdisk $driveRoots[0] config set Developer true 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or -not (Get-Content -Raw -LiteralPath $stderrPath).Contains('unavailable')) {
        throw 'Version-incompatible config unexpectedly succeeded.'
    }

    & $eli --bootdisk $driveRoots[0] config set resolution 'w1920 h1080 b32 f60' `
        1> $stdoutPath 2> $stderrPath
    if ((Get-Content -Raw -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Config\分辨率.txt')) -ne 'w1920 h1080 b32 f60') {
        throw 'Resolution config was not written.'
    }
    & $eli --bootdisk $driveRoots[0] config set resolution 'w1080 h1920 b32 f60' `
        --skip-resolution-validation 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            (Get-Content -Raw -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Config\分辨率.txt')) -ne 'w1080 h1920 b32 f60') {
        throw 'Resolution validation override did not write the requested value.'
    }
    & $eli --bootdisk $driveRoots[0] config set homepage example.com 1> $stdoutPath 2> $stderrPath
    if ((Get-Content -Raw -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Config\HomePage.txt')) -ne 'http://example.com') {
        throw 'Homepage config was not normalized and written.'
    }
    & $eli --bootdisk $driveRoots[0] config set homepage 'https://EXAMPLE.technology/path' `
        1> $stdoutPath 2> $stderrPath
    $homepagePath = Join-Path $driveRoots[0] 'Edgeless\Config\HomePage.txt'
    if ($LASTEXITCODE -ne 0 -or
            (Get-Content -Raw -LiteralPath $homepagePath) -ne 'https://EXAMPLE.technology/path') {
        throw 'A valid HTTP(S) URL was not preserved and written.'
    }
    & $eli --bootdisk $driveRoots[0] config set homepage 'ftp://example.com' `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0 -or
            (Get-Content -Raw -LiteralPath $homepagePath) -ne 'https://EXAMPLE.technology/path') {
        throw 'An unsupported homepage URL changed the existing config.'
    }
    & $eli --bootdisk $driveRoots[0] config set resolution auto 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Config\分辨率.txt'))) {
        throw 'Automatic resolution did not remove the resolution config.'
    }
    & $eli --bootdisk $driveRoots[0] config set homepage false 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Config\HomePage.txt'))) {
        throw 'Disabling homepage did not remove the homepage config.'
    }
    $wallpaperPath = Join-Path $resolvedTestRoot 'wallpaper.jpg'
    Set-Content -AsByteStream -NoNewline -LiteralPath $wallpaperPath -Value ([byte[]](0xFF, 0xD8, 0xFF, 0xD9))
    & $eli --bootdisk $driveRoots[0] config set wallpaper $wallpaperPath 1> $stdoutPath 2> $stderrPath
    $copiedWallpaperPath = Join-Path $driveRoots[0] 'Edgeless\wp.jpg'
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $copiedWallpaperPath) -or
            (Get-FileHash -LiteralPath $wallpaperPath).Hash -ne (Get-FileHash -LiteralPath $copiedWallpaperPath).Hash) {
        throw 'JPEG wallpaper was not copied.'
    }

    $packagePath = Join-Path $resolvedTestRoot '工具箱_1.0.0_Edgeless.7z'
    Set-Content -NoNewline -LiteralPath $packagePath -Value 'stored-package'
    & $eli plugin store $packagePath 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Plugin storage without an explicit disk unexpectedly succeeded.'
    }
    if ((Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Resource\工具箱_1.0.0_Edgeless.7z')) -or
            (Test-Path -LiteralPath (Join-Path $driveRoots[1] 'Edgeless\Resource\工具箱_1.0.0_Edgeless.7z'))) {
        throw 'Ambiguous plugin storage modified a boot disk.'
    }
    if (-not (Get-Content -Raw -LiteralPath $stderrPath).Contains('--bootdisk')) {
        throw 'Ambiguous plugin storage did not suggest --bootdisk.'
    }

    & $eli --bootdisk $driveRoots[0] plugin store $packagePath 1> $stdoutPath 2> $stderrPath
    $storedPath = Join-Path $driveRoots[0] 'Edgeless\Resource\工具箱_1.0.0_Edgeless.7z'
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $storedPath)) {
        throw 'Plugin storage failed.'
    }
    if ((Get-Content -Raw -LiteralPath $storedPath) -ne 'stored-package') {
        throw 'Stored plugin content was changed.'
    }

    & $eli plugin outdate '工具箱_1.0.0_Edgeless' --bootdisk $driveRoots[0] `
        1> $stdoutPath 2> $stderrPath
    $outdatedPath = Join-Path $driveRoots[0] 'Edgeless\Resource\过期插件包\工具箱_1.0.0_Edgeless.7zf'
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $outdatedPath) -or
            (Test-Path -LiteralPath $storedPath)) {
        throw 'Marking a plugin package as outdated failed.'
    }

    & $eli --bootdisk $driveRoots[0] plugin list 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Plugin listing failed.' }
    $pluginRows = @(Get-Content -LiteralPath $stdoutPath)
    if ($pluginRows.Count -ne 2 -or
            $pluginRows[0] -notmatch '^Name\s+Version\s+Author\s+Attribute\s+AutoBuild$' -or
            $pluginRows[1] -notmatch '^搜狗拼音\s+16\.4\.0\.0\s+Cno\s+Normal\s+Yes$') {
        throw "Unexpected plugin list: '$($pluginRows -join '; ')'."
    }
    if (-not [string]::IsNullOrWhiteSpace((Get-Content -Raw -LiteralPath $stderrPath))) {
        throw 'Plugin listing unexpectedly emitted a warning.'
    }

    & $eli --bootdisk $driveRoots[0] plugin attr '搜狗拼音_16.4.0.0_Cno（bot）' Frozen `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            -not (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Resource\搜狗拼音_16.4.0.0_Cno（bot）.7zf'))) {
        throw 'Changing plugin attribute to Frozen failed.'
    }
    if (-not [string]::IsNullOrWhiteSpace((Get-Content -Raw -LiteralPath $stderrPath))) {
        throw 'Changing plugin attribute to Frozen unexpectedly emitted a warning.'
    }
    & $eli --bootdisk $driveRoots[0] plugin list 1> $stdoutPath 2> $stderrPath
    $frozenRows = @(Get-Content -LiteralPath $stdoutPath)
    if ($LASTEXITCODE -ne 0 -or
            $frozenRows[1] -notmatch '^搜狗拼音\s+16\.4\.0\.0\s+Cno\s+Frozen\s+Yes$') {
        throw "Plugin list did not report the Frozen attribute: '$($frozenRows -join '; ')'."
    }

    & $eli plugin attr '搜狗拼音_16.4.0.0_Cno（bot）.7zf' Normal --bootdisk $driveRoots[0] `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0 -or
            -not (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Resource\搜狗拼音_16.4.0.0_Cno（bot）.7z'))) {
        throw 'Changing plugin attribute back to Normal failed.'
    }
    if (-not [string]::IsNullOrWhiteSpace((Get-Content -Raw -LiteralPath $stderrPath))) {
        throw 'Changing plugin attribute back to Normal unexpectedly emitted a warning.'
    }

    & $eli plugin delete '搜狗拼音_16.4.0.0_Cno（bot）.7z' 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -eq 0) {
        throw 'Plugin deletion without an explicit disk unexpectedly succeeded.'
    }
    if (-not (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Resource\搜狗拼音_16.4.0.0_Cno（bot）.7z')) -or
            -not (Test-Path -LiteralPath (Join-Path $driveRoots[1] 'Edgeless\Resource\搜狗拼音_16.4.0.0_Cno（bot）.7z'))) {
        throw 'Ambiguous plugin deletion modified a boot disk.'
    }
    $errorOutput = Get-Content -Raw -LiteralPath $stderrPath
    if (-not $errorOutput.Contains('--bootdisk')) {
        throw "Ambiguous plugin deletion did not suggest --bootdisk: '$errorOutput'."
    }

    & $eli --bootdisk $driveRoots[0] plugin delete '搜狗拼音_16.4.0.0_Cno（bot）.7z' `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Plugin deletion by complete file name failed.' }
    if (Test-Path -LiteralPath (Join-Path $driveRoots[0] 'Edgeless\Resource\搜狗拼音_16.4.0.0_Cno（bot）.7z')) {
        throw 'Plugin deletion by complete file name did not remove the package.'
    }

    & $eli plugin delete '搜狗拼音_16.4.0.0_Cno（bot）' --bootdisk $driveRoots[1] `
        1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Plugin deletion by stem failed.' }
    if (Test-Path -LiteralPath (Join-Path $driveRoots[1] 'Edgeless\Resource\搜狗拼音_16.4.0.0_Cno（bot）.7z')) {
        throw 'Plugin deletion by stem did not remove the package.'
    }
}
finally {
    Pop-Location
    foreach ($drive in $substDrives) {
        subst $drive /D
    }
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}
