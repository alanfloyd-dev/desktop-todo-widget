#Requires -Version 5.1
<#
.SYNOPSIS
    Hard safety interlock for Phase 1 lifecycle QA harnesses (test-only).

.DESCRIPTION
    Every function here is a pure assertion: nothing is created, written or
    deleted. Lifecycle QA harnesses must call the right assertion before any
    filesystem/registry-mutating step, and must refuse to run (fail hard) when
    an assertion does not hold. There is deliberately no "continue with a
    warning" path: a QA run that cannot prove it is isolated is a run that
    once overwrote a real production installation.

    Background: the first bootstrap-roundtrip.ps1 draft invoked the legacy
    install.ps1 path outside a sandboxed LOCALAPPDATA and briefly replaced the
    real v1.1 production executable. It was restored byte-exact from the
    official release ZIP (SHA256-verified) and the incident is recorded in
    docs/maintenance-phase1-handoff.md. These interlocks exist so that class
    of mistake cannot recur.
#>

Set-StrictMode -Version Latest

function Get-QANormalizedPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
}

function Test-QAPathUnder {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Root
    )
    $candidate = Get-QANormalizedPath $Path
    $parent = Get-QANormalizedPath $Root
    if ($candidate -ieq $parent) { return $true }
    return $candidate.StartsWith($parent + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-QAIdentityEnv {
    param([Parameter(Mandatory = $true)][string]$QaId)
    if ($QaId -notmatch '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$') {
        throw "QA SAFETY INTERLOCK: QA id is not a canonical lowercase UUID: '$QaId'"
    }
    $envId = $env:DTW_MAINTENANCE_QA_ID
    if ([string]::IsNullOrWhiteSpace($envId)) {
        throw 'QA SAFETY INTERLOCK: DTW_MAINTENANCE_QA_ID is not set. A QA harness without an explicit QA identity must not run maintenance operations.'
    }
    if ($envId -ne $QaId) {
        throw ("QA SAFETY INTERLOCK: DTW_MAINTENANCE_QA_ID ('{0}') does not match the scenario id ('{1}')." -f $envId, $QaId)
    }
}

function Assert-QAHelperContext {
    # Guards every helper/product invocation: the QA identity is explicit and
    # the sandbox root lives inside the dedicated QA namespace under the REAL
    # LocalAppData Known Folder, and can never alias a production root.
    param(
        [Parameter(Mandatory = $true)][string]$QaId,
        [Parameter(Mandatory = $true)][string]$QaRoot
    )
    Assert-QAIdentityEnv -QaId $QaId
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        throw 'QA SAFETY INTERLOCK: LOCALAPPDATA is not set; the QA namespace cannot be located.'
    }
    $namespaceRoot = Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$QaId"
    if (-not (Test-QAPathUnder -Path $QaRoot -Root $namespaceRoot)) {
        throw ("QA SAFETY INTERLOCK: sandbox root '{0}' is not inside the QA namespace '{1}'." -f $QaRoot, $namespaceRoot)
    }
    $qaInstall = Get-QANormalizedPath (Join-Path $QaRoot 'install')
    $qaData = Get-QANormalizedPath (Join-Path $QaRoot 'data')
    $productionInstall = Get-QANormalizedPath (Join-Path $env:LOCALAPPDATA 'Programs\desktop-todo-widget')
    if ([string]::IsNullOrWhiteSpace($env:APPDATA)) {
        throw 'QA SAFETY INTERLOCK: APPDATA is not set; the production data root cannot be compared.'
    }
    $productionData = Get-QANormalizedPath (Join-Path $env:APPDATA 'net.alanfloyd.desktop')
    foreach ($pair in @(
            @{ Name = 'QA install root'; Path = $qaInstall },
            @{ Name = 'QA data root'; Path = $qaData })) {
        if ($pair.Path -ieq $productionInstall) {
            throw ("QA SAFETY INTERLOCK: {0} aliases the production install root." -f $pair.Name)
        }
        if ($pair.Path -ieq $productionData) {
            throw ("QA SAFETY INTERLOCK: {0} aliases the production data root." -f $pair.Name)
        }
    }
    # The registry namespace this scenario may touch is derived from the QA id;
    # it must never be the production ARP key.
    $registryKey = "HKCU:\Software\desktop-todo-maintenance-qa\$QaId"
    if ($registryKey -like '*CurrentVersion*') {
        throw "QA SAFETY INTERLOCK: QA registry namespace unexpectedly contains a production path fragment."
    }
}

