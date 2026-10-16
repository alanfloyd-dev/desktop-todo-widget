# RC-0 diagnostic probe for the live product window.
#
# Reports the real Win32 style flags, geometry and DPI of the Tauri top-level
# window, verified against the running process rather than the source alone.
#
# Two correctness details matter here:
#   1. WebView2 spawns additional same-named child processes, so the product
#      window is resolved by matching the process that owns a WRY_WEBVIEW child
#      instead of trusting Process.MainWindowHandle.
#   2. This process must opt into per-monitor DPI awareness before it reads any
#      window rect. A DPI-unaware probe receives Windows-virtualised (scaled
#      down) rectangles, which silently misreports geometry on a 150% display.
param(
  [string]$ProcessName = "alan-desktop",
  [int]$ResizeTo = 0
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Rc0Win {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr GetWindowLongPtrW(IntPtr hWnd, int nIndex);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetDpiForWindow(IntPtr hWnd);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder buf, int max);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

  // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == (HANDLE)-4
  public static bool MakePerMonitorAware() {
    return SetProcessDpiAwarenessContext(new IntPtr(-4));
  }
  public static List<IntPtr> TopLevel() {
    var list = new List<IntPtr>();
    EnumWindows((h, p) => { list.Add(h); return true; }, IntPtr.Zero);
    return list;
  }
  public static List<IntPtr> Children(IntPtr parent) {
    var list = new List<IntPtr>();
    EnumChildWindows(parent, (h, p) => { list.Add(h); return true; }, IntPtr.Zero);
    return list;
  }
  public static string ClassOf(IntPtr h) {
    var sb = new StringBuilder(256);
    GetClassNameW(h, sb, sb.Capacity);
    return sb.ToString();
  }
  public static uint PidOf(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }
}
"@

$aware = [Rc0Win]::MakePerMonitorAware()
if (-not $aware) {
  # Already set by a manifest/embedding host; per-thread override still applies.
  [void][Rc0Win]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
  Write-Output "dpi-awareness  : process call refused (already set); per-thread override applied"
} else {
  Write-Output "dpi-awareness  : process set to PER_MONITOR_AWARE_V2"
}

function Get-Rect([IntPtr]$h) {
  $wr = New-Object Rc0Win+RECT
  $cr = New-Object Rc0Win+RECT
  [void][Rc0Win]::GetWindowRect($h, [ref]$wr)
  [void][Rc0Win]::GetClientRect($h, [ref]$cr)
  return @{
    OuterW = $wr.Right - $wr.Left
    OuterH = $wr.Bottom - $wr.Top
    X      = $wr.Left
    Y      = $wr.Top
    ClientW = $cr.Right - $cr.Left
    ClientH = $cr.Bottom - $cr.Top
  }
}

function Describe([IntPtr]$hwnd, [string]$label) {
  $r = Get-Rect $hwnd
  $style = [Rc0Win]::GetWindowLongPtrW($hwnd, -16).ToInt64()
  $dpi = [Rc0Win]::GetDpiForWindow($hwnd)
  $scale = $dpi / 96.0
  $thick = (($style -band 0x00040000) -eq 0x00040000)
  $lines = @(
    "$label hwnd=0x$('{0:X}' -f $hwnd.ToInt64()) class=$([Rc0Win]::ClassOf($hwnd)) pid=$([Rc0Win]::PidOf($hwnd)) visible=$([Rc0Win]::IsWindowVisible($hwnd))"
    "   style        : 0x$('{0:X8}' -f $style)  WS_THICKFRAME/WS_SIZEBOX=$thick -> border-resize $(if ($thick) {'ENABLED'} else {'DISABLED'})"
    "   outer        : $($r.OuterW)x$($r.OuterH) px at ($($r.X),$($r.Y))   client: $($r.ClientW)x$($r.ClientH) px   dpi=$dpi scale=$scale"
    ("   outer        : {0:N1}x{1:N1} DIP   client: {2:N1}x{3:N1} DIP" -f ($r.OuterW/$scale), ($r.OuterH/$scale), ($r.ClientW/$scale), ($r.ClientH/$scale))
  )
  Write-Host ($lines -join [Environment]::NewLine)
  return $r
}

$procs = Get-Process -Name $ProcessName -ErrorAction Stop
$target = $null
# Primary match: the framed product window. Desktop mode reparents the window under
# the shell's Progman/WorkerW, which replaces the 'Tauri Window' class, so fall back
# to any visible window of the process that hosts the WRY_WEBVIEW child.
foreach ($p in $procs) {
  foreach ($h in [Rc0Win]::TopLevel()) {
    if ([Rc0Win]::PidOf($h) -ne $p.Id) { continue }
    if ([Rc0Win]::ClassOf($h) -ne 'Tauri Window') { continue }
    $target = $h; break
  }
  if ($target) { break }
}
if (-not $target) {
  foreach ($p in $procs) {
    foreach ($h in [Rc0Win]::TopLevel()) {
      if ([Rc0Win]::PidOf($h) -ne $p.Id) { continue }
      if (-not [Rc0Win]::IsWindowVisible($h)) { continue }
      $webviewChildren = [Rc0Win]::Children($h) | Where-Object { [Rc0Win]::ClassOf($_) -eq 'WRY_WEBVIEW' }
      if ($webviewChildren.Count -gt 0) { $target = $h; break }
    }
    if ($target) { break }
  }
}
if (-not $target) { throw "could not resolve the Tauri product window for $ProcessName" }
Write-Output "(resolved window class: $([Rc0Win]::ClassOf($target)))"

Write-Output "=== product window (true physical values) ==="
$r = Describe $target "top-level"
$r | Out-Null
Write-Output "=== children ==="
foreach ($c in [Rc0Win]::Children($target)) {
  $cr = Get-Rect $c
  Write-Output ("   child hwnd=0x{0:X} class={1} client={2}x{3} px" -f $c.ToInt64(), [Rc0Win]::ClassOf($c), $cr.ClientW, $cr.ClientH)
}

if ($ResizeTo -gt 0) {
  Write-Output "--- SetWindowPos width=$ResizeTo (SWP_NOMOVE|SWP_NOZORDER) ---"
  $ok = [Rc0Win]::SetWindowPos($target, [IntPtr]::Zero, 0, 0, $ResizeTo, $r.OuterH, 0x0002 -bor 0x0004)
  Write-Output "SetWindowPos   : $ok lastError=$([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())"
  Start-Sleep -Milliseconds 900
  Describe $target "after" | Out-Null
}
