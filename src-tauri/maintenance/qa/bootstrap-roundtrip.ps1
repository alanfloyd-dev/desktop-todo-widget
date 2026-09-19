#Requires -Version 5.1
<#
.SYNOPSIS
    Isolated bootstrap round-trip QA for the maintenance helper (test-only).

.DESCRIPTION
    Builds the qa-feature helper and the maintenance-qa product binary, then
    exercises the real bootstrap path end to end inside the dedicated QA
    namespace:

        %LOCALAPPDATA%\desktop-todo-maintenance-qa\<uuid>\

    with registry under HKCU\Software\desktop-todo-maintenance-qa\<uuid>.
    Production builds ignore DTW_MAINTENANCE_QA_ID, and this script never
    touches the production canonical install root or the real user data.

    Covered: fresh bootstrap (receipt, integrations, data preservation,
    unknown-file preservation), the admission probe (the installed product
    binary must admit as Managed), same-version reinstall determinism,
    v1.1-style adoption, malformed-receipt fail-closed, busy-lifecycle
    refusal, uninstall-journal refusal, install.ps1 delegation and its
    canonical-location / legacy-payload guards.

.NOTES
    The admission probe briefly starts the real product binary (tray icon and
    window) against the sandbox data root; the script stops the process as
    soon as the admission record appears in the sandbox QA log.
#>
[CmdletBinding()]
param(
    [switch]$KeepSandbox,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Hard safety interlock: pure assertions that refuse to run outside the QA
# sandbox. See qa-safety.ps1 for the incident this guards against.
. (Join-Path $PSScriptRoot 'qa-safety.ps1')

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$targetDir = Join-Path $repoRoot 'src-tauri\target'
$helperExe = Join-Path $targetDir 'debug\desktop-todo-maintenance.exe'
$productExe = Join-Path $targetDir 'debug\alan-desktop.exe'
$releaseZip = Join-Path $repoRoot 'release\v1.1.0\desktop-todo-widget-v1.1.0-windows-x64.zip'

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition; Detail = $Detail }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

function Invoke-Helper {
    param(
        [Parameter(Mandatory = $true)][string]$PayloadDir,
        [string[]]$Arguments = @('--install'),
        [Parameter(Mandatory = $true)]$Context
    )
    # Safety interlock before anything runs: set the QA identity first, then
    # require the interlock to prove the sandbox sits inside the QA namespace
    # and never aliases production.
    $previous = $env:DTW_MAINTENANCE_QA_ID
    $env:DTW_MAINTENANCE_QA_ID = $Context.QaId
    Assert-QAHelperContext -QaId $Context.QaId -QaRoot $Context.Root
    Assert-QAIdentityEnv -QaId $Context.QaId
    # The helper resolves its payload directory from its own location, so a
    # copy of it must be placed inside the payload first; Invoke-Helper does
    # that so callers can pass the same payload directory repeatedly.
    $payloadHelper = Join-Path $PayloadDir 'desktop-todo-maintenance.exe'
    Copy-Item -LiteralPath $helperExe -Destination $payloadHelper -Force
    $outFile = Join-Path $Context.State 'helper-last-out.txt'
    $errFile = Join-Path $Context.State 'helper-last-err.txt'
    $ErrorActionPreference = 'Continue'
    try {
        $process = Start-Process -FilePath $payloadHelper -ArgumentList $arguments `
            -WorkingDirectory $PayloadDir -Wait -PassThru `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        $code = $process.ExitCode
    }
    finally {
        $ErrorActionPreference = 'Stop'
        if ($null -ne $previous) { $env:DTW_MAINTENANCE_QA_ID = $previous } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }
    $output = ''
    foreach ($captured in @($errFile, $outFile)) {
        if (Test-Path -LiteralPath $captured -PathType Leaf) {
            $output += (Get-Content -LiteralPath $captured -Raw -ErrorAction SilentlyContinue)
        }
    }
    $log = Join-Path $Context.State 'maintenance.log'
    if (Test-Path -LiteralPath $log -PathType Leaf) {
        $output += [Environment]::NewLine + ((Get-Content -LiteralPath $log -Tail 4 | Out-String))
    }
    return [pscustomobject]@{ ExitCode = $code; Output = $output }
}

function Invoke-InstallWithResume {
    # The documented behavior for an external/system-level sharing conflict
    # (the machine's third-party real-time protection service can hold files
    # for a few seconds after a registration change): the replacement refuses
    # with a precise per-file error, and the rerun resumes. Success-expected
    # scenarios use this wrapper so the conflict is tolerated without masking
    # any other failure; fail-expected scenarios keep calling Invoke-Helper
    # directly and always surface refusals.
    param([Parameter(Mandatory = $true)][string]$PayloadDir, [Parameter(Mandatory = $true)]$Context)
    $result = Invoke-Helper -PayloadDir $PayloadDir -Context $Context
    if ($result.ExitCode -ne 0 -and $result.Output -match '0x80070005') {
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        do {
            $ready = $true
            foreach ($name in @('desktop-todo-widget.exe', 'desktop-todo-maintenance.exe')) {
                if (-not (Test-QAReplaceReady -Path (Join-Path $Context.Install $name))) { $ready = $false }
            }
            if (-not $ready) { Start-Sleep -Milliseconds 250 }
        } until ($ready -or [DateTime]::UtcNow -gt $deadline)
        $result = Invoke-Helper -PayloadDir $PayloadDir -Context $Context
    }
    return $result
}

