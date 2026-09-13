#Requires -Version 5.1
<#
.SYNOPSIS
    Removes a per-user install of desktop-todo-widget.

.DESCRIPTION
    Runs the inverse of install.ps1:

      1. stops the app if it is running from the install directory,
      2. removes the per-user Start Menu shortcut that install.ps1 created,
      3. removes the installed program files and the install directory.

    User data is preserved by default. Tasks, settings, appearance profiles,
    Quick Links, and the weather cache live in

        %APPDATA%\net.alanfloyd.desktop

    and are only removed when -RemoveUserData is passed explicitly, which prints
    the exact paths first and asks for confirmation unless -Force is also given.

    Deletion is deliberately narrow:
      * only files inside the resolved install directory are considered,
      * every file must look like an installed program file (the executable,
        DLL/WinMD/PRI runtime payload, documentation, the payload manifest, or
        the app's own log file); anything else stops the uninstall instead of
        being deleted,
      * subdirectories in the install directory stop the uninstall,
      * the install directory itself is removed only once it is empty.

    No registry keys, PATH entries, services, or machine-wide state are involved,
    and no administrator rights are required.

.PARAMETER InstallDir
    Installation to remove. Defaults to the documented per-user location.

.PARAMETER RemoveUserData
    Also delete %APPDATA%\net.alanfloyd.desktop. Off by default.

.PARAMETER Force
    With -RemoveUserData, skip the interactive confirmation prompt. Intended for
    scripted verification only.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\uninstall.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -RemoveUserData

.NOTES
    This is a design-stage script for the v1.0.1 install path. See README.md
    ("Install") for the user-facing description.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\desktop-todo-widget'),
    [switch]$RemoveUserData,
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$DefaultInstallDirName = 'desktop-todo-widget'
$ManifestName = 'installed-payload.txt'
$ShortcutName = 'desktop-todo-widget.lnk'
$UserDataDirName = 'net.alanfloyd.desktop'
# Every file an install can legitimately leave behind. Anything else in the
# install directory is a reason to stop, not something to delete.
$AllowedFilePatterns = @(
    '^desktop-todo-widget\.exe$',
    '^alan-desktop\.exe$',
    '\.dll$',
    '\.winmd$',
    '\.pri$',
    '\.log$',
    '\.md$',
    '\.txt$',
    '^LICENSE$'
)

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

function Resolve-UninstallTarget {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][bool]$Explicit
    )
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        throw 'LOCALAPPDATA is not set. This uninstaller only knows about per-user installs.'
    }
    $normalized = Get-NormalizedPath $Path
    $localRoot = Get-NormalizedPath $env:LOCALAPPDATA
    $leaf = Split-Path -Leaf $normalized

    # The strong invariant: never delete a directory that is not a plausible
    # install target. A filesystem root, the per-user data directory itself, or a
    # well-known system directory can never be the widget's install directory.
    if (-not (Test-PathInside $normalized $localRoot) -or $normalized -eq $localRoot) {
        throw ("Refusing to remove a directory outside the per-user data directory:`n" +
            "  requested : $normalized`n" +
            "  expected  : a subdirectory of $localRoot")
    }
    if (-not $leaf -or $leaf -match '[\\/]' -or $leaf -notmatch '^[A-Za-z0-9._ -]+$') {
        throw "Refusing to remove an unexpected directory name: $normalized"
    }
    if (-not $Explicit -and $leaf -ne $DefaultInstallDirName) {
        throw ("Refusing to remove '$normalized': it is not the documented install directory " +
            "($DefaultInstallDirName). Pass -InstallDir explicitly to remove a custom location.")
    }
    if ($Explicit -and $leaf -eq 'Programs') {
        throw "Refusing to remove '$normalized': that is the per-user Programs directory itself."
    }
    return $normalized
}

function Resolve-UserDataDir {
    if ([string]::IsNullOrWhiteSpace($env:APPDATA)) {
        throw 'APPDATA is not set, so the user-data directory cannot be identified.'
    }
    $candidate = Get-NormalizedPath (Join-Path $env:APPDATA $UserDataDirName)
    $appDataRoot = Get-NormalizedPath $env:APPDATA
    # Only ever this one exact directory: a renamed or redirected path is a
    # reason to stop rather than to delete something the user did not expect.
    if ((Split-Path -Leaf $candidate) -ne $UserDataDirName -or -not (Test-PathInside $candidate $appDataRoot)) {
        throw "Refusing to consider an unexpected user-data path: $candidate"
    }
    return $candidate
}

function Get-InstalledProcesses {
    param([Parameter(Mandatory = $true)][string]$InstallDir)
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
    if ($running.Count -eq 0) {
        Write-Host '  app is not running'
        return
    }
    foreach ($process in $running) {
        Write-Host "  stopping running instance (pid $($process.Id))"
        # Closing the widget's window can stop it; the tray-resident "Quit" is
        # not available programmatically, so a forced stop is the fallback.
        try { [void]$process.CloseMainWindow() } catch { }
        try { [void]$process.WaitForExit(3000) } catch { }
        if (Get-Process -Id $process.Id -ErrorAction SilentlyContinue) {
            try { $process | Stop-Process -Force } catch { }
            Start-Sleep -Milliseconds 300
        }
        if (Get-Process -Id $process.Id -ErrorAction SilentlyContinue) {
            throw "Could not stop pid $($process.Id). Quit desktop-todo-widget from its tray menu and run the uninstaller again."
        }
    }
}

