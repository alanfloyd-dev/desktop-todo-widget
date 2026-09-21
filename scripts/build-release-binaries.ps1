#Requires -Version 5.1
<#
.SYNOPSIS
    Builds and verifies the two production release binaries.

.DESCRIPTION
    A narrow build substrate, not a release pipeline: preflight consistency
    checks, the two canonical build commands, then artifact verification and
    metadata output. No ZIP, no package assembly, no manifest, no signing, no
    publishing.

    The canonical build entries are frozen:

      main   : pnpm tauri build --no-bundle
               (the Tauri CLI is the only production main entry because it is
               the one path that guarantees vue-tsc, the Vite build, a fresh
               frontendDist, custom-protocol, and the Rust release build
               together; a bare `cargo build --release` would produce a
               release-profile binary still pointed at the dev server)
      helper : cargo build --release -p desktop-todo-maintenance
               --manifest-path src-tauri/Cargo.toml
               (the parent manifest applies the parent release profile)

    The script exposes no Cargo feature parameters by design: feature
    selection for a release is explicit and minimal, and never caller-chosen.
    `--all-features` is forbidden — for the main binary it would pull the
    maintenance-qa sandbox into the production shape, for the helper the qa
    feature. The only switch is -Preflight, which runs the preflight checks
    and exits without building.

.PARAMETER Preflight
    Run only the preflight checks (version consistency, config invariants,
    feature hygiene) and exit. No build, no artifacts.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release-binaries.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release-binaries.ps1 -Preflight
#>
[CmdletBinding()]
param(
    [switch]$Preflight
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$packageJsonPath = Join-Path $repoRoot 'package.json'
$tauriConfPath = Join-Path $repoRoot 'src-tauri\tauri.conf.json'
$mainManifestPath = Join-Path $repoRoot 'src-tauri\Cargo.toml'

# The only production artifacts this script will ever accept. No globbing, no
# "newest exe" search, no fallback directories — in particular nothing under
# .rc0-taskbar-target, maintenance\target, or release\ can ever be selected.
$MainArtifact = Join-Path $repoRoot 'src-tauri\target\release\alan-desktop.exe'
$HelperArtifact = Join-Path $repoRoot 'src-tauri\target\release\desktop-todo-maintenance.exe'

function Get-ProductVersionSources {
    $packageJson = Get-Content -LiteralPath $packageJsonPath -Raw | ConvertFrom-Json
    $tauriConf = Get-Content -LiteralPath $tauriConfPath -Raw | ConvertFrom-Json
    # Structured, not a regex over the TOML: cargo metadata is the manifest's
    # own parser.
    $metadataJson = & cargo metadata --format-version 1 --manifest-path $mainManifestPath 2>$null
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $metadata = $metadataJson | ConvertFrom-Json
    $product = @($metadata.packages | Where-Object { $_.name -eq 'alan-desktop' })
    if ($product.Count -ne 1) { throw 'alan-desktop package not found in cargo metadata' }
    [pscustomobject]@{
        PackageJson = [string]$packageJson.version
        TauriConf   = [string]$tauriConf.version
        CargoToml   = [string]$product[0].version
        TauriConfObject = $tauriConf
        ProductPackage  = $product[0]
        MaintenancePackage = @($metadata.packages | Where-Object { $_.name -eq 'desktop-todo-maintenance' })[0]
    }
}

function Assert-PreFlight {
    $sources = Get-ProductVersionSources

    # Version consistency: the three product version sources must agree. The
    # maintenance crate's own package version (0.1.0) is deliberately not part
    # of this equality — it is the helper's crate version, not the product's.
    foreach ($pair in @(
        @{ Name = 'package.json'; Value = $sources.PackageJson },
        @{ Name = 'src-tauri/Cargo.toml'; Value = $sources.CargoToml },
        @{ Name = 'src-tauri/tauri.conf.json'; Value = $sources.TauriConf }
    )) {
        if ($pair.Value -ne $sources.PackageJson) {
            throw ("Product version mismatch:`n" +
                "  package.json            : $($sources.PackageJson)`n" +
                "  src-tauri/Cargo.toml    : $($sources.CargoToml)`n" +
                "  src-tauri/tauri.conf.json: $($sources.TauriConf)")
        }
    }
    Write-Host "  version consistency : $($sources.PackageJson) (package.json = Cargo.toml = tauri.conf.json)"

    # Config invariants.
    $bundleActive = $sources.TauriConfObject.bundle.active
    if ($bundleActive) { throw 'tauri.conf.json bundle.active must be false (no bundler payload beside the exe).' }
    $frontendDist = $sources.TauriConfObject.build.frontendDist
    if ($frontendDist -ne '../dist') { throw "tauri.conf.json build.frontendDist must be '../dist', found '$frontendDist'." }
    Write-Host '  config invariants   : bundle.active=false, frontendDist=../dist'

    # Feature hygiene: explicit and minimal, never default-on, never caller-chosen.
    $features = $sources.ProductPackage.features
    if (-not $features.PSObject.Properties['custom-protocol']) {
        throw 'main package must define the custom-protocol feature.'
    }
    if ($features.PSObject.Properties['default'] -and
        @($features.'default') -contains 'custom-protocol') {
        throw 'custom-protocol must not be a default feature (it would flip tauri dev onto embedded assets).'
    }
    if ($features.PSObject.Properties['default'] -and
        @($features.'default') -contains 'maintenance-qa') {
        throw 'maintenance-qa must never be a default feature.'
    }
    $maintenanceFeatures = $sources.MaintenancePackage.features
    if ($maintenanceFeatures.PSObject.Properties['default'] -and
        @($maintenanceFeatures.'default') -contains 'qa') {
        throw 'the maintenance qa feature must never be default.'
    }
    Write-Host '  feature hygiene     : custom-protocol defined & not default; qa/maintenance-qa not default'

    Write-Host 'preflight OK'
}

function Assert-ProductionArtifact {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedVersion,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "missing $Label artifact: $Path"
    }
    $item = Get-Item -LiteralPath $Path
    $fileVersion = [string]$item.VersionInfo.FileVersion
    $productVersion = [string]$item.VersionInfo.ProductVersion
    # PE numeric versions carry a trailing revision zero (1.2.0.0); normalize
    # by trimming trailing ".0" components before comparing.
    $normalizedExpected = ($ExpectedVersion -replace '(\.0)+$', '')
    $normalizedFile = ($fileVersion -replace '(\.0)+$', '')
    $normalizedProduct = ($productVersion -replace '(\.0)+$', '')
    if ($normalizedFile -ne $normalizedExpected -or $normalizedProduct -ne $normalizedExpected) {
        throw ("$Label PE version mismatch: expected $ExpectedVersion, found FileVersion='$fileVersion' ProductVersion='$productVersion'.")
    }
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    [pscustomobject]@{
        Artifact       = $Label
        Path           = $Path
        SizeBytes      = $item.Length
        FileVersion    = $fileVersion
        ProductVersion = $productVersion
        SHA256         = $hash
    }
}