function New-QaContext {
    param([Parameter(Mandatory = $true)][string]$Name)
    $qaId = [guid]::NewGuid().ToString()
    $root = Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$qaId"
    $context = [pscustomobject]@{
        QaId     = $qaId
        Name     = $Name
        Root     = $root
        Install  = Join-Path $root 'install'
        Data     = Join-Path $root 'data'
        Menu     = Join-Path $root 'menu'
        State    = Join-Path $root 'maintenance'
    }
    New-Item -ItemType Directory -Path $context.Install -Force | Out-Null
    return $context
}

function Remove-QaContext {
    param([Parameter(Mandatory = $true)]$Context)
    Remove-Item -LiteralPath $Context.Root -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$($Context.QaId)" -Recurse -Force -ErrorAction SilentlyContinue
}

function New-Payload {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][string]$MainExecutableSource
    )
    New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    Copy-Item -LiteralPath $helperExe -Destination (Join-Path $Directory 'desktop-todo-maintenance.exe') -Force
    Copy-Item -LiteralPath $MainExecutableSource -Destination (Join-Path $Directory 'desktop-todo-widget.exe') -Force
    Set-Content -LiteralPath (Join-Path $Directory 'README.md') -Value 'payload readme' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $Directory 'LICENSE_ZH.md') -Value 'payload license zh' -Encoding ASCII
}

function Get-Receipt {
    param([Parameter(Mandatory = $true)]$Context)
    Get-Content -LiteralPath (Join-Path $Context.Install 'installation-receipt.json') -Raw | ConvertFrom-Json
}

function Assert-DataPreserved {
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][hashtable]$Hashes,
        [Parameter(Mandatory = $true)][bool]$Exact
    )
    foreach ($name in $Hashes.Keys) {
        $path = Join-Path $Context.Data $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            Check "data preserved: $name ($($Context.Name))" $false 'file missing'
            continue
        }
        if ($Exact) {
            $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
            Check "data preserved: $name ($($Context.Name))" ($hash -eq $Hashes[$name]) 'content changed'
        }
    }
}

