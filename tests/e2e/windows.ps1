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

$version = 'eli-e2e-windows'
$usedDrives = [System.IO.DriveInfo]::GetDrives().Name
$letter = 90..80 |
    ForEach-Object { [char]$_ } |
    Where-Object { $usedDrives -notcontains "${_}:\" } |
    Select-Object -First 1
if ($null -eq $letter) {
    throw 'No free drive letter is available for the E2E test.'
}

$drive = "${letter}:"
$driveRoot = "$drive\"
$substCreated = $false
New-Item -ItemType Directory -Path $resolvedTestRoot | Out-Null

Push-Location $repoRoot
try {
    subst $drive $resolvedTestRoot
    if ($LASTEXITCODE -ne 0) {
        throw 'Failed to create the temporary drive.'
    }
    $substCreated = $true

    New-Item -ItemType Directory -Path (Join-Path $driveRoot 'Edgeless') | Out-Null
    Set-Content -NoNewline `
        -Path (Join-Path $driveRoot 'Edgeless\version.txt') `
        -Value $version

    $output = cargo +stable run --quiet --package eli-cli -- bootdisk list
    if ($LASTEXITCODE -ne 0) {
        throw 'eli bootdisk list failed.'
    }
    $expected = "$driveRoot`t$version"
    if ($output -notcontains $expected) {
        throw "Expected '$expected' in output:`n$($output -join "`n")"
    }
}
finally {
    Pop-Location
    if ($substCreated) {
        subst $drive /D
    }
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}
