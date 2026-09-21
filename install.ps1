#Requires -Version 5.1
<#
.SYNOPSIS
    Per-user install of desktop-todo-widget from a portable release payload.

.DESCRIPTION
    Since v1.2.0 a release payload carries the maintenance helper, and this
    script is only a thin bootstrap: it checks the most basic payload
    prerequisites, then hands the whole installation to the helper
    (desktop-todo-maintenance.exe --install), which owns the canonical install
    root validation, resource ownership, the InstallationReceipt, the Start
    Menu shortcut, and the Windows uninstall registration. The script
    propagates the helper's exit code and never reports success when the
    helper failed.

    A payload without the helper (pre-maintenance releases, raw build output)
    keeps the legacy direct-copy behavior below, except that it refuses to
    touch an installation that already carries an installation receipt: a
    managed installation can only be updated by a current managed payload.

    The canonical managed install root is

        %LOCALAPPDATA%\Programs\desktop-todo-widget

    and nothing here needs administrator rights. What an install never touches
    is user data. Tasks, settings, appearance profiles, Quick Links, and the
    weather cache live in

        %APPDATA%\net.alanfloyd.desktop

    which an install (including an upgrade over a running install) leaves exactly
    as it is.

.PARAMETER Source
    Directory that holds the release payload. Defaults to the script's own
    directory, so the script works when it is shipped inside the release ZIP.

.PARAMETER InstallDir
    Destination directory for legacy (helper-less) payloads. Must be inside
    %LOCALAPPDATA%; the default is the documented install location. Managed
    payloads always install to the canonical root; a custom directory is
    refused.

.PARAMETER NoStartMenuShortcut
    Do not create the per-user Start Menu shortcut.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1 -Source .\unzipped -NoStartMenuShortcut

.NOTES
    See README.md ("Install") for the user-facing description.
#>
[CmdletBinding()]
param(
    [string]$Source = '',
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\desktop-todo-widget'),
    [switch]$NoStartMenuShortcut
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# `$PSScriptRoot` is not reliably bound in a `param()` default under
# `powershell -File`, so the payload directory is resolved here instead: the
# script's own directory, which is where the executable sits inside the release
# ZIP.
if ([string]::IsNullOrWhiteSpace($Source)) {
    $Source = $PSScriptRoot
    if ([string]::IsNullOrWhiteSpace($Source)) {
        $Source = Split-Path -Parent $MyInvocation.MyCommand.Path
    }
}

# --- Fixed install layout ----------------------------------------------------
# The installed executable always carries the public product name, independent of
# the internal crate/binary name (`alan-desktop.exe`) a raw `cargo`/`tauri` build
# produces, so shortcuts, upgrade, and uninstall have one stable target.
$InstalledExecutableName = 'desktop-todo-widget.exe'
$ExecutableCandidates = @('desktop-todo-widget.exe', 'alan-desktop.exe')
# A payload that carries the maintenance helper is a managed release: the whole
# installation is delegated to that helper (Maintenance Protocol 1 bootstrap).
$ManagedHelperName = 'desktop-todo-maintenance.exe'
$CanonicalInstallLeaf = 'Programs\desktop-todo-widget'
# The standard-only product is a single executable: runtime payload files that
# older versions installed beside it are deliberately not created here, so a
# fresh install carries exactly the executable (plus documentation, when
# present).
$DocumentationFiles = @('README.md', 'README_ZH.md', 'LICENSE', 'LICENSE_ZH.md', 'THIRD_PARTY_NOTICES.md')
$ShortcutName = 'desktop-todo-widget.lnk'
# The marker file a managed installation owns; its presence puts the install
# directory under Maintenance Protocol 1 ownership.
$ReceiptName = 'installation-receipt.json'

function Get-NormalizedPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
}

# Per-user install: nothing here needs elevation, and over-the-shoulder
# elevation (a standard user supplying an administrator's credentials) would
# resolve LOCALAPPDATA/APPDATA to the administrator's profile instead of the
# installing user's. Early UX guard only — the maintenance helper performs
# the authoritative token check before any lifecycle side effect.
function Assert-NotElevated {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw ('This is a per-user application. Run the installer normally, ' +
            'without administrator elevation ("Run as administrator"). ' +
            'No installation step requires administrator rights.')
    }
}

function Test-PathInside {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Root
    )
    $candidate = Get-NormalizedPath $Path
    $parent = Get-NormalizedPath $Root
    if ($candidate -eq $parent) { return $true }
    return $candidate.StartsWith($parent + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-PerUserInstallDir {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        throw 'LOCALAPPDATA is not set. This installer only supports per-user Windows installs.'
    }
    $normalized = Get-NormalizedPath $Path
    $localRoot = Get-NormalizedPath $env:LOCALAPPDATA
    if (-not (Test-PathInside $normalized $localRoot) -or $normalized -eq $localRoot) {
        throw ("Refusing to install outside the per-user data directory.`n" +
            "  requested : $normalized`n" +
            "  expected  : a subdirectory of $localRoot`n" +
            'System-wide locations (Program Files, Windows, ProgramData) are deliberately not supported.')
    }
    return $normalized
}