function Remove-InstalledProgramFiles {
    param([Parameter(Mandatory = $true)][string]$InstallDir)
    if (-not (Test-Path -LiteralPath $InstallDir -PathType Container)) {
        Write-Host "  install directory is already gone: $InstallDir"
        return
    }

    $directories = @(Get-ChildItem -LiteralPath $InstallDir -Directory -Force)
    if ($directories.Count -gt 0) {
        throw ("Refusing to delete ${InstallDir}: it contains subdirectories (" +
            (($directories | ForEach-Object { $_.Name }) -join ', ') +
            '). Remove them manually after checking their contents.')
    }

    $files = @(Get-ChildItem -LiteralPath $InstallDir -File -Force)
    $unexpected = @()
    foreach ($file in $files) {
        $allowed = $false
        foreach ($pattern in $AllowedFilePatterns) {
            if ($file.Name -match $pattern) { $allowed = $true; break }
        }
        if (-not $allowed) { $unexpected += $file.Name }
    }
    if ($unexpected.Count -gt 0) {
        throw ("Refusing to delete ${InstallDir}: it contains files this uninstaller does not own (" +
            (($unexpected | Sort-Object) -join ', ') +
            '). Remove them manually after checking their contents.')
    }

    foreach ($file in $files) {
        Remove-Item -LiteralPath $file.FullName -Force
        Write-Host "  removed $($file.Name)"
    }

    $remaining = @(Get-ChildItem -LiteralPath $InstallDir -Force)
    if ($remaining.Count -eq 0) {
        Remove-Item -LiteralPath $InstallDir -Force
        Write-Host "  removed directory $InstallDir"
    }
    else {
        Write-Host "  kept $InstallDir (still not empty)"
    }
}

function Remove-StartMenuShortcut {
    param([Parameter(Mandatory = $true)][string]$InstallDir)
    if ([string]::IsNullOrWhiteSpace($env:APPDATA)) { return }
    $shortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$ShortcutName"
    if (-not (Test-Path -LiteralPath $shortcutPath -PathType Leaf)) {
        Write-Host '  no Start Menu shortcut found'
        return
    }
    $target = $null
    $shell = New-Object -ComObject WScript.Shell
    try {
        # Only a shortcut that still points into this install is ours to delete.
        $target = $shell.CreateShortcut($shortcutPath).TargetPath
    }
    finally {
        [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }
    if ($target -and (Test-PathInside $target $InstallDir)) {
        Remove-Item -LiteralPath $shortcutPath -Force
        Write-Host "  removed shortcut $shortcutPath"
    }
    else {
        Write-Host "  kept $shortcutPath (it does not point into $InstallDir)"
    }
}

function Remove-UserData {
    param(
        [Parameter(Mandatory = $true)][string]$UserDataDir,
        [Parameter(Mandatory = $true)][bool]$AssumeYes
    )
    if (-not (Test-Path -LiteralPath $UserDataDir -PathType Container)) {
        Write-Host "  no user data to remove: $UserDataDir"
        return
    }

    Write-Host ''
    Write-Host '  WARNING: -RemoveUserData permanently deletes your local data:' -ForegroundColor Yellow
    Write-Host "    $UserDataDir" -ForegroundColor Yellow
    Write-Host '    tasks and task history, settings, appearance profiles, Quick Links,' -ForegroundColor Yellow
    Write-Host '    profile identity, the weather cache, and any managed background/avatar images.' -ForegroundColor Yellow
    Write-Host '    This cannot be undone. Back the directory up first if you are unsure.' -ForegroundColor Yellow
    Write-Host ''

    if (-not $AssumeYes) {
        # A non-interactive host cannot prompt. That must keep the data, never
        # assume consent: the fallback is the same as answering "no".
        $answer = $null
        try { $answer = Read-Host "Type 'remove' to confirm deleting this data" } catch { $answer = $null }
        if ($answer -ne 'remove') {
            Write-Host '  user data kept (confirmation not given); re-run with -RemoveUserData -Force to delete it without a prompt'
            return
        }
    }

    Remove-Item -LiteralPath $UserDataDir -Recurse -Force
    Write-Host "  removed user data $UserDataDir"
}

try {
    $explicitInstallDir = $PSBoundParameters.ContainsKey('InstallDir')
    $installDir = Resolve-UninstallTarget -Path $InstallDir -Explicit $explicitInstallDir

    $userDataDir = $null
    if ($RemoveUserData) { $userDataDir = Resolve-UserDataDir }

    Write-Host 'desktop-todo-widget uninstall'
    Write-Host "  target    : $installDir"

    Stop-InstalledApp -InstallDir $installDir
    Remove-StartMenuShortcut -InstallDir $installDir
    Remove-InstalledProgramFiles -InstallDir $installDir

    if ($RemoveUserData) {
        Remove-UserData -UserDataDir $userDataDir -AssumeYes ([bool]$Force)
    }
    else {
        Write-Host '  user data : preserved - %APPDATA%\net.alanfloyd.desktop (pass -RemoveUserData to delete it)'
    }

    Write-Host ''
    Write-Host 'Uninstalled desktop-todo-widget.'
    exit 0
}
catch {
    Write-Host ''
    Write-Host "uninstall failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
