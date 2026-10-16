# Send keystrokes to an open native (Win32) popup menu.
#
# `keybd_event` only reaches the foreground window's queue, which is why it did
# nothing here. A TrackPopupMenu loop consumes keyboard messages from its owning
# thread's queue, so posting them to a window on that thread (the product HWND)
# drives the menu deterministically.
param(
  [string]$ProcessName = "alan-desktop",
  [Parameter(Mandatory = $true)][string]$Keys
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class MenuKeys {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3})", r.L, r.T, r.Rr, r.B); }
  public static List<IntPtr> Menus() { var seen = new List<IntPtr>(); foreach (IntPtr h in All()) { if (Cls(h) == "#32768" && IsWindowVisible(h) && !seen.Contains(h)) seen.Add(h); } return seen; }
}
"@
[void][MenuKeys]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = [IntPtr]::Zero
foreach ($h in [MenuKeys]::All()) {
  if ($ids -contains [MenuKeys]::Pid($h) -and [MenuKeys]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if ($product -eq [IntPtr]::Zero) { throw 'product window not found' }

$menus = [MenuKeys]::Menus()
Write-Host "menus open before: $($menus.Count)"
foreach ($m in $menus) { Write-Host "  0x$($m.ToString('X')) rect=$([MenuKeys]::Rect($m))" }

foreach ($key in ($Keys -split ',')) {
  $k = $key.Trim().ToUpperInvariant()
  $vk = switch ($k) {
    'DOWN'  { 0x28 }
    'UP'    { 0x26 }
    'RIGHT' { 0x27 }
    'LEFT'  { 0x25 }
    'ENTER' { 0x0D }
    'ESC'   { 0x1B }
    default { throw "unsupported key $k" }
  }
  $target = $product
  if ($menus.Count -gt 0) { $target = $menus[0] }
  [void][MenuKeys]::PostMessageW($target, 0x0100, [IntPtr]$vk, [IntPtr]0)   # WM_KEYDOWN
  Start-Sleep -Milliseconds 90
  [void][MenuKeys]::PostMessageW($target, 0x0101, [IntPtr]$vk, [IntPtr]0)   # WM_KEYUP
  Start-Sleep -Milliseconds 250
  $menus = [MenuKeys]::Menus()
  Write-Host "  sent $k -> menus open: $($menus.Count)"
}
foreach ($m in $menus) { Write-Host "  now 0x$($m.ToString('X')) rect=$([MenuKeys]::Rect($m))" }
