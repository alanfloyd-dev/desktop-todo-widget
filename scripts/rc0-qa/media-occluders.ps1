# Minimise an explicit list of top-level HWNDs, and restore them afterwards.
#
# Deliberately explicit: an earlier region-based version also matched Progman and
# the Windows input-experience window, and minimising those disturbs the desktop
# itself. Only the windows the operator names are touched.
param(
  [Parameter(Mandatory = $true)][ValidateSet('minimize', 'restore', 'list')][string]$Action,
  [string]$Handles = '',
  [int]$X1 = 0, [int]$Y1 = 0, [int]$X2 = 0, [int]$Y2 = 0,
  [string[]]$ExcludeClass = @('Progman', 'WorkerW', 'Shell_TrayWnd', 'Windows.UI.Core.CoreWindow', 'CEF-OSC-WIDGET', 'Tauri Window'),
  [string]$StateFile = "$env:TEMP\dtw-media-minimised.json"
)

$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Min2 {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static bool Dpi() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static string Txt(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static bool Overlaps(IntPtr h, int x1, int y1, int x2, int y2) {
    R r; GetWindowRect(h, out r);
    if (r.Rr - r.L <= 0 || r.B - r.T <= 0) return false;
    return !(r.Rr <= x1 || r.L >= x2 || r.B <= y1 || r.T >= y2);
  }
}
"@
[void][Min2]::Dpi()

if ($Action -eq 'list') {
  $rows = @()
  foreach ($h in [Min2]::All()) {
    if (-not [Min2]::IsWindowVisible($h)) { continue }
    if ([Min2]::IsIconic($h)) { continue }
    $class = [Min2]::Cls($h)
    if ($ExcludeClass -contains $class) { continue }
    if (-not [Min2]::Overlaps($h, $X1, $Y1, $X2, $Y2)) { continue }
    $rows += [pscustomobject]@{
      Handle = ('0x{0:X}' -f $h.ToInt64())
      Raw    = $h.ToInt64()
      Class  = $class
      Title  = [Min2]::Txt($h)
      Pid    = [Min2]::Pid($h)
    }
  }
  $rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Output
  Write-Output "candidates: $($rows.Count)"
  Write-Output ("handles: " + (($rows | ForEach-Object { $_.Handle }) -join ','))
}

elseif ($Action -eq 'minimize') {
  if (-not $Handles) { throw 'pass -Handles 0x...,0x...' }
  $list = $Handles.Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ }
  $saved = @()
  foreach ($entry in $list) {
    $h = [IntPtr]::new([Convert]::ToInt64($entry, 16))
    if ([Min2]::IsWindowVisible($h) -and -not [Min2]::IsIconic($h)) {
      $saved += [pscustomobject]@{ Handle = $entry; Raw = $h.ToInt64(); Class = [Min2]::Cls($h); Title = [Min2]::Txt($h) }
      [void][Min2]::ShowWindow($h, 6)   # SW_MINIMIZE
    }
  }
  $saved | ConvertTo-Json -Depth 3 | Set-Content $StateFile -Encoding UTF8
  Start-Sleep -Milliseconds 1500
  $saved | Format-Table -AutoSize | Out-String -Width 200 | Write-Output
  Write-Output "minimised $($saved.Count); state -> $StateFile"
}

else {
  if (-not (Test-Path $StateFile)) { Write-Output "no state file"; exit 0 }
  $saved = Get-Content $StateFile -Raw | ConvertFrom-Json
  $count = 0
  foreach ($item in $saved) {
    [void][Min2]::ShowWindow([IntPtr]::new([int64]$item.Raw), 9)   # SW_RESTORE
    $count++
  }
  Start-Sleep -Milliseconds 800
  Remove-Item $StateFile -Force
  Write-Output "restored $count window(s)"
}
