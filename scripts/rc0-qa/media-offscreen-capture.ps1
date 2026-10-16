# Capture the product widget with the operator's overlapping windows moved out of
# frame for the duration of the frame only.
#
# Hiding (`SW_HIDE`) is not enough for every window: Windows Terminal ignores it and
# re-shows itself, and any approach split across two shell commands always loses the
# race, because the operator's own command execution brings the terminal back. So
# the occluders are *moved* off the virtual screen and put back afterwards, all
# inside this one process so no command boundary intervenes.
param(
  [Parameter(Mandatory = $true)][string]$Out,
  [string]$MoveHandles = '',
  [string]$ProcessName = 'alan-desktop',
  [string]$MoveTo = '',
  [switch]$NoFocus
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Off {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static string Park(IntPtr h, int x, int y, bool remember, ref int rl, ref int rt, ref int rw, ref int rh) {
    R r; GetWindowRect(h, out r);
    rl = r.L; rt = r.T; rw = r.Rr - r.L; rh = r.B - r.T;
    bool ok = SetWindowPos(h, IntPtr.Zero, x, y, rw, rh, 0x0004 | 0x0010);
    return "parked=" + ok;
  }
  public static bool Restore(IntPtr h, int x, int y, int w, int hh) {
    return SetWindowPos(h, IntPtr.Zero, x, y, w, hh, 0x0004 | 0x0010);
  }
}
"@

$saved = @()
try {
  if ($MoveHandles) {
    $x = 2600
    foreach ($entry in ($MoveHandles.Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
      $h = [IntPtr]::new([Convert]::ToInt64($entry, 16))
      $rl = 0; $rt = 0; $rw = 0; $rh = 0
      [void][Off]::Park($h, $x, 40, $true, [ref]$rl, [ref]$rt, [ref]$rw, [ref]$rh)
      $saved += [pscustomobject]@{ Handle = $entry; X = $rl; Y = $rt; W = $rw; H = $rh }
      $x += 60
    }
    Start-Sleep -Milliseconds 1200
  }

  if ($MoveTo) {
    & powershell -ExecutionPolicy Bypass -File (Join-Path $here 'media-capture.ps1') -Action probe -MoveTo $MoveTo | Out-Null
    Start-Sleep -Milliseconds 600
  }

  $captureArgs = @('-Action', 'capture', '-Out', $Out, '-ProcessName', $ProcessName)
  if ($NoFocus) { $captureArgs += '-NoFocus' }
  & powershell -ExecutionPolicy Bypass -File (Join-Path $here 'media-capture.ps1') @captureArgs

  # A capture with the widget focused first: the Acrylic host starts in its active
  # state, so the still shows the real material rather than an unfocused fallback.
  if (-not $NoFocus) { }
}
finally {
  Start-Sleep -Milliseconds 300
  foreach ($item in $saved) {
    $h = [IntPtr]::new([Convert]::ToInt64($item.Handle, 16))
    [void][Off]::Restore($h, [int]$item.X, [int]$item.Y, [int]$item.W, [int]$item.H)
  }
  Write-Output "restored $($saved.Count) window position(s)"
}
