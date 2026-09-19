#Requires -Version 5.1
<#
.SYNOPSIS
    Final isolated full-lifecycle acceptance QA (test-only).

.DESCRIPTION
    Exercises the entire Phase 1 product lifecycle through the real product
    entry points, inside the dedicated QA namespace:

      v1.1-style unmanaged runtime + real user-data fixture
        -> real install.ps1 bootstrap (adoption)
        -> receipt + Windows integration established
        -> real v1.2 app launch: admission Unmanaged before, Managed after
        -> Settings uninstall handoff (real frontend -> start_uninstall)
        -> helper keep-data uninstall (real uninstall_locked, unattended)
        -> runtime/integrations removed, persistent data survives
        -> reinstall via real install.ps1 (new installation id)
        -> same data re-opens with all logical records
        -> second handoff -> delete-data uninstall
        -> known app-owned data removed, unknown sentinels preserved

    Everything runs under the qa-safety.ps1 hard interlock; the real
    production installation is snapshotted before and verified after.
#>
[CmdletBinding()]
param(
    [switch]$KeepSandbox,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'qa-safety.ps1')

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$targetDir = Join-Path $repoRoot 'src-tauri\target'
$helperExe = Join-Path $targetDir 'debug\desktop-todo-maintenance.exe'
# the raw cargo build output keeps the crate name; the payload copy and the
# installed runtime both carry the public product name
$productExe = Join-Path $targetDir 'debug\alan-desktop.exe'
$installScript = Join-Path $repoRoot 'install.ps1'
$releaseZip = Join-Path $repoRoot 'release\v1.1.0\desktop-todo-widget-v1.1.0-windows-x64.zip'
$productVersion = '1.1.0'

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition; Detail = $Detail }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

function Invoke-InstallPs1 {
    # Runs the real install.ps1 under a fully sandboxed environment; the
    # delegated helper keeps resolving its QA namespace from the QA id.
    param(
        [Parameter(Mandatory = $true)][string]$PayloadDir,
        [Parameter(Mandatory = $true)][string]$QaId,
        [Parameter(Mandatory = $true)][string]$FakeLocal,
        [Parameter(Mandatory = $true)][string]$FakeRoaming
    )
    $previousQa = $env:DTW_MAINTENANCE_QA_ID
    $previousLocal = $env:LOCALAPPDATA
    $previousRoaming = $env:APPDATA
    $ErrorActionPreference = 'Continue'
    try {
        $env:DTW_MAINTENANCE_QA_ID = $QaId
        $env:LOCALAPPDATA = $FakeLocal
        $env:APPDATA = $FakeRoaming
        $output = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installScript `
            -Source $PayloadDir 2>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = 'Stop'
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
        if ($null -ne $previousQa) { $env:DTW_MAINTENANCE_QA_ID = $previousQa } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }
    return [pscustomobject]@{ ExitCode = $code; Output = $output }
}

function Wait-LogMarker {
    param(
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][string]$Marker,
        [int]$Seconds = 60
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Test-Path -LiteralPath $LogPath -PathType Leaf) -and
            ((Get-Content -LiteralPath $LogPath -Raw -ErrorAction SilentlyContinue) -match [regex]::Escape($Marker))) {
            return $true
        }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Wait-PathExists {
    param([Parameter(Mandatory = $true)][string]$Path, [int]$Seconds = 60)
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $Path -PathType Leaf) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Wait-PathGone {
    param([Parameter(Mandatory = $true)][string]$Path, [int]$Seconds = 60)
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Test-Path -LiteralPath $Path)) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Invoke-QAApp {
    # Launches the real QA product binary from the installed runtime and waits
    # for its own exit (the frontend QA hook drives start_uninstall).
    param(
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [Parameter(Mandatory = $true)][string]$QaId,
        [Parameter(Mandatory = $true)][string]$DataMode,
        [Parameter(Mandatory = $true)][string]$Marker
    )
    $previousQa = $env:DTW_MAINTENANCE_QA_ID
    $previousHandoff = $env:DTW_MAINTENANCE_QA_HANDOFF
    $previousData = $env:DTW_MAINTENANCE_QA_UNINSTALL_DATA
    $logPath = Join-Path $InstallDir 'qa-diagnostics.log'
    $process = $null
    try {
        $env:DTW_MAINTENANCE_QA_ID = $QaId
        $env:DTW_MAINTENANCE_QA_HANDOFF = '1'
        $env:DTW_MAINTENANCE_QA_UNINSTALL_DATA = $DataMode
        Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
        $process = Start-Process -FilePath (Join-Path $InstallDir 'desktop-todo-widget.exe') `
            -WorkingDirectory $InstallDir -PassThru
        $admitted = Wait-LogMarker -LogPath $logPath -Marker $Marker -Seconds 60
        if (-not $admitted -and -not $process.HasExited) {
            # give the frontend hook its full window before judging
            $exited = $process.WaitForExit(30000)
        }
        else {
            $exited = $process.WaitForExit(60000)
        }
        return [pscustomobject]@{ Admitted = $admitted; Exited = [bool]$exited; LogPath = $logPath }
    }
    finally {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $previousQa) { $env:DTW_MAINTENANCE_QA_ID = $previousQa } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
        if ($null -ne $previousHandoff) { $env:DTW_MAINTENANCE_QA_HANDOFF = $previousHandoff } else { Remove-Item Env:\DTW_MAINTENANCE_QA_HANDOFF -ErrorAction SilentlyContinue }
        if ($null -ne $previousData) { $env:DTW_MAINTENANCE_QA_UNINSTALL_DATA = $previousData } else { Remove-Item Env:\DTW_MAINTENANCE_QA_UNINSTALL_DATA -ErrorAction SilentlyContinue }
    }
}

