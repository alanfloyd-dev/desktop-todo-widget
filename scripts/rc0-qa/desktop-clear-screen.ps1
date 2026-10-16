# Clear the screen for Desktop-mode physical QA, and restore it afterwards.
#
# The Desktop widget is a child of SHELLDLL_DefView, i.e. permanently at the
# bottom of the z-order, so physical mouse QA is only possible while no
# top-level window covers the desktop. This helper minimizes the covering
# window(s), remembering exactly how to restore them, instead of touching the
# desktop widget itself.
param(
  [ValidateSet('clear', 'restore', 'status')]
  [string]$Action = 'status',
  [string]$StatePath = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path '.rc0-evidence\v1-desktop\covering-windows.json')
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Cover {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
}
"@
[void][Cover]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

# Windows that can cover the desktop: visible, not minimized, not the shell or
# our own product window, and large enough to swallow the widget.
function Get-Covering {
  $result = @()
  foreach ($h in [Cover]::Top()) {
    $cls = [Cover]::Cls($h)
    if ($cls -in @('Progman', 'WorkerW', 'Shell_TrayWnd', 'Tauri Window', 'Windows.UI.Core.CoreWindow')) { continue }
    if ($cls -like 'Chrome_WidgetWin*' -and $false) { continue }
    if (-not [Cover]::IsWindowVisible($h)) { continue }
    if ([Cover]::IsIconic($h)) { continue }
    $r = New-Object Cover+R
    [void][Cover]::GetWindowRect($h, [ref]$r)
    $w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
    if ($w -lt 400 -or $ht -lt 400) { continue }
    $result += [pscustomobject]@{ hwnd = ('0x{0:X}' -f $h.ToInt64()); class = $cls; pid = [Cover]::Pid($h); w = $w; h = $ht }
  }
  return $result
}

switch ($Action) {
  'status' {
    Get-Covering | Format-Table -AutoSize
  }
  'clear' {
    $covering = Get-Covering
    if (-not $covering) { Write-Host 'nothing covering the desktop'; return }
    $covering | ConvertTo-Json -Depth 3 | Set-Content -Path $StatePath -Encoding UTF8
    Write-Host "saved covering list -> $StatePath"
    foreach ($entry in $covering) {
      $h = [IntPtr][int64]$entry.hwnd
      [void][Cover]::ShowWindow($h, 6)   # SW_MINIMIZE
      Write-Host "minimized $($entry.hwnd) $($entry.class) pid=$($entry.pid)"
    }
    Start-Sleep -Seconds 2
  }
  'restore' {
    if (-not (Test-Path $StatePath)) { Write-Host "no saved state at $StatePath"; return }
    foreach ($entry in (Get-Content $StatePath -Raw | ConvertFrom-Json)) {
      $h = [IntPtr][int64]$entry.hwnd
      if ([Cover]::IsIconic($h)) {
        [void][Cover]::ShowWindow($h, 9)  # SW_RESTORE
        Write-Host "restored $($entry.hwnd) $($entry.class)"
      }
    }
    Start-Sleep -Seconds 1
  }
}
