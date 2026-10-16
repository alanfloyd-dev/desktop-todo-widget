# Capture one still with the operator's windows parked outside the virtual screen
# for the duration of the frame only.
#
# Same reasoning as `media-record-demo.ps1`: hiding is ignored by Windows Terminal,
# and any split across two shell commands loses the race, because running a command
# makes the terminal visible again. Parking happens inside this process instead, so
# the frame is taken while the occluders are genuinely off-screen.
param(
  [Parameter(Mandatory = $true)][string]$Out,
  [Parameter(Mandatory = $true)][ValidateSet('floating', 'orb', 'sidebar', 'desktop')][string]$Mode,
  [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
  [string]$Exe = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'src-tauri\target\release\alan-desktop.exe'),
  [string]$ParkPid = '23484'
)

$ErrorActionPreference = 'Stop'
$capture = Join-Path $Root 'scripts\rc0-qa\media-capture.ps1'
$settings = Join-Path $Root '.rc0-evidence\rc1-settings.py'

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Park2 {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static int[] Snapshot(IntPtr h) { R r; GetWindowRect(h, out r); return new int[] { r.L, r.T, r.Rr-r.L, r.B-r.T }; }
  public static bool Move(IntPtr h, int x, int y, int w, int int_h) { return SetWindowPos(h, IntPtr.Zero, x, y, w, int_h, 0x0004 | 0x0010); }
  public static List<IntPtr> Frames(uint pid) {
    var found = new List<IntPtr>(); var sizes = new List<int>();
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      uint q; GetWindowThreadProcessId(h, out q);
      if (q != pid || !IsWindowVisible(h)) return true;
      R r; GetWindowRect(h, out r);
      found.Add(h); sizes.Add((r.Rr - r.L) * (r.B - r.T));
      return true;
    }, IntPtr.Zero);
    var order = new List<int>(); for (int i = 0; i < found.Count; i++) order.Add(i);
    order.Sort((a, b) => sizes[b].CompareTo(sizes[a]));
    var result = new List<IntPtr>(); foreach (var i in order) result.Add(found[i]);
    return result;
  }
}
"@

function Set-Mode {
  param([string]$mode, [string]$presentation)
  & python $settings set "mode=$mode" "floatingPresentation=$presentation" | Out-Null
}

function Launch-App {
  Get-Process alan-desktop, desktop-todo-widget -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 3
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9340'
  Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
  Start-Sleep -Seconds 16
}

switch ($Mode) {
  'floating' { Set-Mode 'floating' 'expanded' }
  'orb'      { Set-Mode 'floating' 'collapsed' }
  'sidebar'  { Set-Mode 'sidebar' 'expanded' }
  'desktop'  { Set-Mode 'desktop' 'expanded' }
}
Launch-App
& powershell -ExecutionPolicy Bypass -File $capture -Action focus | Out-Null

# Give the shell a moment to settle before parking anything.
Start-Sleep -Milliseconds 800

$saved = @()
try {
  $offset = 2600
  foreach ($h in [Park2]::Frames([uint32]$ParkPid)) {
    $rect = [Park2]::Snapshot($h)
    if ($rect[2] -lt 300 -or $rect[3] -lt 200) { continue }
    $saved += [pscustomobject]@{ Handle = ('0x{0:X}' -f $h.ToInt64()); X = $rect[0]; Y = $rect[1]; W = $rect[2]; H = $rect[3] }
    [void][Park2]::Move($h, $offset, 40, $rect[2], $rect[3])
    $offset += 80
  }
  Write-Output "parked $($saved.Count) window(s)"
  Start-Sleep -Milliseconds 1500

  & powershell -ExecutionPolicy Bypass -File $capture -Action focus | Out-Null
  Start-Sleep -Milliseconds 900
  & powershell -ExecutionPolicy Bypass -File $capture -Action capture -Out $Out -NoFocus
}
finally {
  Start-Sleep -Milliseconds 300
  foreach ($item in $saved) {
    $h = [IntPtr]::new([Convert]::ToInt64($item.Handle, 16))
    [void][Park2]::Move($h, [int]$item.X, [int]$item.Y, [int]$item.W, [int]$item.H)
  }
  Write-Output "restored $($saved.Count) window position(s)"
}