function Assert-QAFakeEnvironment {
    # Guards install.ps1 legacy-path runs: PowerShell resolves every location
    # from the environment, so both roots must be redirected under the QA
    # sandbox before the script under test starts.
    param([Parameter(Mandatory = $true)][string]$QaRoot)
    foreach ($name in @('LOCALAPPDATA', 'APPDATA')) {
        $value = [System.Environment]::GetEnvironmentVariable($name)
        if ([string]::IsNullOrWhiteSpace($value)) {
            throw "QA SAFETY INTERLOCK: $name is not set; install.ps1 would resolve real profile locations."
        }
        if (-not (Test-QAPathUnder -Path $value -Root $QaRoot)) {
            throw ("QA SAFETY INTERLOCK: {0} ('{1}') is not sandboxed under '{2}'; install.ps1 would write outside the QA sandbox." -f $name, $value, $QaRoot)
        }
    }
}

function Assert-QAHelperIsQaBuild {
    # Runtime capability probe: a helper built with the `qa` feature accepts
    # --qa-uninstall and routes every path from the QA id; a production helper
    # rejects the mode outright. Running it against a throwaway QA id mutates
    # nothing outside the QA namespace (empty-sandbox uninstall is a no-op).
    param(
        [Parameter(Mandatory = $true)][string]$HelperExe,
        [Parameter(Mandatory = $true)][string]$QaId
    )
    if (-not (Test-Path -LiteralPath $HelperExe -PathType Leaf)) {
        throw "QA SAFETY INTERLOCK: helper binary not found: $HelperExe"
    }
    $previous = $env:DTW_MAINTENANCE_QA_ID
    try {
        $env:DTW_MAINTENANCE_QA_ID = $QaId
        # Start-Process, not `&`: the helper is a GUI-subsystem executable, so
        # a plain invocation returns immediately and never sets
        # $LASTEXITCODE, which would silently make this probe read a stale
        # exit code from an unrelated earlier native command. Polled instead
        # of -Wait because a production (non-qa) helper would answer the
        # unknown mode with a blocking native dialog — the poll turns that
        # violation into a timeout refusal instead of a hung harness.
        $process = Start-Process -FilePath $HelperExe -ArgumentList @('--qa-uninstall', 'keep') `
            -WindowStyle Hidden -PassThru
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
            Start-Sleep -Milliseconds 100
        }
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            throw 'QA SAFETY INTERLOCK: the helper binary did not answer the QA probe; it is not a qa build (a production helper blocks on a native dialog). A production helper would ignore the QA identity and write to real locations.'
        }
        $code = $process.ExitCode
    }
    finally {
        if ($null -ne $previous) { $env:DTW_MAINTENANCE_QA_ID = $previous } else { Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue }
    }
    if ($code -ne 0) {
        throw ("QA SAFETY INTERLOCK: the helper binary is not a qa build (probe exit {0}). A production helper would ignore the QA identity and write to real locations." -f $code)
    }
}

function Get-ProductionSafetySnapshot {
    # Read-only observation of the real production installation, taken before
    # any QA scenario runs. Nothing here ever writes.
    $snapshot = [ordered]@{}
    $install = Join-Path $env:LOCALAPPDATA 'Programs\desktop-todo-widget'
    $snapshot.InstallDir = $install
    $exe = Join-Path $install 'desktop-todo-widget.exe'
    if (Test-Path -LiteralPath $exe -PathType Leaf) {
        $item = Get-Item -LiteralPath $exe
        $snapshot.ExePresent = $true
        $snapshot.ExeSize = $item.Length
        $snapshot.ExeSha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
        $snapshot.InstallDirFiles = @((Get-ChildItem -LiteralPath $install -Force | ForEach-Object { $_.Name }) | Sort-Object)
    }
    else {
        $snapshot.ExePresent = $false
    }
    $snapshot.ReceiptPresent = (Test-Path -LiteralPath (Join-Path $install 'installation-receipt.json') -PathType Leaf)
    $snapshot.HelperPresent = (Test-Path -LiteralPath (Join-Path $install 'desktop-todo-maintenance.exe') -PathType Leaf)
    $data = Join-Path $env:APPDATA 'net.alanfloyd.desktop'
    $snapshot.DataDir = $data
    $snapshot.DataDirPresent = (Test-Path -LiteralPath $data -PathType Container)
    $db = Join-Path $data 'alan-desktop.sqlite3'
    $snapshot.DatabasePresent = (Test-Path -LiteralPath $db -PathType Leaf)
    $snapshot.ArpKeyPresent = (Test-Path -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\net.alanfloyd.desktop')
    return [pscustomobject]$snapshot
}

function Assert-ProductionUnchanged {
    param(
        [Parameter(Mandatory = $true)]$Snapshot,
        [Parameter(Mandatory = $true)][string]$Stage
    )
    $install = $Snapshot.InstallDir
    if ($Snapshot.ExePresent) {
        $exe = Join-Path $install 'desktop-todo-widget.exe'
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
            throw "QA SAFETY INTERLOCK ($Stage): the production executable was removed."
        }
        $size = (Get-Item -LiteralPath $exe).Length
        if ($size -ne $Snapshot.ExeSize) {
            throw "QA SAFETY INTERLOCK ($Stage): production executable size changed ($($Snapshot.ExeSize) -> $size)."
        }
        $hash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
        if ($hash -ne $Snapshot.ExeSha256) {
            throw "QA SAFETY INTERLOCK ($Stage): production executable SHA256 changed ($($Snapshot.ExeSha256) -> $hash)."
        }
        $files = @((Get-ChildItem -LiteralPath $install -Force | ForEach-Object { $_.Name }) | Sort-Object)
        $added = @($files | Where-Object { $Snapshot.InstallDirFiles -notcontains $_ })
        if ($added.Count -gt 0) {
            throw ("QA SAFETY INTERLOCK ({0}): new files appeared in the production install directory: {1}" -f $Stage, ($added -join ', '))
        }
    }
    foreach ($guard in @(
            @{ Name = 'installation receipt'; Value = $Snapshot.ReceiptPresent; Path = 'installation-receipt.json' },
            @{ Name = 'maintenance helper'; Value = $Snapshot.HelperPresent; Path = 'desktop-todo-maintenance.exe' })) {
        $present = Test-Path -LiteralPath (Join-Path $install $guard.Path) -PathType Leaf
        if ($present -ne $guard.Value) {
            throw ("QA SAFETY INTERLOCK ({0}): production {1} presence changed ({2} -> {3})." -f $Stage, $guard.Name, $guard.Value, $present)
        }
    }
    if ($Snapshot.DatabasePresent) {
        if (-not (Test-Path -LiteralPath (Join-Path $Snapshot.DataDir 'alan-desktop.sqlite3') -PathType Leaf)) {
            throw "QA SAFETY INTERLOCK ($Stage): the production database disappeared."
        }
    }
    $dataPresent = (Test-Path -LiteralPath $Snapshot.DataDir -PathType Container)
    if ($dataPresent -ne $Snapshot.DataDirPresent) {
        throw "QA SAFETY INTERLOCK ($Stage): production data directory presence changed."
    }
    $arp = (Test-Path -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\net.alanfloyd.desktop')
    if ($arp -ne $Snapshot.ArpKeyPresent) {
        throw "QA SAFETY INTERLOCK ($Stage): production uninstall registration presence changed."
    }
}

function Test-QAReplaceReady {
    # Mirrors MoveFileExW REPLACE_EXISTING: opening with DELETE access and
    # only Read sharing succeeds exactly when a replace of the file would.
    param([Parameter(Mandatory = $true)][string]$Path)
    try {
        $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Delete, [System.IO.FileShare]::Read)
        $stream.Close()
        return $true
    }
    catch { return $false }
}
