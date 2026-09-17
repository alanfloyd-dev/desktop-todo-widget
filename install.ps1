#Requires -Version 5.1
<#
.SYNOPSIS
    Per-user install of desktop-todo-widget from a portable release payload.

.DESCRIPTION
    Copies the executable into

        %LOCALAPPDATA%\Programs\desktop-todo-widget

    and, unless -NoStartMenuShortcut is given, creates a per-user Start Menu
    shortcut. Nothing here needs administrator rights:

      * no system-wide Program Files directory,
      * no registry writes, no uninstall entry, no PATH changes,
      * no machine-wide or service state of any kind.

    What it never touches is user data. Tasks, settings, appearance profiles,
    Quick Links, and the weather cache live in

        %APPDATA%\net.alanfloyd.desktop

    which an install (including an upgrade over a running install) leaves exactly
    as it is. uninstall.ps1 preserves it too unless -RemoveUserData is passed
    explicitly.

    Re-running the script over an existing install is the supported upgrade
    path: the running app is stopped first (only processes started from this
    install directory) and program files are replaced. Runtime payload files
    that older versions installed beside the executable are no longer created
    here; the uninstaller still recognizes and removes any that a previous
    version left behind.

.PARAMETER Source
    Directory that holds the release payload: the executable (plus the
    documentation files, when present). Defaults to the script's own
    directory, so the script works when it is shipped inside the release ZIP.

.PARAMETER InstallDir
    Destination directory. Must be inside %LOCALAPPDATA%; the default is the
    documented install location.

.PARAMETER NoStartMenuShortcut
    Do not create the per-user Start Menu shortcut.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1 -Source .\unzipped -NoStartMenuShortcut

.NOTES
    This is a design-stage script for the v1.0.1 install path. See README.md
    ("Install") for the user-facing description.
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
# The standard-only product is a single executable: runtime payload files that
# older versions installed beside it are deliberately not created here, so a
# fresh install carries exactly the executable (plus documentation, when
# present).
$DocumentationFiles = @('README.md', 'README_ZH.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md')
$ShortcutName = 'desktop-todo-widget.lnk'

function Get-NormalizedPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    return $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
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
    $sourceDir = Get-NormalizedPath $Source
    $installDir = Assert-PerUserInstallDir -Path $InstallDir
    if ($sourceDir -eq $installDir) {
        throw "Source and install directory are the same directory ($sourceDir); nothing to install."
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
