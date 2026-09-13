#Requires -Version 5.1
<#
.SYNOPSIS
    Verifies that the release build no longer opens a console window.

.DESCRIPTION
    Three independent checks:

      1. PE subsystem of the release executable is IMAGE_SUBSYSTEM_WINDOWS_GUI (2).
         This is the static cause of the fix: the Windows loader allocates a
         console for a console-subsystem image and never for a GUI-subsystem one.
      2. The debug executable (when present) still reports
         IMAGE_SUBSYSTEM_WINDOWS_CUI (3), proving the change is release-only and
         that debug builds keep their console for `eprintln!` diagnostics.
      3. A shell launch - the same path a double-click takes - adds no console
         window for the release executable. The debug executable is launched the
         same way as a control case, so a passing run proves the detector actually
         sees console windows rather than being blind to them.

    The product's own window, tray icon and WebView2 surface are checked by
    scripts/verify-first-use-ux.mjs, which drives the running app over its
    WebView2 DevTools endpoint.

.PARAMETER ReleaseExe
    Release executable. Defaults to the standard cargo output path.

.PARAMETER ControlExe
    Debug executable used as the positive control. Skipped when absent.

.PARAMETER SettleSeconds
    How long to wait after a shell launch before looking for console windows.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/verify-no-console-window.ps1
