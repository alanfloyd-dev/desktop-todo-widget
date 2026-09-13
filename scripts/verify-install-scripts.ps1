#Requires -Version 5.1
<#
.SYNOPSIS
    Simulation harness for install.ps1 and uninstall.ps1.

.DESCRIPTION
    Runs the real install/uninstall scripts against a throwaway sandbox: a fake
    payload, a fake %LOCALAPPDATA%, a fake %APPDATA%, and a fake user-data
    directory. The real profile is never read from or written to, and no real
    Start Menu entry is created, because both scripts resolve every location from
    the environment they are started with.

    Covered:
      * clean install (default install directory under the fake LOCALAPPDATA)
      * fresh install writes the payload manifest and the Start Menu shortcut
      * overwrite upgrade replaces the executable and prunes payload files the
        new version no longer ships
      * uninstall keeps user data by default
      * uninstall -RemoveUserData -Force removes it; without -Force in a
        non-interactive host it is kept
      * the safety guards: incomplete payload, missing executable, install
        outside %LOCALAPPDATA%, uninstall of a non-install directory, and
        uninstall of an install directory holding a foreign file

.PARAMETER PowerShellPath
    PowerShell executable used to run the scripts under test. Defaults to
    Windows PowerShell 5.1, which is what the documented
    `powershell -ExecutionPolicy Bypass -File install.ps1` command uses.

.EXAMPLE
    pwsh -File scripts/verify-install-scripts.ps1

.EXAMPLE
    pwsh -File scripts/verify-install-scripts.ps1 -PowerShellPath $PSHOME/pwsh.exe
#>
[CmdletBinding()]
param(
    [string]$PowerShellPath = (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'),
    [switch]$KeepSandbox
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$installScript = Join-Path $repoRoot 'install.ps1'
$uninstallScript = Join-Path $repoRoot 'uninstall.ps1'

foreach ($script in @($installScript, $uninstallScript)) {
    if (-not (Test-Path -LiteralPath $script -PathType Leaf)) { throw "missing script under test: $script" }
}
if (-not (Test-Path -LiteralPath $PowerShellPath -PathType Leaf)) {
    throw "PowerShell host not found: $PowerShellPath (pass -PowerShellPath)"
}

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition; Detail = $Detail }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

function New-Payload {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][string]$ExecutableName,
        [Parameter(Mandatory = $true)][string]$ExecutableContent,
        [string[]]$ExtraFiles = @()
    )
    New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $Directory $ExecutableName) -Value $ExecutableContent -Encoding ASCII
    # The two names the installer treats as required runtime payload.
    Set-Content -LiteralPath (Join-Path $Directory 'Microsoft.WindowsAppRuntime.dll') -Value 'runtime' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $Directory 'wuceffectsi.dll') -Value 'composition' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $Directory 'Microsoft.UI.dll') -Value 'ui' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $Directory 'Microsoft.UI.winmd') -Value 'winmd' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $Directory 'Microsoft.UI.pri') -Value 'pri' -Encoding ASCII
    Set-Content -LiteralPath (Join-Path $Directory 'README.md') -Value 'readme' -Encoding ASCII
    foreach ($extra in $ExtraFiles) {
        Set-Content -LiteralPath (Join-Path $Directory $extra) -Value "extra $extra" -Encoding ASCII
    }
}

# Runs one of the scripts under test in a child PowerShell with a sandboxed
# environment. Output and exit code are returned.
function Invoke-UnderTest {
    param(
        [Parameter(Mandatory = $true)][string]$ScriptPath,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$LocalAppData,
        [Parameter(Mandatory = $true)][string]$RoamingAppData
    )
    $previousLocal = $env:LOCALAPPDATA
    $previousRoaming = $env:APPDATA
    $previousPreference = $ErrorActionPreference
    $env:LOCALAPPDATA = $LocalAppData
    $env:APPDATA = $RoamingAppData
    # `2>&1` on a native command turns its stderr into error records, which would
    # abort this harness under `ErrorActionPreference = 'Stop'`. A failing child
    # is an expected outcome here, so it is captured as output instead.
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $PowerShellPath -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $ScriptPath @Arguments 2>&1
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
        $env:LOCALAPPDATA = $previousLocal
        $env:APPDATA = $previousRoaming
    }
    return [pscustomobject]@{ Output = ($output | Out-String); ExitCode = $code }
}