function Test-NoQaMarkers {
    # Defense-in-depth only: a production artifact must not contain markers
    # that only the compile-time-gated qa/maintenance-qa features can emit.
    # The primary provenance argument is the canonical command plus the
    # explicit feature set plus the exact artifact path — a string scan can
    # never prove feature provenance by itself.
    param([Parameter(Mandatory = $true)][string[]]$Paths)
    foreach ($path in $Paths) {
        $bytes = [System.IO.File]::ReadAllBytes($path)
        $text = [System.Text.Encoding]::ASCII.GetString($bytes)
        if ($text.Contains('DTW_MAINTENANCE_QA_ID')) {
            throw "QA marker 'DTW_MAINTENANCE_QA_ID' found in production artifact: $path"
        }
    }
}

Assert-PreFlight
if ($Preflight) {
    Write-Host 'preflight-only run; no build performed.'
    exit 0
}

# --- Canonical builds (no fallback paths) ------------------------------------
Write-Host 'building main (pnpm tauri build --no-bundle)...'
# The Tauri CLI locates src-tauri relative to the process working directory,
# so the frontend/Rust build runs from the repository root no matter where
# this script was invoked from. The helper cargo build needs no such anchor
# (it takes an explicit --manifest-path), but every path in this script is
# derived from $PSScriptRoot either way.
Push-Location $repoRoot
try {
    & pnpm tauri build --no-bundle
    if ($LASTEXITCODE -ne 0) { throw "main build failed (pnpm tauri build --no-bundle exited $LASTEXITCODE)." }
}
finally {
    Pop-Location
}

Write-Host 'building helper (cargo build --release -p desktop-todo-maintenance)...'
& cargo build --release -p desktop-todo-maintenance --manifest-path $mainManifestPath
if ($LASTEXITCODE -ne 0) { throw "helper build failed (canonical cargo build exited $LASTEXITCODE)." }

# --- Artifact verification ----------------------------------------------------
$sources = Get-ProductVersionSources
$expectedVersion = $sources.PackageJson
$main = Assert-ProductionArtifact -Path $MainArtifact -ExpectedVersion $expectedVersion -Label 'main'
$helper = Assert-ProductionArtifact -Path $HelperArtifact -ExpectedVersion $expectedVersion -Label 'helper'

Write-Host 'scanning for QA markers (defense-in-depth)...'
Test-NoQaMarkers -Paths @($MainArtifact, $HelperArtifact)

Write-Host ''
Write-Host 'release binaries verified:'
foreach ($artifact in @($main, $helper)) {
    Write-Host ("  {0,-7} {1}" -f $artifact.Artifact, $artifact.Path)
    Write-Host ("          size={0}  ProductVersion={1}" -f $artifact.SizeBytes, $artifact.ProductVersion)
    Write-Host ("          sha256={0}" -f $artifact.SHA256)
}
