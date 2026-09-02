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
    }

    cargo +stable build --quiet --package eli-cli
    if ($LASTEXITCODE -ne 0) { throw 'Failed to build eli.' }
    $eli = Join-Path $repoRoot 'target\debug\eli.exe'
    $stdoutPath = Join-Path $resolvedTestRoot 'stdout.txt'
    $stderrPath = Join-Path $resolvedTestRoot 'stderr.txt'

    & $eli bootdisk list 1> $stdoutPath 2> $stderrPath
    if ($LASTEXITCODE -ne 0) { throw 'Boot-disk listing failed.' }
    $listed = @(Get-Content -LiteralPath $stdoutPath)
    $longestBootdisk = ($driveRoots + @('Bootdisk') |
            ForEach-Object { $_.Length } |
            Measure-Object -Maximum).Maximum
    $bootdiskWidth = $longestBootdisk + 5
    $versionWidth = 'Version'.Length + 5
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