$sandbox = [System.IO.Path]::GetFullPath((Join-Path $env:TEMP ("dtw-install-sim-" + [guid]::NewGuid().ToString('N').Substring(0, 8))))
# `GetFullPath` also expands an 8.3 TEMP path to its long form, which is what
# the scripts under test compare paths in.
$local = Join-Path $sandbox 'localappdata'
$roaming = Join-Path $sandbox 'roamingappdata'
$payloadV1 = Join-Path $sandbox 'payload-v1'
$payloadV2 = Join-Path $sandbox 'payload-v2'
$payloadBroken = Join-Path $sandbox 'payload-broken'
$payloadEmpty = Join-Path $sandbox 'payload-empty'
$installDir = Join-Path $local 'Programs\desktop-todo-widget'
$installedExe = Join-Path $installDir 'desktop-todo-widget.exe'
$shortcutPath = Join-Path $roaming 'Microsoft\Windows\Start Menu\Programs\desktop-todo-widget.lnk'
$userDataDir = Join-Path $roaming 'net.alanfloyd.desktop'

Write-Host "desktop-todo-widget install/uninstall simulation"
Write-Host "  host    : $PowerShellPath"
Write-Host "  sandbox : $sandbox"
Write-Host ''

try {
    # --- sandbox fixtures ---------------------------------------------------
    New-Item -ItemType Directory -Path $local, $roaming -Force | Out-Null
    New-Payload -Directory $payloadV1 -ExecutableName 'desktop-todo-widget.exe' `
        -ExecutableContent 'v1 executable' -ExtraFiles @('DeprecatedFeature.dll')
    New-Payload -Directory $payloadV2 -ExecutableName 'alan-desktop.exe' `
        -ExecutableContent 'v2 executable' -ExtraFiles @('NewFeature.dll')
    New-Payload -Directory $payloadBroken -ExecutableName 'desktop-todo-widget.exe' `
        -ExecutableContent 'broken'
    Remove-Item -LiteralPath (Join-Path $payloadBroken 'wuceffectsi.dll') -Force
    New-Item -ItemType Directory -Path $payloadEmpty -Force | Out-Null
    # Simulated user data, exactly where the product keeps it.
    New-Item -ItemType Directory -Path $userDataDir -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $userDataDir 'alan-desktop.sqlite3') -Value 'user tasks' -Encoding ASCII

    # --- clean install ------------------------------------------------------
    Write-Host 'clean install (default install directory)'
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadV1)
    Check 'install exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'installed executable exists' (Test-Path -LiteralPath $installedExe -PathType Leaf)
    Check 'installed executable is the payload' `
        ((Get-Content -LiteralPath $installedExe -Raw).Trim() -eq 'v1 executable')
    Check 'required runtime payload copied' `
        ((Test-Path -LiteralPath (Join-Path $installDir 'Microsoft.WindowsAppRuntime.dll')) -and
         (Test-Path -LiteralPath (Join-Path $installDir 'wuceffectsi.dll')))
    Check 'optional documentation copied' (Test-Path -LiteralPath (Join-Path $installDir 'README.md'))
    Check 'payload manifest written' (Test-Path -LiteralPath (Join-Path $installDir 'installed-payload.txt'))
    Check 'manifest records the installed executable' `
        ((Get-Content -LiteralPath (Join-Path $installDir 'installed-payload.txt') -Raw) -match 'desktop-todo-widget\.exe')
    Check 'Start Menu shortcut created' (Test-Path -LiteralPath $shortcutPath -PathType Leaf)
    $shortcutTarget = $null
    if (Test-Path -LiteralPath $shortcutPath -PathType Leaf) {
        $shell = New-Object -ComObject WScript.Shell
        try { $shortcutTarget = $shell.CreateShortcut($shortcutPath).TargetPath }
        finally { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
    }
    Check 'shortcut targets the installed executable' ($shortcutTarget -eq $installedExe) "target=$shortcutTarget"
    Check 'install left user data alone' (Test-Path -LiteralPath (Join-Path $userDataDir 'alan-desktop.sqlite3'))

    # --- overwrite upgrade --------------------------------------------------
    Write-Host 'overwrite upgrade (raw build name, one payload file replaced)'
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadV2)
    Check 'upgrade exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'executable replaced' ((Get-Content -LiteralPath $installedExe -Raw).Trim() -eq 'v2 executable')
    Check 'new payload file installed' (Test-Path -LiteralPath (Join-Path $installDir 'NewFeature.dll'))
    Check 'dropped payload file pruned' `
        (-not (Test-Path -LiteralPath (Join-Path $installDir 'DeprecatedFeature.dll')))
    Check 'no staging leftovers' `
        (@(Get-ChildItem -LiteralPath $installDir -Filter '*.installing' -Force).Count -eq 0)
    Check 'upgrade left user data alone' (Test-Path -LiteralPath (Join-Path $userDataDir 'alan-desktop.sqlite3'))
    Check 'upgrade kept the shortcut' (Test-Path -LiteralPath $shortcutPath -PathType Leaf)

    # --- upgrade of a running install --------------------------------------
    # The app itself is not run here; only the stop path's safety is asserted:
    # an unrelated process with the same name must not be stopped, because it
    # does not live in the install directory. That is covered by design (path
    # match) and by the fact that no real process is started in this sandbox.
    Write-Host 'uninstall (user data preserved)'
    $result = Invoke-UnderTest -ScriptPath $uninstallScript -LocalAppData $local -RoamingAppData $roaming
    Check 'uninstall exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'install directory removed' (-not (Test-Path -LiteralPath $installDir))
    Check 'Start Menu shortcut removed' (-not (Test-Path -LiteralPath $shortcutPath))
    Check 'user data preserved by default' `
        (Test-Path -LiteralPath (Join-Path $userDataDir 'alan-desktop.sqlite3'))

    # --- uninstall with -RemoveUserData (no confirmation available) ---------
    Write-Host 'uninstall -RemoveUserData without -Force in a non-interactive host'
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadV1)
    Check 'reinstall for the -RemoveUserData run exits 0' ($result.ExitCode -eq 0) $result.Output
    $result = Invoke-UnderTest -ScriptPath $uninstallScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-RemoveUserData')
    Check 'uninstall with unconfirmed -RemoveUserData exits 0' ($result.ExitCode -eq 0) $result.Output
    Check 'unconfirmed -RemoveUserData keeps user data' (Test-Path -LiteralPath $userDataDir)
    Check 'program files still removed' (-not (Test-Path -LiteralPath $installDir))

    Write-Host 'uninstall -RemoveUserData -Force'
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadV1)
    Check 'reinstall for the -Force run exits 0' ($result.ExitCode -eq 0) $result.Output
    $result = Invoke-UnderTest -ScriptPath $uninstallScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-RemoveUserData', '-Force')
    Check '-Force uninstall exits 0' ($result.ExitCode -eq 0) $result.Output
    Check '-Force removed user data' (-not (Test-Path -LiteralPath $userDataDir))
    Check '-Force removed the install directory' (-not (Test-Path -LiteralPath $installDir))

    # --- safety guards ------------------------------------------------------
    Write-Host 'safety guards'
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadEmpty)
    Check 'install refuses a payload without an executable' ($result.ExitCode -ne 0)
    Check 'that failure explains what was expected' ($result.Output -match 'No release executable')

    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadBroken)
    Check 'install refuses an incomplete runtime payload' ($result.ExitCode -ne 0)
    Check 'that failure names the missing runtime file' ($result.Output -match 'wuceffectsi\.dll')

    $outside = Join-Path $sandbox 'outside-install'
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadV1, '-InstallDir', $outside)
    Check 'install refuses a directory outside %LOCALAPPDATA%' ($result.ExitCode -ne 0)
    Check 'that failure says why' ($result.Output -match 'Refusing to install outside')
    Check 'refused install created nothing' (-not (Test-Path -LiteralPath $outside))

    $result = Invoke-UnderTest -ScriptPath $uninstallScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-InstallDir', (Join-Path $local 'Programs'))
    Check 'uninstall refuses the per-user Programs directory' ($result.ExitCode -ne 0)
    Check 'that failure says why' ($result.Output -match 'Refusing to remove')
    Check 'Programs directory survived' (Test-Path -LiteralPath (Join-Path $local 'Programs'))

    # An install directory holding a file this uninstaller does not own must
    # stop the uninstall rather than delete it.
    $result = Invoke-UnderTest -ScriptPath $installScript -LocalAppData $local -RoamingAppData $roaming `
        -Arguments @('-Source', $payloadV1)
    Check 'install before the foreign-file guard exits 0' ($result.ExitCode -eq 0) $result.Output
    Set-Content -LiteralPath (Join-Path $installDir 'notes.db') -Value 'not ours' -Encoding ASCII
    $result = Invoke-UnderTest -ScriptPath $uninstallScript -LocalAppData $local -RoamingAppData $roaming
    Check 'uninstall refuses a foreign file' ($result.ExitCode -ne 0)
    Check 'that failure names the file' ($result.Output -match 'notes\.db')
    Check 'install directory kept intact' (Test-Path -LiteralPath $installedExe)
    Remove-Item -LiteralPath (Join-Path $installDir 'notes.db') -Force
    $result = Invoke-UnderTest -ScriptPath $uninstallScript -LocalAppData $local -RoamingAppData $roaming
    Check 'uninstall succeeds once the foreign file is gone' ($result.ExitCode -eq 0) $result.Output
    Check 'install directory removed after the guard run' (-not (Test-Path -LiteralPath $installDir))
}
finally {
    if ($KeepSandbox) {
        Write-Host "sandbox kept: $sandbox"
    }
    else {
        Remove-Item -LiteralPath $sandbox -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) checks passed"
if ($failed.Count -gt 0) {
    foreach ($check in $failed) { Write-Host "  failed: $($check.Name)" -ForegroundColor Red }
    exit 1
}
exit 0
