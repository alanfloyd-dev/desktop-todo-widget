# RC-0 / v1 Desktop usability QA helper.
#
# Finds the product HWND even when it is a child of SHELLDLL_DefView (where
# EnumWindows cannot see it), reports its native frame facts, and can drive a
# real mouse gesture against it.
param(
  [string]$ProcessName = "alan-desktop",
  [ValidateSet('report', 'move', 'resize')]
  [string]$Action = 'report',
  # Gesture parameters, in physical pixels from the widget's client origin.
  [int]$StartX = -1,
  [int]$StartY = -1,
  [int]$Dx = 0,
  [int]$Dy = 0,
  [int]$Steps = 24
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class DeskProbe {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int index);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out int value, int size);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static List<IntPtr> Descendants() { var l = new List<IntPtr>(); EnumChildWindows(GetDesktopWindow(), (h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.Left, r.Top, r.Right, r.Bottom, r.Right-r.Left, r.Bottom-r.Top); }
  public static int Dwm(IntPtr h, int attr) { int v; return DwmGetWindowAttribute(h, attr, out v, 4) == 0 ? v : -1; }
}
"@
[void][DeskProbe]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$WS_CHILD = 0x40000000
$WS_POPUP = 0x80000000
$WS_CAPTION = 0x00C00000
$WS_THICKFRAME = 0x00040000
$WS_SIZEBOX = 0x00040000
$WS_TABSTOP = 0x00010000

$proc = Get-Process -Name $ProcessName -ErrorAction Stop
$pid0 = $proc[0].Id
$candidates = @()
foreach ($h in ([DeskProbe]::Top() + [DeskProbe]::Descendants())) {
  if ([DeskProbe]::Pid($h) -eq $pid0 -and [DeskProbe]::Cls($h) -eq 'Tauri Window') { $candidates += $h }
}
if ($candidates.Count -eq 0) { throw "product window not found for pid $pid0" }
$target = $candidates[0]

$style = [int64][DeskProbe]::GetWindowLongPtrW($target, -16)
$exstyle = [int64][DeskProbe]::GetWindowLongPtrW($target, -20)
$parent = [DeskProbe]::GetParent($target)
$owner = [DeskProbe]::GetWindow($target, 4)
$isChild = ($style -band $WS_CHILD) -ne 0
$topLevel = ($parent -eq [IntPtr]::Zero) -and (-not $isChild)

Write-Host "pid=$pid0 hwnd=0x$($target.ToString('X')) class=$([DeskProbe]::Cls($target))"
Write-Host ("style=0x{0:X8} exstyle=0x{1:X8} WS_CHILD={2} WS_POPUP={3} WS_CAPTION={4} WS_THICKFRAME={5} WS_TABSTOP={6}" -f $style, $exstyle, $isChild, (($style -band $WS_POPUP) -ne 0), (($style -band $WS_CAPTION) -ne 0), (($style -band $WS_THICKFRAME) -ne 0), (($style -band $WS_TABSTOP) -ne 0))
Write-Host "parent=0x$($parent.ToString('X')) parent_class=$(if ($parent -ne [IntPtr]::Zero) { [DeskProbe]::Cls($parent) } else { 'none' }) owner=0x$($owner.ToString('X')) top_level=$topLevel"
Write-Host "rect=$([DeskProbe]::Rect($target))"
Write-Host "dwm_host_backdrop=$([DeskProbe]::Dwm($target, 20)) dwm_system_backdrop_type=$([DeskProbe]::Dwm($target, 38))"

$client = New-Object DeskProbe+R
[void][DeskProbe]::GetClientRect($target, [ref]$client)
$origin = New-Object DeskProbe+P
[void][DeskProbe]::ClientToScreen($target, [ref]$origin)
Write-Host "client=$($client.Right)x$($client.Bottom) client_origin=($($origin.X),$($origin.Y))"

if ($Action -eq 'report') { return }

if ($StartX -lt 0) { $StartX = [int]($client.Right / 2) }
if ($StartY -lt 0) { $StartY = 60 }
$startScreenX = $origin.X + $StartX
$startScreenY = $origin.Y + $StartY
$endScreenX = $startScreenX + $Dx
$endScreenY = $startScreenY + $Dy

Write-Host "gesture start=($startScreenX,$startScreenY) end=($endScreenX,$endScreenY) steps=$Steps"
[void][DeskProbe]::SetCursorPos($startScreenX, $startScreenY)
Start-Sleep -Milliseconds 300

$lp = [IntPtr](($startScreenY -shl 16) -bor ($startScreenX -band 0xFFFF))
$hit = [DeskProbe]::SendMessageW($target, 0x0084, [IntPtr]::Zero, $lp).ToInt64()
$hitName = switch ($hit) { 0 {'HTNOWHERE'} 1 {'HTCLIENT'} 2 {'HTCAPTION'} 10 {'HTLEFT'} 11 {'HTRIGHT'} 12 {'HTTOP'} 15 {'HTBOTTOM'} 13 {'HTTOPLEFT'} 14 {'HTTOPRIGHT'} 16 {'HTBOTTOMLEFT'} 17 {'HTBOTTOMRIGHT'} default {"code=$hit"} }
Write-Host "hit_test=$hitName"

[DeskProbe]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 200
for ($i = 1; $i -le $Steps; $i++) {
  $x = [int]($startScreenX + ($endScreenX - $startScreenX) * $i / $Steps)
  $y = [int]($startScreenY + ($endScreenY - $startScreenY) * $i / $Steps)
  [void][DeskProbe]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 200
[DeskProbe]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 1200

Write-Host "rect_after=$([DeskProbe]::Rect($target))"
