#Requires -Version 5.1
<#
.SYNOPSIS
    Runs the avatar verification against the release build with a real fresh profile.

.DESCRIPTION
    The product resolves its data directory through the Windows known folder
    (%APPDATA%\net.alanfloyd.desktop), so a genuine "fresh profile" run has to move
    the real profile aside. This wrapper does that behind a verified copy:

      1. stops any running product instance (reporting what it stopped),
      2. copies the real profile to a stash directory and verifies the copy by size
         and SHA-256 before anything is removed,
      3. removes the (now duplicated) real profile so the app creates a new one,
      4. runs scripts/verify-avatar-flow.mjs against that fresh profile,
      5. stops the app, deletes the scratch profile, copies the stash back and
         verifies every restored file against the manifest,
      6. reports both results, and keeps the stash when the restore did not verify.

.PARAMETER ReleaseExe
    Executable to test. Defaults to the release cargo output.

.PARAMETER KeepFixtures
    Keep the generated synthetic fixtures next to the run for inspection.

.PARAMETER KeepEvidence
    Keep the stash directory after a successful restore.

.EXAMPLE
    pwsh -File scripts/verify-avatar-flow.ps1
#>
[CmdletBinding()]
param(
    [string]$ReleaseExe = '',
    [string]$StashRoot = '',
    [switch]$KeepFixtures,
    [switch]$KeepEvidence
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Resolved in the body: `$PSScriptRoot` is not reliably bound while a
# `powershell -File` script binds its parameters.
if ([string]::IsNullOrWhiteSpace($ReleaseExe)) {
    $ReleaseExe = Join-Path $PSScriptRoot '..\src-tauri\target\release\alan-desktop.exe'
}
if ([string]::IsNullOrWhiteSpace($StashRoot)) {
    $StashRoot = Join-Path $env:TEMP ('dtw-avatar-stash-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
}

$profile = Join-Path $env:APPDATA 'net.alanfloyd.desktop'
$stash = Join-Path $StashRoot 'profile'
$manifestPath = Join-Path $StashRoot 'manifest.json'
$nodeScript = Join-Path $PSScriptRoot 'verify-avatar-flow.mjs'
$fixtures = Join-Path $StashRoot 'fixtures'

function Stop-Product {
    foreach ($process in @(Get-Process -Name 'alan-desktop', 'desktop-todo-widget' -ErrorAction SilentlyContinue)) {
        $path = ''
        try { $path = $process.Path } catch { $path = '<unknown>' }
        Write-Host "  stopping pid=$($process.Id) path=$path"
        $process | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 1000
}

function Get-TreeManifest {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return @() }
    # Normalized root: `GetFullPath` expands an 8.3 %TEMP% to the long form that
    # `FullName` reports, so relative keys match between profile and stash.
    $root = [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
    return @(Get-ChildItem -LiteralPath $root -Recurse -File | ForEach-Object {
        [pscustomobject]@{
            Path   = $_.FullName.Substring($root.Length).TrimStart('\')
            Length = $_.Length
            SHA256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        }
    })
}

function Copy-TreeVerified {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    Copy-Item -Path (Join-Path $Source '*') -Destination $Destination -Recurse -Force
    $sourceManifest = Get-TreeManifest -Path $Source
    $copyManifest = Get-TreeManifest -Path $Destination
    $problems = @()
    foreach ($entry in $sourceManifest) {
        $match = $copyManifest | Where-Object { $_.Path -eq $entry.Path }
        if (-not $match) { $problems += "missing in copy: $($entry.Path)"; continue }
        if ($match.Length -ne $entry.Length) { $problems += "size differs: $($entry.Path)"; continue }
        if ($match.SHA256 -ne $entry.SHA256) { $problems += "hash differs: $($entry.Path)"; continue }
    }
    return [pscustomobject]@{ Manifest = $sourceManifest; Problems = $problems }
}

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

if (-not (Test-Path -LiteralPath $nodeScript -PathType Leaf)) { throw "missing $nodeScript" }
if (-not (Test-Path -LiteralPath $ReleaseExe -PathType Leaf)) { throw "release executable not found: $ReleaseExe" }
if ([string]::IsNullOrWhiteSpace($env:APPDATA)) { throw 'APPDATA is not set' }

$profileExisted = Test-Path -LiteralPath $profile -PathType Container
$qaExitCode = 1
$stashed = $false
$originalManifest = @()

try {
    Write-Host 'desktop-todo-widget avatar verification wrapper'
    Write-Host "  profile : $profile"
    Write-Host "  stash   : $stash"
    Write-Host ''

    Stop-Product
    New-Item -ItemType Directory -Path $StashRoot -Force | Out-Null

    if ($profileExisted) {
        $copy = Copy-TreeVerified -Source $profile -Destination $stash
        $originalManifest = $copy.Manifest
        Check 'real profile copied and verified before it is moved aside' ($copy.Problems.Count -eq 0) ($copy.Problems -join '; ')
        if ($copy.Problems.Count -gt 0) { throw 'refusing to touch the real profile: the copy did not verify' }
        $copy.Manifest | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
        Remove-Item -LiteralPath $profile -Recurse -Force
        Check 'real profile moved aside for the fresh-profile run' (-not (Test-Path -LiteralPath $profile))
        $stashed = $true
    }
    else {
        Write-Host '  no existing profile; the run creates one from scratch'
    }

    $nodeArgs = @($nodeScript, '--fresh-profile-ok', '--exe', $ReleaseExe, '--data-dir', $profile, '--fixtures', $fixtures)
    if ($KeepFixtures) { $nodeArgs += '--keep-fixtures' }
    # Node writes progress to stdout and failures to stderr; a failing QA run is an
    # expected outcome and must not abort the wrapper before the profile is restored.
    $ErrorActionPreference = 'Continue'
    try {
        & node @nodeArgs
        $qaExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = 'Stop'
    }
    if ($null -eq $qaExitCode) { $qaExitCode = 1 }
}
finally {
    Stop-Product

    if ($stashed) {
        Write-Host ''
        Write-Host 'restoring the real profile'
        if (Test-Path -LiteralPath $profile) { Remove-Item -LiteralPath $profile -Recurse -Force }
        Copy-Item -LiteralPath $stash -Destination $profile -Recurse -Force
        $restored = Get-TreeManifest -Path $profile
        $problems = @()
        foreach ($entry in $originalManifest) {
            $match = $restored | Where-Object { $_.Path -eq $entry.Path }
            if (-not $match) { $problems += "missing: $($entry.Path)"; continue }
            if ($match.Length -ne $entry.Length) { $problems += "size differs: $($entry.Path)"; continue }
            if ($match.SHA256 -ne $entry.SHA256) { $problems += "hash differs: $($entry.Path)"; continue }
        }
        foreach ($entry in $restored) {
            if (-not ($originalManifest | Where-Object { $_.Path -eq $entry.Path })) {
                $problems += "unexpected extra file: $($entry.Path)"
            }
        }
        Check 'real profile restored and verified (size and SHA-256)' ($problems.Count -eq 0) ($problems -join '; ')
        if ($problems.Count -gt 0) { Write-Host "  stash kept for recovery: $stash" -ForegroundColor Yellow }
        elseif (-not $KeepEvidence -and -not $KeepFixtures) {
            Remove-Item -LiteralPath $StashRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "QA exit code: $qaExitCode"
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) wrapper checks passed"
if ($qaExitCode -ne 0 -or $failed.Count -gt 0) { exit 1 }
exit 0