function Invoke-Fixture {
    param([Parameter(Mandatory = $true)][string]$DataRoot, [Parameter(Mandatory = $true)][string]$TestName)
    $previous = $env:DTW_QA_FIXTURE_DATA_ROOT
    $env:DTW_QA_FIXTURE_DATA_ROOT = $DataRoot
    $previousEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $output = ''
    try {
        $output = & cargo test --manifest-path (Join-Path $repoRoot 'src-tauri\Cargo.toml') --offline $TestName -- --ignored --nocapture 2>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousEap
        if ($null -ne $previous) { $env:DTW_QA_FIXTURE_DATA_ROOT = $previous } else { Remove-Item Env:\DTW_QA_FIXTURE_DATA_ROOT -ErrorAction SilentlyContinue }
    }
    return [pscustomobject]@{ ExitCode = $code; Output = $output }
}

# --- build -------------------------------------------------------------------
if (-not $SkipBuild) {
    Write-Host 'building frontend, QA helper, and QA product binary (custom-protocol)...'
    Push-Location $repoRoot
    $previousEap = $ErrorActionPreference
    try {
        # via cmd: pnpm writes progress to stderr, which PowerShell 5.1 would
        # otherwise wrap into a stopping error under EAP=Stop
        $ErrorActionPreference = 'Continue'
        & cmd /c "pnpm build" 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'pnpm build failed' }
    }
    finally {
        $ErrorActionPreference = $previousEap
        Pop-Location
    }
    & cargo build --manifest-path (Join-Path $repoRoot 'src-tauri\maintenance\Cargo.toml') --offline --features qa --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'helper build failed' }
    & cargo build --manifest-path (Join-Path $repoRoot 'src-tauri\Cargo.toml') --offline --features maintenance-qa,custom-protocol --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'product build failed' }
}
foreach ($binary in @($helperExe, $productExe)) {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "missing build output: $binary" }
}
$info = (Get-Item -LiteralPath $productExe).VersionInfo
if ($info.ProductVersion -ne $productVersion) { throw "product fixture has unexpected ProductVersion '$($info.ProductVersion)'" }

