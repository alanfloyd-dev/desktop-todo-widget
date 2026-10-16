# v1 Desktop usability QA: does user32's non-client move loop move this HWND?
#
# This is the exact mechanism Tauri's `start_dragging` uses: tao posts
# `WM_NCLBUTTONDOWN` with `HTCAPTION` and lets the default window procedure run
# the modal move loop from there. Posting it directly isolates the platform
# question from the DOM/bridge layers, which the physical-drag control already
# proved healthy.
param(
  [string]$ProcessName = "alan-desktop",
  [int]$Dx = -240,
  [int]$Dy = 160,
  [int]$Steps = 24
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Nc {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out P p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), (h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.Left, r.Top, r.Right, r.Bottom, r.Right-r.Left, r.Bottom-r.Top); }
}
"@
[void][Nc]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$appIds = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = $null
foreach ($h in [Nc]::All()) {
  if ($appIds -contains [Nc]::Pid($h) -and [Nc]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if (-not $product) { throw "product window not found" }

Write-Host "product=0x$($product.ToString('X')) rect_before=$([Nc]::Rect($product))"
$start = New-Object Nc+P
[void][Nc]::GetCursorPos([ref]$start)
Write-Host "cursor=($($start.X),$($start.Y))"

# WM_NCLBUTTONDOWN (0x00A1) with HTCAPTION (2); lparam carries screen coords.
$lp = [IntPtr](($start.Y -shl 16) -bor ($start.X -band 0xFFFF))
[void][Nc]::PostMessageW($product, 0x00A1, [IntPtr]2, $lp)
Start-Sleep -Milliseconds 400

for ($i = 1; $i -le $Steps; $i++) {
  [void][Nc]::SetCursorPos([int]($start.X + $Dx * $i / $Steps), [int]($start.Y + $Dy * $i / $Steps))
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 400
[void][Nc]::PostMessageW($product, 0x0202, [IntPtr]0, $lp)   # WM_LBUTTONUP to end any loop
Start-Sleep -Milliseconds 1200

Write-Host "rect_after=$([Nc]::Rect($product))"
