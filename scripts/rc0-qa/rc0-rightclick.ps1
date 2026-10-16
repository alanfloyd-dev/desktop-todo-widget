# RC-0 QA helper: right-clicks at a fraction of the product window's client area
# using real Win32 mouse events, to exercise the window's context-menu path.
param(
  [string]$ProcessName = "alan-desktop",
  [double]$XFrac = 0.5,
  [double]$YFrac = 0.5
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Rc0RClick {
  public delegate bool EWP(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EWP cb, IntPtr p);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", SetLastError=true)] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
}
"@
[void][Rc0RClick]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$target = $null
foreach ($p in (Get-Process -Name $ProcessName -ErrorAction Stop)) {
  foreach ($h in [Rc0RClick]::Top()) {
    if ([Rc0RClick]::Pid($h) -eq $p.Id -and [Rc0RClick]::Cls($h) -eq 'Tauri Window') { $target = $h; break }
  }
  if ($target) { break }
}
if (-not $target) { throw "product window not found" }

$cr = New-Object Rc0RClick+R
[void][Rc0RClick]::GetClientRect($target, [ref]$cr)
$pt = New-Object Rc0RClick+P
$pt.X = [int](($cr.Right - $cr.Left) * $XFrac)
$pt.Y = [int](($cr.Bottom - $cr.Top) * $YFrac)
[void][Rc0RClick]::ClientToScreen($target, [ref]$pt)

Write-Host "right-clicking screen $($pt.X),$($pt.Y)"
[void][Rc0RClick]::SetCursorPos($pt.X, $pt.Y)
Start-Sleep -Milliseconds 200
[Rc0RClick]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)  # RIGHTDOWN
Start-Sleep -Milliseconds 70
[Rc0RClick]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)  # RIGHTUP
Start-Sleep -Milliseconds 1500
Write-Host "delivered"

# Enumerate popup menus that appeared.
$menus = @()
foreach ($h in [Rc0RClick]::Top()) {
  if ([Rc0RClick]::Cls($h) -eq '#32768') { $menus += $h }
}
Write-Host "visible popup menus after right-click: $($menus.Count)"