function Assert-Payload {
    param([Parameter(Mandatory = $true)][string]$SourceDir)
    if (-not (Test-Path -LiteralPath $SourceDir -PathType Container)) {
        throw "Payload directory not found: $SourceDir"
    }
    $executable = $null
    foreach ($candidate in $ExecutableCandidates) {
        $path = Join-Path $SourceDir $candidate
        if (Test-Path -LiteralPath $path -PathType Leaf) { $executable = $path; break }
    }
    if (-not $executable) {
        throw ("No release executable in $SourceDir.`n" +
            "Expected one of: $($ExecutableCandidates -join ', ')`n" +
            'Extract the full release ZIP (or build with `pnpm tauri build --no-bundle`) and run this script from that directory.')
    }
    return $executable
}

function Get-InstalledProcesses {
    param([Parameter(Mandatory = $true)][string]$InstallDir)
    # Only processes whose image actually lives in this install directory are
    # candidates. A same-named process started from somewhere else is never
    # touched.
    $processes = @()
    foreach ($name in @('desktop-todo-widget', 'alan-desktop')) {
        foreach ($process in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
            $path = $null
            try { $path = $process.Path } catch { $path = $null }
            if ($path -and (Test-PathInside $path $InstallDir)) { $processes += $process }
        }
    }
    return $processes
}

function Stop-InstalledApp {
    param([Parameter(Mandatory = $true)][string]$InstallDir)
    $running = @(Get-InstalledProcesses -InstallDir $InstallDir)
    if ($running.Count -eq 0) { return }
    foreach ($process in $running) {
        Write-Host "  stopping running instance (pid $($process.Id)) so program files can be replaced"
        # A graceful close first: the widget is tray-resident, so this usually
        # only succeeds when it still has a window. The exit is waited for, not
        # assumed, because the replacement copy fails on a locked image.
        try { [void]$process.CloseMainWindow() } catch { }
        try { [void]$process.WaitForExit(3000) } catch { }
        if (Get-Process -Id $process.Id -ErrorAction SilentlyContinue) {
            try { $process | Stop-Process -Force } catch { }
            Start-Sleep -Milliseconds 300
        }
        if (Get-Process -Id $process.Id -ErrorAction SilentlyContinue) {
            throw "Could not stop pid $($process.Id). Quit desktop-todo-widget from its tray menu and run the installer again."
        }
    }
}

function Copy-PayloadFile {
    param(
        [Parameter(Mandatory = $true)][string]$SourceFile,
        [Parameter(Mandatory = $true)][string]$DestinationFile
    )
    # Stage beside the destination and rename into place: a partially written
    # program file would be worse than a failed install, because the app would
    # start from a half-replaced payload.
    $staging = "$DestinationFile.installing"
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Force }
    Copy-Item -LiteralPath $SourceFile -Destination $staging -Force
    Move-Item -LiteralPath $staging -Destination $DestinationFile -Force
}

function Install-StartMenuShortcut {
    param(
        [Parameter(Mandatory = $true)][string]$TargetExe,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )
    if ([string]::IsNullOrWhiteSpace($env:APPDATA)) { return $null }
    $programs = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
    if (-not (Test-Path -LiteralPath $programs -PathType Container)) {
        New-Item -ItemType Directory -Path $programs -Force | Out-Null
    }
    $shortcutPath = Join-Path $programs $ShortcutName
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $TargetExe
        $shortcut.WorkingDirectory = $WorkingDirectory
        $shortcut.Description = 'desktop-todo-widget'
        $shortcut.IconLocation = "$TargetExe,0"
        $shortcut.Save()
    }
    finally {
        [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }
    return $shortcutPath
}

