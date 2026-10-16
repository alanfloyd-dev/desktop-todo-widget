# Reproduction: mode switch taken from the Orb's NATIVE right-click menu.
#
# Floating+collapsed (the Orb) is the only state where the widget's right-click
# goes to the native Tauri menu (`show_product_context_menu`, same menu as the
# tray). The frontend discards that command's result and no state event exists,
# so this script checks whether the backend/native window changes while the DOM
# keeps rendering the Orb.
param(
  [string]$ProcessName = "alan-desktop",
  [switch]$DryRun
)

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ev = Join-Path $root '.rc0-evidence\restart-diag'

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class MenuRepro {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1}) {2}x{3}", r.L, r.T, r.Rr - r.L, r.B - r.T); }
  public static List<IntPtr> Menus() { var l = new List<IntPtr>(); foreach (IntPtr h in All()) { if (Cls(h) == "#32768") l.Add(h); } return l; }
  public static void Key(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(60); keybd_event(vk, 0, 2, UIntPtr.Zero); System.Threading.Thread.Sleep(120); }
}
"@
[void][MenuRepro]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = [IntPtr]::Zero
foreach ($h in [MenuRepro]::All()) {
  if ($ids -contains [MenuRepro]::Pid($h) -and [MenuRepro]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if ($product -eq [IntPtr]::Zero) { throw 'product window not found' }

$cr = New-Object MenuRepro+R
[void][MenuRepro]::GetClientRect($product, [ref]$cr)
$wr = New-Object MenuRepro+R
[void][MenuRepro]::GetWindowRect($product, [ref]$wr)
Write-Host "before: product=0x$($product.ToString('X')) rect=$([MenuRepro]::Rect($product)) client=$($cr.Rr)x$($cr.B)"

# Synthetic right-click on the Orb: the DOM contextmenu handler calls
# `show_product_context_menu`, which pops the native menu at the cursor.
[void][MenuRepro]::SetCursorPos([int](($wr.L + $wr.Rr) / 2), [int](($wr.T + $wr.B) / 2))
Start-Sleep -Milliseconds 300
[void][MenuRepro]::SetForegroundWindow($product)
Start-Sleep -Milliseconds 300
$cx = [int]($cr.Rr / 2); $cy = [int]($cr.B / 2)
$lp = [IntPtr](([long]($cy -shl 16)) -bor ([long]($cx -band 0xFFFF)))
[void][MenuRepro]::PostMessageW($product, 0x0204, [IntPtr]2, $lp)
Start-Sleep -Milliseconds 300
[void][MenuRepro]::PostMessageW($product, 0x0205, [IntPtr]0, $lp)
Start-Sleep -Seconds 1

$menus = [MenuRepro]::Menus()
Write-Host "native popup menus open: $($menus.Count)"
foreach ($m in $menus) { Write-Host "  menu 0x$($m.ToString('X')) rect=$([MenuRepro]::Rect($m))" }
if ($DryRun) { Write-Host 'dry run: leaving the menu open is not wanted'; [MenuRepro]::Key(0x1B); return }
if ($menus.Count -eq 0) { Write-Host 'no native menu appeared; aborting'; return }

# Menu order: [Window mode ▸ (Sidebar, Floating, Desktop)], [Side ▸], ---, ...
# First item is highlighted on open; Right opens the submenu, Enter selects the
# submenu's first item (Sidebar).
[MenuRepro]::Key(0x27)   # VK_RIGHT
[MenuRepro]::Key(0x0D)   # VK_RETURN
Start-Sleep -Seconds 3

Write-Host "after:  product=0x$($product.ToString('X')) rect=$([MenuRepro]::Rect($product))"
Write-Host "menus still open: $(([MenuRepro]::Menus()).Count)"
& (Join-Path $root 'scripts\rc0-qa\capture-product-window.ps1') -Out (Join-Path $ev 'shot-after-menu-switch.png') | Out-Null
Write-Host '=== app log (mode transitions) ==='
$err = Get-ChildItem $ev -Filter 'run-f1-floating-collapsed.err.txt' | Select-Object -First 1
if ($err) { Select-String -Path $err.FullName -Pattern 'mode transition|apply collapsed|apply expanded|sidebar|frontend_input_received' | ForEach-Object { $_.Line } | Select-Object -Last 8 }
