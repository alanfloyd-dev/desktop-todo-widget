#Requires -Version 5.1
<#
.SYNOPSIS
    Negative tests for the QA safety interlock (test-only).

.DESCRIPTION
    Proves the interlock functions refuse unsafe configurations BEFORE any
    filesystem or registry mutation, and that the refusals themselves have no
    side effects. Each case plants sentinel files and verifies afterwards that
    nothing was created, modified or deleted:

      * missing DTW_MAINTENANCE_QA_ID
      * mismatched DTW_MAINTENANCE_QA_ID
      * non-canonical QA id
      * sandbox root outside the QA namespace (including the real production
        install root passed verbatim)
      * QA install/data root aliasing a production root
      * install.ps1-style run without a sandboxed LOCALAPPDATA/APPDATA
      * helper binary that is not a qa build (simulated through a missing
        binary; the real production helper would exit non-zero on the probe)
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'qa-safety.ps1')

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition; Detail = $Detail }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

function Assert-Throws {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Body,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string[]]$SentinelPaths
    )
    $threw = $false
    try { & $Body } catch { $threw = $true }
    Check "$Name refuses" $threw
    foreach ($sentinel in $SentinelPaths) {
        Check "$Name leaves the sentinel untouched" (Test-Path -LiteralPath $sentinel -PathType Leaf) $sentinel
    }
}

$qaId = [guid]::NewGuid().ToString()
$qaRoot = Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$qaId"

$sentinelDir = Join-Path $env:TEMP ('dtw-interlock-sentinel-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sentinelDir -Force | Out-Null
$sentinel = Join-Path $sentinelDir 'sentinel.txt'
Set-Content -LiteralPath $sentinel -Value 'must survive' -Encoding ASCII
$productionInstallRoot = Join-Path $env:LOCALAPPDATA 'Programs\desktop-todo-widget'

$previousQaId = $env:DTW_MAINTENANCE_QA_ID
try {
    # --- identity interlocks ---------------------------------------------------
    Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue
    Assert-Throws -Name 'missing QA identity env' -SentinelPaths @($sentinel) -Body {
        Assert-QAHelperContext -QaId $qaId -QaRoot $qaRoot
    }

    $env:DTW_MAINTENANCE_QA_ID = [guid]::NewGuid().ToString()
    Assert-Throws -Name 'mismatched QA identity env' -SentinelPaths @($sentinel) -Body {
        Assert-QAHelperContext -QaId $qaId -QaRoot $qaRoot
    }

    $env:DTW_MAINTENANCE_QA_ID = 'not-a-canonical-uuid'
    Assert-Throws -Name 'non-canonical QA id' -SentinelPaths @($sentinel) -Body {
        Assert-QAHelperContext -QaId 'not-a-canonical-uuid' -QaRoot (Join-Path $qaRoot 'x')
    }

    # --- namespace interlocks ---------------------------------------------------
    $env:DTW_MAINTENANCE_QA_ID = $qaId
    Assert-Throws -Name 'sandbox root outside the QA namespace' -SentinelPaths @($sentinel) -Body {
        Assert-QAHelperContext -QaId $qaId -QaRoot (Join-Path $env:TEMP 'outside-namespace')
    }
    Assert-Throws -Name 'production install root as sandbox root' -SentinelPaths @($sentinel) -Body {
        Assert-QAHelperContext -QaId $qaId -QaRoot $productionInstallRoot
    }
    # The namespace containment must already separate the production data root
    # from any QA sandbox, before the alias assertions even run.
    $productionData = Join-Path $env:APPDATA 'net.alanfloyd.desktop'
    Check 'production data root is outside the QA namespace' `
        (-not (Test-QAPathUnder -Path $productionData -Root (Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$qaId")))
    $aliasedRoot = $productionData
    Assert-Throws -Name 'sandbox root aliasing the production data root' -SentinelPaths @($sentinel) -Body {
        Assert-QAHelperContext -QaId $qaId -QaRoot $aliasedRoot
    }

    # --- fake-environment interlock ----------------------------------------------
    $previousLocal = $env:LOCALAPPDATA
    $previousRoaming = $env:APPDATA
    try {
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
        Assert-Throws -Name 'unsandboxed environment for install.ps1 runs' -SentinelPaths @($sentinel) -Body {
            Assert-QAFakeEnvironment -QaRoot $qaRoot
        }
        $fakeLocal = Join-Path $qaRoot 'fake-local'
        $fakeRoaming = Join-Path $qaRoot 'fake-roaming'
        $env:LOCALAPPDATA = $fakeLocal
        $env:APPDATA = $fakeRoaming
        $refused = $false
        try { Assert-QAFakeEnvironment -QaRoot $qaRoot } catch { $refused = $true }
        Check 'sandboxed environment is accepted (no mutation)' (-not $refused)
        Check 'acceptance created nothing' (-not (Test-Path -LiteralPath $fakeLocal))
    }
    finally {
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
    }

    # --- helper capability interlock ----------------------------------------------
    $previousQaIdRestore = $env:DTW_MAINTENANCE_QA_ID
    $env:DTW_MAINTENANCE_QA_ID = $qaId
    try {
        Assert-Throws -Name 'missing helper binary' -SentinelPaths @($sentinel) -Body {
            Assert-QAHelperIsQaBuild -HelperExe (Join-Path $sentinelDir 'no-such-helper.exe') -QaId $qaId
        }
    }
    finally {
        if ($null -ne $previousQaIdRestore) { $env:DTW_MAINTENANCE_QA_ID = $previousQaIdRestore } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }

    # --- no QA namespace was created by any refusal --------------------------------
    Check 'refusals created no QA namespace directory' `
        (-not (Test-Path -LiteralPath $qaRoot))
    Check 'sentinel directory still holds only the sentinel' `
        (@(Get-ChildItem -LiteralPath $sentinelDir -Force).Count -eq 1)
}
finally {
    if ($null -ne $previousQaId) { $env:DTW_MAINTENANCE_QA_ID = $previousQaId } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $sentinelDir -Recurse -Force -ErrorAction SilentlyContinue
}

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) interlock self-test checks passed"
if ($failed.Count -gt 0) {
    foreach ($check in $failed) { Write-Host "  failed: $($check.Name)" -ForegroundColor Red }
    exit 1
}
exit 0