try {
    # First gate, before any path resolution, copy, shortcut, receipt, or
    # helper handoff.
    Assert-NotElevated

    $sourceDir = Get-NormalizedPath $Source
    $installDir = Assert-PerUserInstallDir -Path $InstallDir
    if ($sourceDir -eq $installDir) {
        throw "Source and install directory are the same directory ($sourceDir); nothing to install."
    }

    # --- managed payload: thin bootstrap into the maintenance helper ----------
    # The helper derives the payload directory from its own location and the
    # canonical roots from Windows Known Folders, so this script passes no
    # paths to it and never reimplements installation itself.
    $helperPath = Join-Path $sourceDir $ManagedHelperName
    if (Test-Path -LiteralPath $helperPath -PathType Leaf) {
        $canonicalInstallDir = Get-NormalizedPath (Join-Path $env:LOCALAPPDATA $CanonicalInstallLeaf)
        if ($installDir -ine $canonicalInstallDir) {
            throw ("Managed payloads install only to the canonical per-user location.`n" +
                "  expected : $canonicalInstallDir`n" +
                "  requested: $installDir`n" +
                'Custom install locations are not supported for managed installations.')
        }
        if (-not (Test-Path -LiteralPath (Join-Path $sourceDir $InstalledExecutableName) -PathType Leaf)) {
            throw ("Managed payload incomplete: expected $InstalledExecutableName beside $ManagedHelperName in $sourceDir.`n" +
                'Extract the full release ZIP and run this script from that directory.')
        }

        Write-Host 'desktop-todo-widget install (managed)'
        Write-Host "  source : $sourceDir"
        Write-Host "  target : $canonicalInstallDir"
        Write-Host '  handing off to the maintenance helper'

        $arguments = @('--install')
        if ($NoStartMenuShortcut) { $arguments += '--no-start-menu-shortcut' }
        # Start-Process (not `&`): the helper is a GUI-subsystem executable, so
        # a plain invocation would return before it finishes. -Wait + -PassThru
        # gives the real exit code back.
        $process = Start-Process -FilePath $helperPath -ArgumentList $arguments `
            -WorkingDirectory $sourceDir -Wait -PassThru
        $helperExitCode = $process.ExitCode
        if ($helperExitCode -ne 0) {
            throw ("the maintenance helper exited with code $helperExitCode; the installation was not completed. " +
                'See its error dialog for the reason. Recovery logs live under the app''s maintenance directory.')
        }

        Write-Host ''
        Write-Host 'Installed desktop-todo-widget as a managed installation.'
        Write-Host "  program   : $(Join-Path $canonicalInstallDir $InstalledExecutableName)"
        Write-Host '  user data : preserved (never touched by install) - %APPDATA%\net.alanfloyd.desktop'
        Write-Host '  uninstall : Windows Settings > Apps > Installed apps > desktop-todo-widget'
        exit 0
    }

    # --- legacy payload (pre-maintenance release or raw build output) ---------
    # A managed installation is never updated by a legacy payload: its receipt
    # puts the directory under maintenance ownership, and silently replacing
    # its runtime would desynchronize the receipt and the Windows registration.
    # The helper alone is equally fail-closed evidence: helper-without-receipt
    # is broken managed state that only maintenance tooling may resolve.
    $managedMarkerNames = @($ReceiptName, $ManagedHelperName)
    foreach ($markerName in $managedMarkerNames) {
        $marker = Join-Path $installDir $markerName
        if (Test-Path -LiteralPath $marker -PathType Leaf) {
            throw ("This installation is under maintenance ownership ($markerName exists in $installDir).`n" +
                'A payload without desktop-todo-maintenance.exe cannot update it. ' +
                'Install a current managed release payload, or run the installed maintenance helper.')
        }
    }

    Write-Host 'desktop-todo-widget install'
    Write-Host "  source : $sourceDir"
    Write-Host "  target : $installDir"

    $executable = Assert-Payload -SourceDir $sourceDir
    Write-Host "  payload: $(Split-Path -Leaf $executable)"

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    Stop-InstalledApp -InstallDir $installDir

    $executableName = Split-Path -Leaf $executable
    $targetExe = Join-Path $installDir $InstalledExecutableName
    Copy-PayloadFile -SourceFile $executable -DestinationFile $targetExe
    if ($executableName -ne $InstalledExecutableName) {
        Write-Host "  installed as $InstalledExecutableName (build output was $executableName)"
    }

    # Documentation is optional: a raw build output has none, and its absence
    # must not fail an install.
    foreach ($name in $DocumentationFiles) {
        $candidate = Join-Path $sourceDir $name
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            Copy-PayloadFile -SourceFile $candidate -DestinationFile (Join-Path $installDir $name)
        }
    }

    $shortcutInfo = 'skipped (-NoStartMenuShortcut)'
    if (-not $NoStartMenuShortcut) {
        $shortcutPath = Install-StartMenuShortcut -TargetExe $targetExe -WorkingDirectory $installDir
        if ($shortcutPath) { $shortcutInfo = $shortcutPath } else { $shortcutInfo = 'skipped (APPDATA is not set)' }
    }

    $version = ''
    try {
        $fileVersion = (Get-Item -LiteralPath $targetExe).VersionInfo.FileVersion
        if ($fileVersion) { $version = " $fileVersion" }
    }
    catch { }

    Write-Host ''
    Write-Host "Installed desktop-todo-widget$version"
    Write-Host "  program   : $targetExe"
    Write-Host "  shortcut  : $shortcutInfo"
    Write-Host '  user data : preserved (never touched by install) - %APPDATA%\net.alanfloyd.desktop'
    Write-Host ''
    Write-Host 'Start it from the Start Menu, or run:'
    Write-Host "  `"$targetExe`""
    Write-Host 'Remove it again with uninstall.ps1 (user data is kept unless you pass -RemoveUserData).'
    exit 0
}
catch {
    Write-Host ''
    Write-Host "install failed: $($_.Exception.Message)" -ForegroundColor Red
    if ($_.InvocationInfo) { Write-Host "  at $($_.InvocationInfo.PositionMessage.Trim())" -ForegroundColor DarkGray }
    exit 1
}