#>
[CmdletBinding()]
param(
    [string]$ReleaseExe = '',
    [string]$ControlExe = '',
    [string]$AppDataRoot = '',
    [int]$SettleSeconds = 14
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Resolved in the body, not as parameter defaults: `$PSScriptRoot` is not
# reliably bound while a `powershell -File` script is binding its parameters.
if ([string]::IsNullOrWhiteSpace($ReleaseExe)) {
    $ReleaseExe = Join-Path $PSScriptRoot '..\src-tauri\target\release\alan-desktop.exe'
}
if ([string]::IsNullOrWhiteSpace($ControlExe)) {
    $ControlExe = Join-Path $PSScriptRoot '..\src-tauri\target\debug\alan-desktop.exe'
}
if ([string]::IsNullOrWhiteSpace($AppDataRoot)) {
    $AppDataRoot = Join-Path $env:TEMP 'dtw-no-console-qa'
}

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class ConsoleProbe {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public static List<IntPtr> TopLevel() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static string Txt(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Describe(IntPtr h) { return Cls(h) + "|" + Pid(h) + "|" + Txt(h); }
  public static bool Visible(IntPtr h) { return IsWindowVisible(h); }
}
"@

$checks = @()
function Check {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    $script:checks += [pscustomobject]@{ Name = $Name; Pass = [bool]$Condition }
    if ($Condition) { Write-Host "  PASS  $Name" }
    else { Write-Host "  FAIL  $Name  $Detail" -ForegroundColor Red }
}

function Get-PeSubsystem {
    param([Parameter(Mandatory = $true)][string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    return [BitConverter]::ToUInt16($bytes, $peOffset + 24 + 68)
}

# Console windows are hosted, so the window class depends on the configured
# default terminal application: classic conhost uses ConsoleWindowClass, Windows
# Terminal (the Windows 11 default) hosts the same console in a
# CASCADIA_HOSTING_WINDOW_CLASS window, and either way a PseudoConsoleWindow
# belongs to the console. A visible window of any of these classes is a console
# window a user would see.
$ConsoleWindowClasses = @('ConsoleWindowClass', 'CASCADIA_HOSTING_WINDOW_CLASS', 'PseudoConsoleWindow')

function Get-ConsoleWindowKeys {
    $keys = @()
    foreach ($handle in [ConsoleProbe]::TopLevel()) {
        if (-not [ConsoleProbe]::Visible($handle)) { continue }
        if ($ConsoleWindowClasses -notcontains [ConsoleProbe]::Cls($handle)) { continue }
        $keys += [ConsoleProbe]::Describe($handle)
    }
    return $keys
}

function Wait-ForProcess {
    param([Parameter(Mandatory = $true)][string]$Name, [int]$Seconds = 20)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $process = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($process) { return $process }
        Start-Sleep -Milliseconds 400
    }
    return $null
}

function Stop-ProductProcesses {
    Get-Process -Name 'alan-desktop', 'desktop-todo-widget' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
}

$releaseExe = (Resolve-Path -LiteralPath $ReleaseExe).Path
$controlAvailable = Test-Path -LiteralPath $ControlExe -PathType Leaf
$controlExe = if ($controlAvailable) { (Resolve-Path -LiteralPath $ControlExe).Path } else { '' }

Write-Host 'desktop-todo-widget console-window verification'
Write-Host "  release : $releaseExe"
if ($controlAvailable) { Write-Host "  control : $controlExe" } else { Write-Host '  control : (missing, skipped)' }
Write-Host ''

Write-Host 'PE subsystem'
$releaseSubsystem = Get-PeSubsystem -Path $releaseExe
Check 'release executable is a GUI-subsystem image (2)' ($releaseSubsystem -eq 2) "subsystem=$releaseSubsystem"
if ($controlAvailable) {
    $controlSubsystem = Get-PeSubsystem -Path $controlExe
    Check 'debug executable is still a console-subsystem image (3)' ($controlSubsystem -eq 3) "subsystem=$controlSubsystem"
}

$sandbox = [System.IO.Path]::GetFullPath((Join-Path $env:TEMP ('dtw-no-console-' + [guid]::NewGuid().ToString('N').Substring(0, 8))))
New-Item -ItemType Directory -Path $sandbox -Force | Out-Null
$env:APPDATA = $sandbox

function Invoke-ShellLaunchCase {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][bool]$ExpectConsoleWindow
    )
    Stop-ProductProcesses
    $before = @(Get-ConsoleWindowKeys)
    # The process name follows the tested file: the release payload ships the
    # binary under its public product name (`desktop-todo-widget.exe`) while the
    # cargo build output keeps the internal one (`alan-desktop.exe`).
    $processName = [System.IO.Path]::GetFileNameWithoutExtension($Path)
    Write-Host "$Label launch (explorer.exe, no parent console)"
    # `explorer.exe <exe>` is the shell launch a double-click performs: the child
    # inherits no console from this script, so a console-subsystem image has to
    # allocate one of its own.
    Start-Process -FilePath 'explorer.exe' -ArgumentList "`"$Path`""
    $process = Wait-ForProcess -Name $processName -Seconds 25
    if (-not $process) {
        Check "$Label process started" $false 'process did not appear'
        return
    }
    Start-Sleep -Seconds $SettleSeconds
    $after = @(Get-ConsoleWindowKeys)
    $new = @($after | Where-Object { $before -notcontains $_ })
    $windowCount = @(Get-Process -Id $process.Id -ErrorAction SilentlyContinue).Count
    Check "$Label process is alive" ($windowCount -eq 1)
    if ($ExpectConsoleWindow) {
        Check "$Label opened a console window (detector sanity check)" ($new.Count -gt 0) "new console windows: $($new.Count)"
        if ($new.Count -gt 0) { Write-Host "        detected: $($new -join '; ')" }
    }
    else {
        Check "$Label opened no console window" ($new.Count -eq 0) "unexpected console windows: $($new -join '; ')"
    }
    $windowDump = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
    Check "$Label process is responding" ($windowDump -and $windowDump.Responding)
    Stop-ProductProcesses
    Start-Sleep -Seconds 1
}

try {
    Invoke-ShellLaunchCase -Path $releaseExe -Label 'release' -ExpectConsoleWindow $false
    if ($controlAvailable) {
        Invoke-ShellLaunchCase -Path $controlExe -Label 'debug (control)' -ExpectConsoleWindow $true
    }
    $leftover = @(Get-Process -Name 'alan-desktop', 'desktop-todo-widget' -ErrorAction SilentlyContinue)
    $leftoverDetail = ($leftover | ForEach-Object { $_.Id }) -join ','
    Check 'no product process left running' ($leftover.Count -eq 0) "still running: $leftoverDetail"
}
finally {
    Stop-ProductProcesses
    Remove-Item -LiteralPath $sandbox -Recurse -Force -ErrorAction SilentlyContinue
}

$failed = @($checks | Where-Object { -not $_.Pass })
Write-Host ''
Write-Host "$($checks.Count - $failed.Count)/$($checks.Count) checks passed"
if ($failed.Count -gt 0) {
    foreach ($check in $failed) { Write-Host "  failed: $($check.Name)" -ForegroundColor Red }
    exit 1
}
exit 0
