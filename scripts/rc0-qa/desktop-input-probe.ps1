# v1 Desktop usability QA: where does the pointer actually land?
#
# Everything here runs in one DPI-aware process so window rects, SetCursorPos and
# WindowFromPoint all speak the same (physical) coordinate space.
param(
  [string]$ProcessName = "alan-desktop",
  [int]$ClientX = 315,
  [int]$ClientY = 60
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class InProbe {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(P p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out P p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();
  [DllImport("user32.dll")] public static extern int GetAwarenessFromDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static List<IntPtr> Descendants() { var l = new List<IntPtr>(); EnumChildWindows(GetDesktopWindow(), (h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string R2S(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.Left, r.Top, r.Right, r.Bottom, r.Right-r.Left, r.Bottom-r.Top); }
}
"@
[void][InProbe]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
$awareness = [InProbe]::GetAwarenessFromDpiAwarenessContext([InProbe]::GetThreadDpiAwarenessContext())
Write-Host "process_dpi_awareness=$awareness (2=system,3=per-monitor)"

$appIds = (Get-Process -Name $ProcessName -ErrorAction Stop).Id
$product = $null
foreach ($h in ([InProbe]::Top() + [InProbe]::Descendants())) {
  if ($appIds -contains [InProbe]::Pid($h) -and [InProbe]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if (-not $product) { throw "product window not found" }
Write-Host "product=0x$($product.ToString('X')) rect=$([InProbe]::R2S($product))"

$client = New-Object InProbe+R
[void][InProbe]::GetClientRect($product, [ref]$client)
$origin = New-Object InProbe+P
[void][InProbe]::ClientToScreen($product, [ref]$origin)
$target = New-Object InProbe+P
$target.X = $origin.X + $ClientX
$target.Y = $origin.Y + $ClientY
Write-Host "client_point=($ClientX,$ClientY) -> screen=($($target.X),$($target.Y))"

[void][InProbe]::SetCursorPos($target.X, $target.Y)
Start-Sleep -Milliseconds 250
$now = New-Object InProbe+P
[void][InProbe]::GetCursorPos([ref]$now)
Write-Host "cursor_after_set=($($now.X),$($now.Y))"

$under = [InProbe]::WindowFromPoint($now)
Write-Host "window_under_pointer=0x$($under.ToString('X')) class=$([InProbe]::Cls($under)) pid=$([InProbe]::Pid($under)) visible=$([InProbe]::IsWindowVisible($under)) rect=$([InProbe]::R2S($under))"
$productPid = [InProbe]::Pid($product)
Write-Host "product_pid=$productPid window_under_is_product=$([InProbe]::Pid($under) -eq $productPid)"

Write-Host "`n--- windows owned by the app process tree ---"
$treePids = @($productPid)
$all = Get-CimInstance Win32_Process | Where-Object { $_.ParentProcessId -eq $productPid }
foreach ($child in $all) { $treePids += [int]$child.ProcessId }
foreach ($h in ([InProbe]::Top() + [InProbe]::Descendants())) {
  $hpid = [InProbe]::Pid($h)
  if ($treePids -contains $hpid) {
    $cls = [InProbe]::Cls($h)
    Write-Host ("  0x{0:X} pid={1} class={2} visible={3} rect={4}" -f $h.ToInt64(), $hpid, $cls, [InProbe]::IsWindowVisible($h), [InProbe]::R2S($h))
  }
}
