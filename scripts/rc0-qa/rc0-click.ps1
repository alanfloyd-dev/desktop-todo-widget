# RC-0 QA helper: performs a real left-click at a fraction of the product
# window's client area using SendInput-equivalent Win32 mouse events.
#
# Synthetic mouse_event input goes through the same WM_LBUTTONDOWN/UP path a
# physical click uses, so the WebView's pointerdown/pointerup handlers and the
# Orb's drag-vs-click threshold are exercised rather than bypassed.
param(
  [string]$ProcessName = "alan-desktop",
  [double]$XFrac = 0.5,
  [double]$YFrac = 0.5,
  [int]$HoldMs = 60
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Rc0Click {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder buf, int max);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", SetLastError=true)] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public static List<IntPtr> TopLevel() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string ClassOf(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint PidOf(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }
}
"@

[void][Rc0Click]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$target = $null
foreach ($p in (Get-Process -Name $ProcessName -ErrorAction Stop)) {
  foreach ($h in [Rc0Click]::TopLevel()) {
    if ([Rc0Click]::PidOf($h) -eq $p.Id -and [Rc0Click]::ClassOf($h) -eq 'Tauri Window') { $target = $h; break }
  }
  if ($target) { break }
}
if (-not $target) { throw "Tauri product window not found" }

$cr = New-Object Rc0Click+RECT
[void][Rc0Click]::GetClientRect($target, [ref]$cr)
$pt = New-Object Rc0Click+POINT
$pt.X = [int](($cr.Right - $cr.Left) * $XFrac)
$pt.Y = [int](($cr.Bottom - $cr.Top) * $YFrac)
[void][Rc0Click]::ClientToScreen($target, [ref]$pt)

Write-Host ("clicking client {0},{1} -> screen {2},{3}" -f [int](($cr.Right-$cr.Left)*$XFrac), [int](($cr.Bottom-$cr.Top)*$YFrac), $pt.X, $pt.Y)
[void][Rc0Click]::SetCursorPos($pt.X, $pt.Y)
Start-Sleep -Milliseconds 150
[Rc0Click]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)  # LEFTDOWN
Start-Sleep -Milliseconds $HoldMs
[Rc0Click]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)  # LEFTUP
Start-Sleep -Milliseconds 1200
Write-Host "click delivered"
