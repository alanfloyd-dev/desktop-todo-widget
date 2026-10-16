# Complete the reproduction: choose "Window mode -> Sidebar" from the Orb's
# native right-click menu, then observe backend/native state versus the DOM.
#
# Safety: a mouse button is only pressed while the window under the pointer is a
# `#32768` popup owned by the product process.
param(
  [string]$ProcessName = "alan-desktop"
)

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ev = Join-Path $root '.rc0-evidence\restart-diag'

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Mnu {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(P p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3})", r.L, r.T, r.Rr, r.B); }
  public static IntPtr[] Menus() { var seen = new List<IntPtr>(); foreach (IntPtr h in All()) { if (Cls(h) == "#32768" && IsWindowVisible(h) && !seen.Contains(h)) seen.Add(h); } var a = seen.ToArray(); Array.Sort(a, delegate(IntPtr x, IntPtr y) { return x.ToInt64().CompareTo(y.ToInt64()); }); return a; }
}
"@
[void][Mnu]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = [IntPtr]::Zero
foreach ($h in [Mnu]::All()) {
  if ($ids -contains [Mnu]::Pid($h) -and [Mnu]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if ($product -eq [IntPtr]::Zero) { throw 'product window not found' }
$wr = New-Object Mnu+R; [void][Mnu]::GetWindowRect($product, [ref]$wr)
$cr = New-Object Mnu+R; [void][Mnu]::GetClientRect($product, [ref]$cr)
Write-Host "before: rect=$([Mnu]::Rect($product)) client=$($cr.Rr)x$($cr.B)"

# Open the Orb's native menu.
[void][Mnu]::SetCursorPos([int](($wr.L + $wr.Rr) / 2), [int](($wr.T + $wr.B) / 2))
Start-Sleep -Milliseconds 300
$cx = [int]($cr.Rr / 2); $cy = [int]($cr.B / 2)
$lp = [IntPtr](([long]($cy -shl 16)) -bor ([long]($cx -band 0xFFFF)))
[void][Mnu]::PostMessageW($product, 0x0204, [IntPtr]2, $lp)
Start-Sleep -Milliseconds 250
[void][Mnu]::PostMessageW($product, 0x0205, [IntPtr]0, $lp)
Start-Sleep -Seconds 1

$main = [Mnu]::Menus()
if ($main.Count -eq 0) { Write-Host 'no menu appeared'; return }
$mainHwnd = $main[0]
$mr = New-Object Mnu+R; [void][Mnu]::GetWindowRect($mainHwnd, [ref]$mr)
Write-Host "menu=0x$($mainHwnd.ToString('X')) rect=$([Mnu]::Rect($mainHwnd))"

# Hover the first row ("Window mode") until its submenu appears to either side.
$rowH = [int](($mr.B - $mr.T) / 7)
$found = $false
foreach ($offset in 0..5) {
  $y = $mr.T + 6 + [int]($rowH / 2) + ($offset * 3)
  $x = $mr.L + 60
  [void][Mnu]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 1100
  $sub = [IntPtr]::Zero
  foreach ($candidate in [Mnu]::Menus()) {
    if ($candidate -ne $mainHwnd) { $sub = $candidate; break }
  }
  if ($sub -ne [IntPtr]::Zero) {
    $sr = New-Object Mnu+R; [void][Mnu]::GetWindowRect($sub, [ref]$sr)
    Write-Host "submenu opened at hover y=$y : 0x$($sub.ToString('X')) rect=$([Mnu]::Rect($sub))"
    $sx = $sr.L + 60
    $sy = $sr.T + 6 + [int](($sr.B - $sr.T) / 3 / 2)
    $pt = New-Object Mnu+P; $pt.X = $sx; $pt.Y = $sy
    $under = [Mnu]::WindowFromPoint($pt)
    Write-Host "under submenu point=0x$($under.ToString('X')) class=$([Mnu]::Cls($under)) ours=$($ids -contains [Mnu]::Pid($under))"
    if (-not ($ids -contains [Mnu]::Pid($under))) { Write-Host 'SAFETY ABORT'; return }
    [void][Mnu]::SetCursorPos($sx, $sy)
    Start-Sleep -Milliseconds 400
    [Mnu]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
    [Mnu]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    $found = $true
    break
  }
}
if (-not $found) { Write-Host 'submenu never opened; aborting (menu left open, pressing Escape)'; [void][Mnu]::SetCursorPos($mr.L + 60, $mr.T + 6); [Mnu]::mouse_event(0x0002,0,0,0,[UIntPtr]::Zero); [Mnu]::mouse_event(0x0004,0,0,0,[UIntPtr]::Zero); return }

Start-Sleep -Seconds 3
Write-Host "after:  rect=$([Mnu]::Rect($product)) menus_open=$(([Mnu]::Menus()).Count)"
& (Join-Path $root 'scripts\rc0-qa\capture-product-window.ps1') -Out (Join-Path $ev 'shot-after-sidebar-switch.png') | Out-Null
