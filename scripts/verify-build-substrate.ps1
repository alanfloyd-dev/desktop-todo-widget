#Requires -Version 5.1
<#
.SYNOPSIS
    Structural verification for scripts/build-release-binaries.ps1.

.DESCRIPTION
    Keeps the build substrate narrow and safe to run:

      * the substrate accepts no Cargo feature parameters (a caller can never
        inject --all-features or an arbitrary feature set),
      * the canonical build commands are the exact frozen entries,
      * artifact selection is fixed-path only - no globbing, no directory
        search, so historical QA or release directories can never be picked up,
      * and the preflight half actually runs against this repository
        (positive test of the version/config/feature checks).

    Checks run against the substrate with comments stripped: its own
    documentation mentions the forbidden constructs in order to forbid them,
    so matching the raw text would self-report. Negative functional fixtures
    (a deliberately mismatched version, a missing artifact, a wrong PE
    version) would require mutating repository files or a fixture harness
    around the substrate; they are deliberately not built here.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\verify-build-substrate.ps1
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$substratePath = Join-Path $repoRoot 'scripts\build-release-binaries.ps1'

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition; Detail = $Detail }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

$text = Get-Content -LiteralPath $substratePath -Raw
$code = [regex]::Replace($text, '(?s)<#.*?#>', '')
$code = [regex]::Replace($code, '(?m)^\s*#.*$', '')

Write-Host 'build substrate structural checks'

$paramBlock = [regex]::Match($code, 'param\((?s)(.*?)\)').Groups[1].Value
Check 'substrate accepts only -Preflight' `
    ($paramBlock.Contains('$Preflight') -and $paramBlock -notmatch 'Feature')
Check 'substrate code never references all-features' `
    (-not $code.Contains('all-features'))

$invocations = @($code -split "`r?`n" | Where-Object { $_ -match '^\s*&\s' })
Check 'substrate invokes pnpm tauri build --no-bundle exactly once' `
    (@($invocations | Where-Object { $_ -match '&\s*pnpm tauri build --no-bundle' }).Count -eq 1)
Check 'substrate invokes the canonical helper build exactly once' `
    (@($invocations | Where-Object { $_ -match '&\s*cargo build --release -p desktop-todo-maintenance --manifest-path' }).Count -eq 1)
Check 'no other cargo/pnpm invocation exists (no fallback build path)' `
    ($invocations.Count -eq 2) ("invocations: $($invocations.Count)")
Check 'build failures propagate (no fallback continues after failure)' `
    ($code.Contains('if ($LASTEXITCODE -ne 0) { throw'))

# External-command failure semantics: every invoked external command must be
# followed by an immediate exit-code check that throws, so a failed build can
# never fall through to artifact verification of stale on-disk files.
$exitChecks = @([regex]::Matches($code, 'if \(\$LASTEXITCODE -ne 0\) \{ throw')).Count
Check 'every external command has an immediate exit-code throw' ($exitChecks -eq 3) "checks=$exitChecks"

# Stable repo-root derivation and cwd independence: all paths come from the
# script's own location, and the Tauri CLI (which resolves src-tauri from the
# process working directory) runs anchored at the repo root.
Check 'substrate derives repo root from its own location' `
    ($code.Contains('$repoRoot = Split-Path -Parent $PSScriptRoot'))
Check 'substrate anchors the frontend build at the repo root (cwd independence)' `
    ($code.Contains('Push-Location $repoRoot') -and $code.Contains('Pop-Location'))

Check 'main artifact is the exact release path' `
    ($text.Contains('src-tauri\target\release\alan-desktop.exe'))
Check 'helper artifact is the exact release path' `
    ($text.Contains('src-tauri\target\release\desktop-todo-maintenance.exe'))
foreach ($forbidden in @('Get-ChildItem', 'rc0-taskbar-target', 'maintenance\target')) {
    Check "substrate code never searches for artifacts ('$forbidden')" `
        (-not $code.Contains($forbidden))
}

Check 'substrate scans production artifacts for QA markers' `
    ($text.Contains('DTW_MAINTENANCE_QA_ID'))
Check 'substrate labels the marker scan as defense-in-depth' `
    ($text.Contains('Defense-in-depth only'))

Check 'substrate reads versions via cargo metadata' `
    ($code.Contains('cargo metadata --format-version 1'))
Check 'substrate reads package.json / tauri.conf.json via ConvertFrom-Json' `
    ($code.Contains('ConvertFrom-Json'))

Write-Host 'substrate preflight runs clean'
$output = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass `
    -File $substratePath -Preflight 2>&1
$exitCode = $LASTEXITCODE
Check 'preflight exits 0 on the current repository' ($exitCode -eq 0) (($output | Out-String))
$flattened = $output | Out-String
Check 'preflight asserts version consistency' ($flattened -match 'version consistency')
Check 'preflight asserts config invariants' ($flattened -match 'config invariants')
Check 'preflight asserts feature hygiene' ($flattened -match 'feature hygiene')

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) checks passed"
if ($failed.Count -gt 0) {
    foreach ($check in $failed) { Write-Host "  failed: $($check.Name)" -ForegroundColor Red }
    exit 1
}
exit 0