# --- interlock ----------------------------------------------------------------
$productionSnapshot = Get-ProductionSafetySnapshot
Write-Host ("production exe sha256: {0}" -f $productionSnapshot.ExeSha256)
$probeId = [guid]::NewGuid().ToString()
Assert-QAHelperIsQaBuild -HelperExe $helperExe -QaId $probeId
Remove-Item -LiteralPath (Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$probeId") -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$probeId" -Recurse -Force -ErrorAction SilentlyContinue

$legacyExtract = Join-Path $env:TEMP ('dtw-lifecycle-' + [guid]::NewGuid().ToString('N'))
Expand-Archive -LiteralPath $releaseZip -DestinationPath $legacyExtract -Force
$legacyExe = Join-Path $legacyExtract 'desktop-todo-widget.exe'
if (-not (Test-Path -LiteralPath $legacyExe -PathType Leaf)) { throw 'v1.1 zip missing executable' }

$context = $null
try {
    Write-Host ''
    Write-Host 'lifecycle: unmanaged v1.1 runtime + real user data'
    $qaId = [guid]::NewGuid().ToString()
    $root = Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$qaId"
    $previousQaId = $env:DTW_MAINTENANCE_QA_ID
    $env:DTW_MAINTENANCE_QA_ID = $qaId
    Assert-QAHelperContext -QaId $qaId -QaRoot $root
    $context = [pscustomobject]@{
        QaId = $qaId; Root = $root
        Install = Join-Path $root 'install'
        Data = Join-Path $root 'data'
        Menu = Join-Path $root 'menu'
        State = Join-Path $root 'maintenance'
    }
    New-Item -ItemType Directory -Path $context.Install, $context.Data, (Join-Path $context.Data 'assets') -Force | Out-Null

    # v1.1-style unmanaged runtime + unknown sentinels + real user data
    Copy-Item -LiteralPath $legacyExe -Destination (Join-Path $context.Install 'desktop-todo-widget.exe') -Force
    Set-Content -LiteralPath (Join-Path $context.Install 'QA_UNKNOWN_DO_NOT_DELETE.dll') -Value 'install sentinel' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $context.Data 'QA_UNKNOWN_DO_NOT_DELETE.txt') -Value 'data sentinel' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $context.Data "assets\avatar-$qaId.png") -Value 'managed appearance image' -Encoding ASCII
    $fixtureCode = Invoke-Fixture -DataRoot $context.Data -TestName 'write_representative_user_data_fixture'
    Check 'real-schema user data fixture created' ($fixtureCode.ExitCode -eq 0) $fixtureCode.Output
    $dbHashBefore = (Get-FileHash -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3') -Algorithm SHA256).Hash

    # P0: before bootstrap the QA app admits as Unmanaged
    $scratch = Join-Path $root 'scratch'
    New-Item -ItemType Directory -Path $scratch -Force | Out-Null
    Copy-Item -LiteralPath $productExe -Destination (Join-Path $scratch 'desktop-todo-widget.exe') -Force
    $env:DTW_MAINTENANCE_QA_ID = $qaId
    $scratchLog = Join-Path $scratch 'qa-diagnostics.log'
    $p0 = Start-Process -FilePath (Join-Path $scratch 'desktop-todo-widget.exe') -WorkingDirectory $scratch -PassThru
    $unmanagedSeen = Wait-LogMarker -LogPath $scratchLog -Marker 'maintenance_admission=Unmanaged' -Seconds 60
    if (-not $p0.HasExited) { Stop-Process -Id $p0.Id -Force -ErrorAction SilentlyContinue }
    $p0.WaitForExit(10000) | Out-Null
    if ($null -ne $previousQaId) { $env:DTW_MAINTENANCE_QA_ID = $previousQaId }
    Check 'pre-bootstrap admission is Unmanaged' $unmanagedSeen

    # P1: real install.ps1 bootstrap (v1.1 adoption through the managed path)
    Write-Host 'lifecycle: managed bootstrap via real install.ps1'
    $payload = Join-Path $root 'payload'
    New-Item -ItemType Directory -Path $payload -Force | Out-Null
    Copy-Item -LiteralPath $helperExe -Destination (Join-Path $payload 'desktop-todo-maintenance.exe') -Force
    Copy-Item -LiteralPath $productExe -Destination (Join-Path $payload 'desktop-todo-widget.exe') -Force
    Set-Content -LiteralPath (Join-Path $payload 'README.md') -Value 'payload readme' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $payload 'LICENSE_ZH.md') -Value 'payload license zh' -Encoding ASCII
    $fakeLocal = Join-Path $root 'fake-local'
    $fakeRoaming = Join-Path $root 'fake-roaming'
    New-Item -ItemType Directory -Path (Join-Path $fakeLocal 'Programs'), $fakeRoaming -Force | Out-Null
    $bootstrap = Invoke-InstallPs1 -PayloadDir $payload -QaId $qaId -FakeLocal $fakeLocal -FakeRoaming $fakeRoaming
    Check 'install.ps1 bootstrap exits 0' ($bootstrap.ExitCode -eq 0) $bootstrap.Output
    $receiptPath = Join-Path $context.Install 'installation-receipt.json'
    Check 'managed receipt established' (Test-Path -LiteralPath $receiptPath -PathType Leaf)
    $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
    Check 'receipt lifecycle Installed, version invariant holds' (
        $receipt.lifecycleState -eq 'Installed' -and
        $receipt.currentVersion -eq $productVersion -and
        $receipt.installationId -eq $receipt.installationId)
    $firstInstallationId = $receipt.installationId
    $reg = Get-ItemProperty -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$qaId" -ErrorAction SilentlyContinue
    Check 'Windows integration established with version invariant' (
        $null -ne $reg -and
        $reg.DisplayVersion -eq $productVersion -and
        $reg.UninstallString -eq ('"{0}" --uninstall' -f (Join-Path $context.Install 'desktop-todo-maintenance.exe')))
    $shell = New-Object -ComObject WScript.Shell
    try { $shortcutTarget = $shell.CreateShortcut((Join-Path $context.Menu 'desktop-todo-widget.lnk')).TargetPath }
    finally { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
    Check 'Start Menu shortcut targets the installed executable' `
        ($shortcutTarget -eq (Join-Path $context.Install 'desktop-todo-widget.exe'))

    # P2: real app launch -> Managed -> Settings uninstall handoff -> keep data
    Write-Host 'lifecycle: managed launch + keep-data uninstall through Settings handoff'
    $launch1 = Invoke-QAApp -InstallDir $context.Install -QaId $qaId -DataMode 'keep' `
        -Marker "maintenance_admission=Managed installationId=$firstInstallationId"
    Check 'app admits as Managed (post-bootstrap)' $launch1.Admitted
    Check 'app exits after launching the uninstaller' $launch1.Exited
    $null = Wait-PathGone -Path $receiptPath -Seconds 90
    $null = Wait-PathExists -Path (Join-Path $context.State 'qa-uninstall-complete.txt') -Seconds 30
    Check 'keep-data uninstall completed' `
        ((Test-Path -LiteralPath (Join-Path $context.State 'qa-uninstall-complete.txt') -PathType Leaf) -and
         (-not (Test-Path -LiteralPath $receiptPath)))

    # P3: keep-data state assertions
    Check 'main executable removed' (-not (Test-Path -LiteralPath (Join-Path $context.Install 'desktop-todo-widget.exe')))
    Check 'installed maintenance helper removed' (-not (Test-Path -LiteralPath (Join-Path $context.Install 'desktop-todo-maintenance.exe')))
    Check 'uninstall registration removed' (-not (Test-Path -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$qaId"))
    Check 'Start Menu shortcut removed' (-not (Test-Path -LiteralPath (Join-Path $context.Menu 'desktop-todo-widget.lnk')))
    Check 'final receipt removed' (-not (Test-Path -LiteralPath $receiptPath))
    Check 'active uninstall journal removed' `
        (-not (Test-Path -LiteralPath (Join-Path $context.State 'active-uninstall.json')))
    Check 'persistent database survives keep-data uninstall' `
        (Test-Path -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3'))
    Check 'unknown install-root sentinel preserved' `
        (Test-Path -LiteralPath (Join-Path $context.Install 'QA_UNKNOWN_DO_NOT_DELETE.dll'))
    Check 'unknown data-root sentinel preserved' `
        (Test-Path -LiteralPath (Join-Path $context.Data 'QA_UNKNOWN_DO_NOT_DELETE.txt'))
    Check 'unknown install-root file keeps the install directory in place' `
        (Test-Path -LiteralPath $context.Install -PathType Container)

    # P4: reinstall via real install.ps1 -> new lifecycle, same data
    Write-Host 'lifecycle: reinstall after keep-data uninstall'
    $reinstall = Invoke-InstallPs1 -PayloadDir $payload -QaId $qaId -FakeLocal $fakeLocal -FakeRoaming $fakeRoaming
    Check 'reinstall exits 0' ($reinstall.ExitCode -eq 0) $reinstall.Output
    $receipt2 = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
    Check 'reinstall establishes Installed receipt with a NEW installation id' (
        $receipt2.lifecycleState -eq 'Installed' -and
        $receipt2.installationId -ne $firstInstallationId)
    $reg2 = Get-ItemProperty -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$qaId" -ErrorAction SilentlyContinue
    Check 'reinstall re-establishes integration with matching version' (
        $null -ne $reg2 -and
        $reg2.DisplayVersion -eq $productVersion -and
        $reg2.InstallationId -eq $receipt2.installationId)
    $verifyCode = Invoke-Fixture -DataRoot $context.Data -TestName 'verify_representative_user_data_fixture'
    Check 'same database re-opens with all logical records after reinstall' ($verifyCode.ExitCode -eq 0) $verifyCode.Output
    Check 'database path never moved' (Test-Path -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3'))

    # P5: second handoff -> delete-data uninstall
    Write-Host 'lifecycle: managed launch + delete-data uninstall through Settings handoff'
    $launch2 = Invoke-QAApp -InstallDir $context.Install -QaId $qaId -DataMode 'remove' `
        -Marker "maintenance_admission=Managed installationId=$($receipt2.installationId)"
    Check 'reinstalled app admits as Managed' $launch2.Admitted
    Check 'reinstalled app exits after launching the uninstaller' $launch2.Exited
    $null = Wait-PathGone -Path $receiptPath -Seconds 90
    $null = Wait-PathExists -Path (Join-Path $context.State 'qa-uninstall-complete.txt') -Seconds 30
    Check 'delete-data uninstall completed' `
        ((Get-Content -LiteralPath (Join-Path $context.State 'qa-uninstall-complete.txt') -Raw) -match 'RemoveUserData')

    # P6: delete-data state assertions
    Check 'known database removed by delete-data uninstall' `
        (-not (Test-Path -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3')))
    Check 'known SQLite sidecars removed' (
        (-not (Test-Path -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3-wal'))) -and
        (-not (Test-Path -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3-shm'))) -and
        (-not (Test-Path -LiteralPath (Join-Path $context.Data 'alan-desktop.sqlite3-journal'))))
    Check 'known app-owned managed asset removed' `
        (-not (Test-Path -LiteralPath (Join-Path $context.Data "assets\avatar-$qaId.png")))
    Check 'receipt removed again' (-not (Test-Path -LiteralPath $receiptPath))
    Check 'runtime removed again' (
        (-not (Test-Path -LiteralPath (Join-Path $context.Install 'desktop-todo-widget.exe'))) -and
        (-not (Test-Path -LiteralPath (Join-Path $context.Install 'desktop-todo-maintenance.exe'))))
    Check 'integration removed again' (
        (-not (Test-Path -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$qaId")) -and
        (-not (Test-Path -LiteralPath (Join-Path $context.Menu 'desktop-todo-widget.lnk'))))
    Check 'unknown data-root sentinel still preserved after delete-data' `
        (Test-Path -LiteralPath (Join-Path $context.Data 'QA_UNKNOWN_DO_NOT_DELETE.txt'))
    Check 'unknown install-root sentinel still preserved after delete-data' `
        (Test-Path -LiteralPath (Join-Path $context.Install 'QA_UNKNOWN_DO_NOT_DELETE.dll'))
    Check 'data root retained because an unknown resource remains (correct result)' `
        (Test-Path -LiteralPath $context.Data -PathType Container)
}
finally {
    if ($null -ne $context -and -not $KeepSandbox) {
        Remove-Item -LiteralPath $context.Root -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$($context.QaId)" -Recurse -Force -ErrorAction SilentlyContinue
    }
    elseif ($null -ne $context) {
        Write-Host "QA context kept: $($context.Root)"
    }
    Remove-Item -LiteralPath $legacyExtract -Recurse -Force -ErrorAction SilentlyContinue
}

Assert-ProductionUnchanged -Snapshot $productionSnapshot -Stage 'after lifecycle QA'
Write-Host '  PASS  production installation unchanged (post-lifecycle verification)'

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) lifecycle checks passed"
if ($failed.Count -gt 0) {
    foreach ($check in $failed) { Write-Host "  failed: $($check.Name)" -ForegroundColor Red }
    exit 1
}
exit 0
