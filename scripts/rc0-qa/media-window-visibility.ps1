# Hide or re-show a specific top-level window.
#
# SW_HIDE (rather than SW_MINIMIZE) is deliberate: a minimized window still gets
# rendered into the taskbar's live thumbnail, which then lands inside the captured
# widget rect. A hidden window contributes nothing to the frame. The window is
# only hidden for the duration of a capture and is shown again immediately after.
param(
  [Parameter(Mandatory = $true)][ValidateSet('hide', 'show')][string]$Action,
  [Parameter(Mandatory = $true)][string]$Handle
)

$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Vis {
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
"@

$h = [IntPtr]::new([Convert]::ToInt64($Handle, 16))
# 0 = SW_HIDE, 5 = SW_SHOW
$cmd = if ($Action -eq 'hide') { 0 } else { 5 }
$ok = [Vis]::ShowWindow($h, $cmd)
Start-Sleep -Milliseconds 600
Write-Output "$Action $Handle -> call=$ok visibleNow=$([Vis]::IsWindowVisible($h))"