function Get-DataHashes {
    param([Parameter(Mandatory = $true)]$Context)
    $hashes = @{}
    foreach ($file in @(Get-ChildItem -LiteralPath $Context.Data -File)) {
        $hashes[$file.Name] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
    return $hashes
}

function Invoke-AdmissionProbe {
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$InstallationId
    )
    $previous = $env:DTW_MAINTENANCE_QA_ID
    $env:DTW_MAINTENANCE_QA_ID = $Context.QaId
    Assert-QAIdentityEnv -QaId $Context.QaId
    $logPath = Join-Path $Context.Install 'qa-diagnostics.log'
    $marker = "maintenance_admission=Managed installationId=$InstallationId"
    $process = $null
    try {
        Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
        $process = Start-Process -FilePath (Join-Path $Context.Install 'desktop-todo-widget.exe') `
            -WorkingDirectory $Context.Install -PassThru
        $deadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $deadline) {
            Start-Sleep -Milliseconds 300
            if ($process.HasExited) { break }
            if ((Test-Path -LiteralPath $logPath -PathType Leaf) -and
                ((Get-Content -LiteralPath $logPath -Raw -ErrorAction SilentlyContinue) -match [regex]::Escape($marker))) {
                break
            }
        }
        $observed = $null
        if (Test-Path -LiteralPath $logPath -PathType Leaf) {
            $observed = (Get-Content -LiteralPath $logPath -Raw) -match [regex]::Escape($marker)
        }
        Check "admission probe is Managed ($($Context.Name))" ([bool]$observed) "marker='$marker' log='$logPath' exited=$($process.HasExited)"
    }
    finally {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $previous) { $env:DTW_MAINTENANCE_QA_ID = $previous } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }
}

# --- build -------------------------------------------------------------------
if (-not $SkipBuild) {
    Write-Host 'building frontend, qa helper, and maintenance-qa product binary...'
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
    # custom-protocol embeds the real frontend, so the QA handoff hook in
    # App.vue runs and drives the production start_uninstall command
    & cargo build --manifest-path (Join-Path $repoRoot 'src-tauri\Cargo.toml') --offline --features maintenance-qa,custom-protocol --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'product build failed' }
}
foreach ($binary in @($helperExe, $productExe)) {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "missing build output: $binary (run without -SkipBuild)" }
}

# The helper validates the payload main executable's PE identity; assert the
# fixture binaries really carry the product name and matching version before
# using them.
foreach ($binary in @($productExe, $helperExe)) {
    $info = (Get-Item -LiteralPath $binary).VersionInfo
    if ($info.ProductName -ne 'desktop-todo-widget') { throw "$binary has unexpected ProductName '$($info.ProductName)'" }
    if ($info.ProductVersion -ne '1.1.0') { throw "$binary has unexpected ProductVersion '$($info.ProductVersion)'" }
}

# The v1.1 release executable stands in for a legacy installation's runtime.
New-Item -ItemType Directory -Path (Join-Path $env:TEMP 'dtw-bootstrap-qa') -Force | Out-Null
$legacyExtract = Join-Path $env:TEMP ('dtw-bootstrap-qa\' + [guid]::NewGuid().ToString('N'))
Expand-Archive -LiteralPath $releaseZip -DestinationPath $legacyExtract -Force
$legacyExe = Join-Path $legacyExtract 'desktop-todo-widget.exe'
if (-not (Test-Path -LiteralPath $legacyExe -PathType Leaf)) { throw 'v1.1 zip did not contain the expected executable' }

# --- safety interlock --------------------------------------------------------
# Observe the real production installation before anything runs, and prove the
# helper binary is a qa build (a production helper would ignore the QA identity
# and write to real locations). Both are pure/read-only or QA-namespace-only.
$productionSnapshot = Get-ProductionSafetySnapshot
Write-Host ("production exe sha256: {0}" -f $productionSnapshot.ExeSha256)
$probeId = [guid]::NewGuid().ToString()
Assert-QAHelperIsQaBuild -HelperExe $helperExe -QaId $probeId
Remove-Item -LiteralPath (Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$probeId") -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$probeId" -Recurse -Force -ErrorAction SilentlyContinue

$qaContexts = @()
try {
    # --- scenario A: fresh bootstrap -----------------------------------------
    Write-Host ''
    Write-Host 'fresh bootstrap'
    $fresh = New-QaContext 'fresh'
    $qaContexts += $fresh
    New-Item -ItemType Directory -Path (Join-Path $fresh.Data 'assets') -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $fresh.Data 'alan-desktop.sqlite3') -Value 'seeded user tasks' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $fresh.Data "avatar-$($fresh.QaId).png") -Value 'seeded image' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $fresh.Install 'unknown.dll') -Value 'foreign' -Encoding ASCII
    $dataBefore = Get-DataHashes $fresh

    $payload = Join-Path $fresh.Root 'payload'
    New-Payload -Directory $payload -MainExecutableSource $productExe
    $result = Invoke-InstallWithResume -PayloadDir $payload -Context $fresh
    Check 'fresh bootstrap exits 0' ($result.ExitCode -eq 0)

    $receipt = Get-Receipt $fresh
    Check 'receipt lifecycle is Installed' ($receipt.lifecycleState -eq 'Installed') $receipt.lifecycleState
    Check 'receipt committed manifest digest stays empty after bootstrap' ($null -eq $receipt.maintenance.committedManifestSha256)
    Check 'receipt records exactly two runtime identities' ($receipt.runtimeResources.Count -eq 2)
    Check 'receipt runtime identities are the two executables' `
        ((@($receipt.runtimeResources | ForEach-Object { $_.identity } | Sort-Object) -join ',') -eq 'mainExecutable,maintenanceHelper')
    Check 'support records never enter the runtime set' `
        (@($receipt.supportResources | Where-Object { $_.identity -in 'mainExecutable', 'maintenanceHelper' }).Count -eq 0)
    Check 'support resources recorded (readme, license zh)' `
        ((@($receipt.supportResources | ForEach-Object { $_.identity } | Sort-Object) -join ',') -eq 'licenseZh,readme')

    $reg = Get-ItemProperty -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$($fresh.QaId)" -ErrorAction SilentlyContinue
    Check 'uninstall registration exists with publisher metadata' (
        $null -ne $reg -and
        $reg.DisplayName -eq 'desktop-todo-widget' -and
        $reg.DisplayVersion -eq '1.1.0' -and
        $reg.Publisher -eq 'Alan Floyd' -and
        $reg.InstallLocation -eq $fresh.Install)
    Check 'UninstallString points at the maintenance helper' `
        ($null -ne $reg -and ($reg.UninstallString -eq ('"{0}" --uninstall' -f (Join-Path $fresh.Install 'desktop-todo-maintenance.exe')))) $reg.UninstallString
    Check 'installation id matches the receipt' ($null -ne $reg -and $reg.InstallationId -eq $receipt.installationId)

    $shell = New-Object -ComObject WScript.Shell
    try { $shortcutTarget = $shell.CreateShortcut((Join-Path $fresh.Menu 'desktop-todo-widget.lnk')).TargetPath }
    finally { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
    Check 'Start Menu shortcut targets the installed executable' `
        ($shortcutTarget -eq (Join-Path $fresh.Install 'desktop-todo-widget.exe')) $shortcutTarget

    Check 'unknown install-root file survives bootstrap' (Test-Path -LiteralPath (Join-Path $fresh.Install 'unknown.dll'))
    Assert-DataPreserved -Context $fresh -Hashes $dataBefore -Exact $true
    Check 'no stray temp files in the install root' `
        (@(Get-ChildItem -LiteralPath $fresh.Install -Filter '*.tmp' -Force).Count -eq 0)

    Invoke-AdmissionProbe -Context $fresh -InstallationId $receipt.installationId
    Check 'seeded data still present after the probe' `
        ((Test-Path -LiteralPath (Join-Path $fresh.Data 'alan-desktop.sqlite3')) -and
         (Test-Path -LiteralPath (Join-Path $fresh.Data "avatar-$($fresh.QaId).png")))

    # --- scenario A2: same-version reinstall ----------------------------------
    Write-Host 'same-version reinstall'
    $generationBefore = $receipt.maintenance.receiptGeneration
    $result = Invoke-InstallWithResume -PayloadDir $payload -Context $fresh
    Check 'reinstall exits 0' ($result.ExitCode -eq 0)
    $reinstalled = Get-Receipt $fresh
    Check 'reinstall keeps the installation id' ($reinstalled.installationId -eq $receipt.installationId)
    Check 'reinstall advances the receipt generation' `
        ($reinstalled.maintenance.receiptGeneration -gt $generationBefore)
    Check 'reinstall keeps lifecycle Installed' ($reinstalled.lifecycleState -eq 'Installed')
    Check 'reinstall keeps the unknown file' (Test-Path -LiteralPath (Join-Path $fresh.Install 'unknown.dll'))
    Assert-DataPreserved -Context $fresh -Hashes $dataBefore -Exact $true

    # --- scenario B: v1.1-style adoption ---------------------------------------
    Write-Host 'v1.1 adoption'
    $adoption = New-QaContext 'adoption'
    $qaContexts += $adoption
    New-Item -ItemType Directory -Path $adoption.Data -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $adoption.Data 'alan-desktop.sqlite3') -Value 'v1.1 user data' -Encoding ASCII
    Copy-Item -LiteralPath $legacyExe -Destination (Join-Path $adoption.Install 'desktop-todo-widget.exe') -Force
    [System.IO.File]::WriteAllText((Join-Path $adoption.Install 'README.md'), 'v1.1 readme')
    $adoptionDataBefore = Get-DataHashes $adoption

    $adoptionPayload = Join-Path $adoption.Root 'payload'
    New-Payload -Directory $adoptionPayload -MainExecutableSource $productExe
    $result = Invoke-InstallWithResume -PayloadDir $adoptionPayload -Context $adoption
    Check 'adoption bootstrap exits 0' ($result.ExitCode -eq 0)
    Check 'legacy runtime replaced by the payload executable' `
        ((Get-FileHash -LiteralPath (Join-Path $adoption.Install 'desktop-todo-widget.exe') -Algorithm SHA256).Hash -eq
         (Get-FileHash -LiteralPath $productExe -Algorithm SHA256).Hash)
    $adoptionReceipt = Get-Receipt $adoption
    Check 'adoption produces an Installed receipt' ($adoptionReceipt.lifecycleState -eq 'Installed')
    Check 'adoption did not overwrite the pre-existing document' `
        ((Get-Content -LiteralPath (Join-Path $adoption.Install 'README.md') -Raw) -eq 'v1.1 readme')
    Assert-DataPreserved -Context $adoption -Hashes $adoptionDataBefore -Exact $true
    Invoke-AdmissionProbe -Context $adoption -InstallationId $adoptionReceipt.installationId

    # --- scenario C: fail-closed guards ----------------------------------------
    Write-Host 'fail-closed guards'
    $guards = New-QaContext 'guards'
    $qaContexts += $guards
    $guardsPayload = Join-Path $guards.Root 'payload'
    New-Payload -Directory $guardsPayload -MainExecutableSource $productExe
    $result = Invoke-Helper -PayloadDir $guardsPayload -Context $guards
    Check 'guard-scenario bootstrap exits 0' ($result.ExitCode -eq 0)
    $mainHash = (Get-FileHash -LiteralPath (Join-Path $guards.Install 'desktop-todo-widget.exe') -Algorithm SHA256).Hash

    [System.IO.File]::WriteAllText((Join-Path $guards.Install 'installation-receipt.json'), '{ broken')
    $result = Invoke-Helper -PayloadDir $guardsPayload -Context $guards
    Check 'malformed receipt refuses bootstrap' ($result.ExitCode -ne 0)
    Check 'malformed receipt is preserved, not deleted or rewritten' `
        ((Get-Content -LiteralPath (Join-Path $guards.Install 'installation-receipt.json') -Raw) -eq '{ broken')
    Check 'failed bootstrap did not touch the runtime' `
        ((Get-FileHash -LiteralPath (Join-Path $guards.Install 'desktop-todo-widget.exe') -Algorithm SHA256).Hash -eq $mainHash)

    # transitional lifecycle state: bootstrap must refuse, not adopt
    $busy = New-QaContext 'busy'
    $qaContexts += $busy
    $busyPayload = Join-Path $busy.Root 'payload'
    New-Payload -Directory $busyPayload -MainExecutableSource $productExe
    $result = Invoke-Helper -PayloadDir $busyPayload -Context $busy
    Check 'busy-scenario bootstrap exits 0' ($result.ExitCode -eq 0)
    $busyReceiptPath = Join-Path $busy.Install 'installation-receipt.json'
    $busyJson = Get-Content -LiteralPath $busyReceiptPath -Raw | ConvertFrom-Json
    $busyJson.lifecycleState = 'Uninstalling'
    # WriteAllText, not Set-Content UTF8: the receipt must stay BOM-less UTF-8
    [System.IO.File]::WriteAllText($busyReceiptPath, ($busyJson | ConvertTo-Json -Depth 10))
    $result = Invoke-Helper -PayloadDir $busyPayload -Context $busy
    Check 'transitional lifecycle refuses bootstrap' ($result.ExitCode -ne 0)

    $journal = New-QaContext 'journal'
    $qaContexts += $journal
    New-Item -ItemType Directory -Path $journal.State -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $journal.State 'active-uninstall.json') -Value '{}' -Encoding ASCII
    $journalPayload = Join-Path $journal.Root 'payload'
    New-Payload -Directory $journalPayload -MainExecutableSource $productExe
    $result = Invoke-Helper -PayloadDir $journalPayload -Context $journal
    Check 'active uninstall journal refuses bootstrap' ($result.ExitCode -ne 0)

    # --- scenario D: foreign payload is rejected --------------------------------
    Write-Host 'foreign payload rejection'
    $foreign = New-QaContext 'foreign'
    $qaContexts += $foreign
    $foreignPayload = Join-Path $foreign.Root 'payload'
    New-Item -ItemType Directory -Path $foreignPayload -Force | Out-Null
    Copy-Item -LiteralPath $helperExe -Destination (Join-Path $foreignPayload 'desktop-todo-maintenance.exe') -Force
    Set-Content -LiteralPath (Join-Path $foreignPayload 'desktop-todo-widget.exe') -Value 'not a real executable' -Encoding ASCII
    $result = Invoke-Helper -PayloadDir $foreignPayload -Context $foreign
    Check 'foreign main executable refuses bootstrap' ($result.ExitCode -ne 0)
    Check 'refused foreign bootstrap created no receipt' `
        (-not (Test-Path -LiteralPath (Join-Path $foreign.Install 'installation-receipt.json')))

    # --- scenario E: install.ps1 delegation and guards --------------------------
    # install.ps1 derives every location from the environment, so all its runs
    # here use a sandboxed LOCALAPPDATA/APPDATA (same technique as
    # scripts/verify-install-scripts.ps1). The delegated helper still resolves
    # its QA sandbox from the real Known Folder, so nothing in this scenario
    # can touch the real canonical install root.
    Write-Host 'install.ps1 bootstrap integration'
    $installScript = Join-Path $repoRoot 'install.ps1'
    $delegation = New-QaContext 'delegation'
    $qaContexts += $delegation
    $delegationPayload = Join-Path $delegation.Root 'payload'
    New-Payload -Directory $delegationPayload -MainExecutableSource $productExe
    $fakeLocal = Join-Path $delegation.Root 'fake-local'
    $fakeRoaming = Join-Path $delegation.Root 'fake-roaming'
    New-Item -ItemType Directory -Path (Join-Path $fakeLocal 'Programs'), $fakeRoaming -Force | Out-Null
    $previousQa = $env:DTW_MAINTENANCE_QA_ID
    $previousLocal = $env:LOCALAPPDATA
    $previousRoaming = $env:APPDATA
    $ErrorActionPreference = 'Continue'
    try {
        $env:DTW_MAINTENANCE_QA_ID = $delegation.QaId
        $env:LOCALAPPDATA = $fakeLocal
        $env:APPDATA = $fakeRoaming
        $output = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installScript `
            -Source $delegationPayload 2>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = 'Stop'
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
        if ($null -ne $previousQa) { $env:DTW_MAINTENANCE_QA_ID = $previousQa } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }
    Check 'install.ps1 delegates a managed payload to the helper' ($code -eq 0) $output
    Check 'delegated bootstrap produced the QA receipt' `
        (Test-Path -LiteralPath (Join-Path $delegation.Install 'installation-receipt.json'))

    $customDir = Join-Path $fakeLocal 'custom-target'
    $ErrorActionPreference = 'Continue'
    try {
        $env:DTW_MAINTENANCE_QA_ID = $delegation.QaId
        $env:LOCALAPPDATA = $fakeLocal
        $env:APPDATA = $fakeRoaming
        $output = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installScript `
            -Source $delegationPayload -InstallDir $customDir 2>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = 'Stop'
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
        if ($null -ne $previousQa) { $env:DTW_MAINTENANCE_QA_ID = $previousQa } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }
    Check 'install.ps1 refuses a custom target for a managed payload' ($code -ne 0) $output
    Check 'that refusal names the canonical location' ($output -match 'canonical per-user location')
    Check 'refused custom target created nothing' (-not (Test-Path -LiteralPath $customDir))

    # legacy payload over a managed installation must be refused: seed the fake
    # canonical install with the receipt marker the guard looks for
    $fakeCanonical = Join-Path $fakeLocal 'Programs\desktop-todo-widget'
    New-Item -ItemType Directory -Path $fakeCanonical -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $fakeCanonical 'installation-receipt.json') -Value '{"seeded":true}' -Encoding ASCII
    $legacyPayload = Join-Path $delegation.Root 'legacy-payload'
    New-Item -ItemType Directory -Path $legacyPayload -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $legacyPayload 'desktop-todo-widget.exe') -Value 'legacy exe' -Encoding ASCII
    $ErrorActionPreference = 'Continue'
    try {
        $env:LOCALAPPDATA = $fakeLocal
        $env:APPDATA = $fakeRoaming
        $output = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $installScript `
            -Source $legacyPayload 2>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = 'Stop'
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
    }
    Check 'install.ps1 refuses a legacy payload over a managed installation' ($code -ne 0) $output
    Check 'that refusal explains the managed state' ($output -match 'already managed')
    Check 'managed marker survived the refused legacy install' `
        ((Get-Content -LiteralPath (Join-Path $fakeCanonical 'installation-receipt.json') -Raw) -match '"seeded":true')
    Check 'refused legacy install placed no executable' `
        (-not (Test-Path -LiteralPath (Join-Path $fakeCanonical 'desktop-todo-widget.exe')))

    # --- scenario F: interrupted install recovery ------------------------------
    # Simulates crashes after the Installing receipt was published by rewinding
    # a completed install to the exact durable state each crash point would
    # leave, then verifies that rerunning the bootstrap resumes to Installed.
    Write-Host 'interrupted install recovery'
    $interrupt = New-QaContext 'interrupt'
    $qaContexts += $interrupt
    New-Item -ItemType Directory -Path $interrupt.Data -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $interrupt.Data 'alan-desktop.sqlite3') -Value 'seeded before crash' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $interrupt.Install 'unknown.dll') -Value 'foreign' -Encoding ASCII
    $interruptDataBefore = Get-DataHashes $interrupt
    $interruptPayload = Join-Path $interrupt.Root 'payload'
    New-Payload -Directory $interruptPayload -MainExecutableSource $productExe

    function Set-ReceiptField {
        # Rewrites one receipt field with BOM-less UTF-8, exactly how a crash
        # would leave the durable Installing marker behind.
        param([Parameter(Mandatory = $true)]$Context, [Parameter(Mandatory = $true)][string]$Field, [Parameter(Mandatory = $true)]$Value)
        $path = Join-Path $Context.Install 'installation-receipt.json'
        $json = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        $json.$Field = $Value
        [System.IO.File]::WriteAllText($path, ($json | ConvertTo-Json -Depth 10))
    }
    function Start-InterruptedInstall {
        # Completed install -> Installing marker -> partial rewind.
        param(
            [Parameter(Mandatory = $true)]$Context,
            [Parameter(Mandatory = $true)][string]$PayloadDir,
            [bool]$DeleteRuntime = $true,
            [bool]$DeleteRegistry = $true,
            [bool]$DeleteShortcut = $true
        )
        Set-ReceiptField -Context $Context -Field 'lifecycleState' -Value 'Installing'
        if ($DeleteRuntime) {
            Remove-Item -LiteralPath (Join-Path $Context.Install 'desktop-todo-widget.exe') -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $Context.Install 'desktop-todo-maintenance.exe') -Force -ErrorAction SilentlyContinue
        }
        if ($DeleteRegistry) {
            Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$($Context.QaId)" -Recurse -Force -ErrorAction SilentlyContinue
        }
        if ($DeleteShortcut) {
            Remove-Item -LiteralPath (Join-Path $Context.Menu 'desktop-todo-widget.lnk') -Force -ErrorAction SilentlyContinue
        }
    }

    # A: crash right after the Installing receipt (no runtime, no integration)
    $result = Invoke-InstallWithResume -PayloadDir $interruptPayload -Context $interrupt
    Check 'interrupt baseline install exits 0' ($result.ExitCode -eq 0) $result.Output
    $interruptReceipt = Get-Receipt $interrupt
    $interruptId = $interruptReceipt.installationId
    Start-InterruptedInstall -Context $interrupt -PayloadDir $interruptPayload
    $result = Invoke-InstallWithResume -PayloadDir $interruptPayload -Context $interrupt
    Check 'crash A (nothing copied) retry exits 0' ($result.ExitCode -eq 0) $result.Output
    $resumed = Get-Receipt $interrupt
    Check 'crash A retry ends Installed' ($resumed.lifecycleState -eq 'Installed')
    Check 'crash A retry keeps the installation id' ($resumed.installationId -eq $interruptId)
    Check 'crash A retry re-created the runtime' `
        ((Test-Path -LiteralPath (Join-Path $interrupt.Install 'desktop-todo-widget.exe')) -and
         (Test-Path -LiteralPath (Join-Path $interrupt.Install 'desktop-todo-maintenance.exe')))
    Check 'crash A retry re-created the registry registration' `
        ($null -ne (Get-ItemProperty -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$($interrupt.QaId)" -ErrorAction SilentlyContinue))
    Check 'crash A retry re-created the shortcut' `
        (Test-Path -LiteralPath (Join-Path $interrupt.Menu 'desktop-todo-widget.lnk'))
    Assert-DataPreserved -Context $interrupt -Hashes $interruptDataBefore -Exact $true
    Check 'crash A retry did not adopt the unknown file' `
        (@(@($resumed.runtimeResources) + @($resumed.supportResources) |
            Where-Object { $_.identity -eq 'unknown.dll' }).Count -eq 0)

    # determinism: the retry itself is repeatable
    $generationAfterRetry = $resumed.maintenance.receiptGeneration
    $result = Invoke-InstallWithResume -PayloadDir $interruptPayload -Context $interrupt
    Check 'repeated retry exits 0' ($result.ExitCode -eq 0) $result.Output
    $repeated = Get-Receipt $interrupt
    Check 'repeated retry keeps the installation id' ($repeated.installationId -eq $interruptId)
    Check 'repeated retry advances the generation deterministically' `
        ($repeated.maintenance.receiptGeneration -gt $generationAfterRetry)

    # B: crash after the main executable was copied
    Start-InterruptedInstall -Context $interrupt -PayloadDir $interruptPayload -DeleteRuntime $false
    Remove-Item -LiteralPath (Join-Path $interrupt.Install 'desktop-todo-maintenance.exe') -Force -ErrorAction SilentlyContinue
    $result = Invoke-InstallWithResume -PayloadDir $interruptPayload -Context $interrupt
    Check 'crash B (main copied) retry exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'crash B retry ends Installed' ((Get-Receipt $interrupt).lifecycleState -eq 'Installed')

    # C: crash with runtime complete but integration partial. On this machine
    # a third-party real-time protection service (identified via Restart
    # Manager as QQPCMgr RTP) can hold the freshly registered DisplayIcon
    # executable for a couple of seconds, so the first retry may surface the
    # documented per-file conflict. That is the designed behavior: the
    # replacement refuses, the state stays resumable, and the next attempt
    # succeeds once the external holder releases the file.
    Start-InterruptedInstall -Context $interrupt -PayloadDir $interruptPayload -DeleteRuntime $false -DeleteRegistry $false
    $result = Invoke-Helper -PayloadDir $interruptPayload -Context $interrupt
    if ($result.ExitCode -ne 0) {
        Check 'crash C retry surfaces a resumable conflict on first attempt' ($result.ExitCode -ne 0) $result.Output
        $mainExe = Join-Path $interrupt.Install 'desktop-todo-widget.exe'
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        while (-not (Test-QAReplaceReady -Path $mainExe) -and [DateTime]::UtcNow -lt $deadline) {
            Start-Sleep -Milliseconds 250
        }
        $result = Invoke-Helper -PayloadDir $interruptPayload -Context $interrupt
    }
    Check 'crash C retry exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'crash C retry ends Installed' ((Get-Receipt $interrupt).lifecycleState -eq 'Installed')
    Check 'crash C retry reconciled the shortcut' `
        (Test-Path -LiteralPath (Join-Path $interrupt.Menu 'desktop-todo-widget.lnk'))
    Assert-DataPreserved -Context $interrupt -Hashes $interruptDataBefore -Exact $true

    # D: malformed Installing marker refuses and is preserved
    Set-ReceiptField -Context $interrupt -Field 'lifecycleState' -Value 'Installing'
    [System.IO.File]::WriteAllText((Join-Path $interrupt.Install 'installation-receipt.json'), '{ broken')
    $result = Invoke-Helper -PayloadDir $interruptPayload -Context $interrupt
    Check 'malformed Installing receipt refuses retry' ($result.ExitCode -ne 0) $result.Output
    Check 'malformed Installing receipt is preserved' `
        ((Get-Content -LiteralPath (Join-Path $interrupt.Install 'installation-receipt.json') -Raw) -eq '{ broken')

    # E: inconsistent Installing markers refuse, and a newer claimed version
    # is a downgrade that must be refused. Runs on a fresh context: the
    # malformed case above proved a broken receipt can never be used to reset
    # the installation, so these guards need a valid receipt to mutate.
    $interruptE = New-QaContext 'interrupt-e'
    $qaContexts += $interruptE
    $interruptPayload2 = Join-Path $interruptE.Root 'payload2'
    New-Payload -Directory $interruptPayload2 -MainExecutableSource $productExe
    $result = Invoke-InstallWithResume -PayloadDir $interruptPayload2 -Context $interruptE
    Check 'guard context install exits 0' ($result.ExitCode -eq 0) $result.Output
    Set-ReceiptField -Context $interruptE -Field 'lifecycleState' -Value 'Installing'
    Set-ReceiptField -Context $interruptE -Field 'appId' -Value 'com.foreign.app'
    $result = Invoke-Helper -PayloadDir $interruptPayload2 -Context $interruptE
    Check 'Installing receipt with wrong appId refuses retry' ($result.ExitCode -ne 0) $result.Output
    Set-ReceiptField -Context $interruptE -Field 'appId' -Value 'net.alanfloyd.desktop'
    Set-ReceiptField -Context $interruptE -Field 'installRoot' -Value 'C:\Windows'
    $result = Invoke-Helper -PayloadDir $interruptPayload2 -Context $interruptE
    Check 'Installing receipt with wrong install root refuses retry' ($result.ExitCode -ne 0) $result.Output
    Set-ReceiptField -Context $interruptE -Field 'installRoot' -Value $interruptE.Install
    Set-ReceiptField -Context $interruptE -Field 'currentVersion' -Value '9.9.9'
    $result = Invoke-Helper -PayloadDir $interruptPayload2 -Context $interruptE
    Check 'Installing receipt claiming a newer version refuses retry (no downgrade)' ($result.ExitCode -ne 0) $result.Output

    # --- scenario G: support ownership across reinstall ------------------------
    Write-Host 'support ownership preservation'
    $support = New-QaContext 'support'
    $qaContexts += $support
    $supportPayload = Join-Path $support.Root 'payload'
    New-Payload -Directory $supportPayload -MainExecutableSource $productExe
    $result = Invoke-InstallWithResume -PayloadDir $supportPayload -Context $support
    Check 'support baseline install exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'support baseline records readme and license zh' `
        ((@((Get-Receipt $support).supportResources | ForEach-Object { $_.identity } | Sort-Object) -join ',') -eq 'licenseZh,readme')
    $supportRuntimeHash = (Get-FileHash -LiteralPath (Join-Path $support.Install 'desktop-todo-widget.exe') -Algorithm SHA256).Hash

    # Reinstall from a payload that lacks the optional documents entirely: the
    # previously established ownership must survive.
    $supportPayloadBare = Join-Path $support.Root 'payload-bare'
    New-Item -ItemType Directory -Path $supportPayloadBare -Force | Out-Null
    Copy-Item -LiteralPath $helperExe -Destination (Join-Path $supportPayloadBare 'desktop-todo-maintenance.exe') -Force
    Copy-Item -LiteralPath $productExe -Destination (Join-Path $supportPayloadBare 'desktop-todo-widget.exe') -Force
    $result = Invoke-InstallWithResume -PayloadDir $supportPayloadBare -Context $support
    Check 'bare-payload reinstall exits 0' ($result.ExitCode -eq 0) $result.Output
    $supportReceipt = Get-Receipt $support
    Check 'bare-payload reinstall keeps support ownership' `
        ((@($supportReceipt.supportResources | ForEach-Object { $_.identity } | Sort-Object) -join ',') -eq 'licenseZh,readme')
    Check 'support files still on disk' `
        ((Test-Path -LiteralPath (Join-Path $support.Install 'README.md')) -and
         (Test-Path -LiteralPath (Join-Path $support.Install 'LICENSE_ZH.md')))
    Check 'runtime managed set stays exactly the two executables' `
        (($supportReceipt.runtimeResources.Count -eq 2) -and
         ((@($supportReceipt.runtimeResources | ForEach-Object { $_.identity } | Sort-Object) -join ',') -eq 'mainExecutable,maintenanceHelper'))

    # A forged receipt cannot smuggle an arbitrary owned resource into the
    # installation: unknown identity fails to parse and fails closed.
    $supportReceiptJson = Get-Content -LiteralPath (Join-Path $support.Install 'installation-receipt.json') -Raw
    $forgedSupport = $supportReceiptJson -replace '"readme"', '"arbitrary-invented-resource"'
    [System.IO.File]::WriteAllText((Join-Path $support.Install 'installation-receipt.json'), $forgedSupport)
    $result = Invoke-Helper -PayloadDir $supportPayload -Context $support
    Check 'forged support identity refuses reinstall' ($result.ExitCode -ne 0) $result.Output
    Check 'forged receipt is preserved, not rewritten' `
        ((Get-Content -LiteralPath (Join-Path $support.Install 'installation-receipt.json') -Raw) -match 'arbitrary-invented-resource')
    Check 'failed forged reinstall did not touch the runtime' `
        ((Get-FileHash -LiteralPath (Join-Path $support.Install 'desktop-todo-widget.exe') -Algorithm SHA256).Hash -eq $supportRuntimeHash)

    # --- scenario H: settings uninstall handoff ---------------------------------
    # The QA product binary drives the exact production uninstall handoff
    # (start_uninstall): launch the canonical helper, do not wait for it, exit
    # the app. The helper's QA seam stops after gate + exclusive lease, proving
    # the app's shared lease was released (no deadlock) without any deletion.
    Write-Host 'settings uninstall handoff'
    $handoff = New-QaContext 'handoff'
    $qaContexts += $handoff
    New-Item -ItemType Directory -Path $handoff.Data -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $handoff.Data 'alan-desktop.sqlite3') -Value 'seeded handoff data' -Encoding ASCII
    $handoffPayload = Join-Path $handoff.Root 'payload'
    New-Payload -Directory $handoffPayload -MainExecutableSource $productExe
    $result = Invoke-InstallWithResume -PayloadDir $handoffPayload -Context $handoff
    Check 'handoff bootstrap exits 0' ($result.ExitCode -eq 0) $result.Output
    $handoffReceipt = Get-Receipt $handoff

    $previousQa = $env:DTW_MAINTENANCE_QA_ID
    $previousHandoff = $env:DTW_MAINTENANCE_QA_HANDOFF
    $env:DTW_MAINTENANCE_QA_ID = $handoff.QaId
    $env:DTW_MAINTENANCE_QA_HANDOFF = '1'
    $logPath = Join-Path $handoff.Install 'qa-diagnostics.log'
    $marker = "maintenance_admission=Managed installationId=$($handoffReceipt.installationId)"
    $process = $null
    try {
        Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
        $process = Start-Process -FilePath (Join-Path $handoff.Install 'desktop-todo-widget.exe') `
            -WorkingDirectory $handoff.Install -PassThru
        $admitted = $false
        $deadline = [DateTime]::UtcNow.AddSeconds(60)
        while ([DateTime]::UtcNow -lt $deadline) {
            Start-Sleep -Milliseconds 300
            if ((Test-Path -LiteralPath $logPath -PathType Leaf) -and
                ((Get-Content -LiteralPath $logPath -Raw -ErrorAction SilentlyContinue) -match [regex]::Escape($marker))) {
                $admitted = $true
                break
            }
            if ($process.HasExited) { break }
        }
        Check 'handoff app admits as Managed' $admitted
        # The app drives the uninstall handoff itself; wait for its own exit.
        $exited = $process.WaitForExit(30000)
        Check 'handoff app exits after launching the helper' ([bool]$exited)
    }
    finally {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $previousQa) { $env:DTW_MAINTENANCE_QA_ID = $previousQa } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
        if ($null -ne $previousHandoff) { $env:DTW_MAINTENANCE_QA_HANDOFF = $previousHandoff } else { Remove-Item Env:\DTW_MAINTENANCE_QA_HANDOFF -ErrorAction SilentlyContinue }
    }
    # The helper polls up to 10 s for the app's lease to release after the
    # exit; poll for its marker instead of checking once.
    $seamSeen = $false
    $seamDeadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $seamDeadline) {
        if (Test-Path -LiteralPath (Join-Path $handoff.State 'qa-handoff-marker.txt') -PathType Leaf) {
            $seamSeen = $true
            break
        }
        Start-Sleep -Milliseconds 300
    }
    Check 'helper seam reached gate and exclusive lease (lease released, no deadlock)' $seamSeen
    Check 'handoff seam performed no deletion (receipt intact)' `
        ((Get-Receipt $handoff).lifecycleState -eq 'Installed')
    Check 'handoff seam performed no deletion (runtime intact)' `
        ((Test-Path -LiteralPath (Join-Path $handoff.Install 'desktop-todo-widget.exe')) -and
         (Test-Path -LiteralPath (Join-Path $handoff.Install 'desktop-todo-maintenance.exe')))
    Check 'handoff seam preserved the data root' `
        (Test-Path -LiteralPath (Join-Path $handoff.Data 'alan-desktop.sqlite3'))
}
finally {
    if (-not $KeepSandbox) {
        foreach ($context in $qaContexts) { Remove-QaContext $context }
        Remove-Item -LiteralPath $legacyExtract -Recurse -Force -ErrorAction SilentlyContinue
    }
    else {
        Write-Host "QA contexts kept: $(($qaContexts | ForEach-Object { $_.Root }) -join ', ')"
    }
}

Assert-ProductionUnchanged -Snapshot $productionSnapshot -Stage 'after QA'
Write-Host '  PASS  production installation unchanged (post-QA verification)'

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) checks passed"
if ($failed.Count -gt 0) {
    foreach ($check in $failed) { Write-Host "  failed: $($check.Name)" -ForegroundColor Red }
    exit 1
}
exit 0
