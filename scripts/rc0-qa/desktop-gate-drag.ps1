# v1 Desktop usability QA: drag gate.
#
# Deterministic drag-trigger test that does not require the widget to be the
# topmost window on screen. The physical pointer path above the input bridge is
# already proven (real right-click works in Desktop mode); what is under test
# here is everything from the bridge onward:
#
#   WM_LBUTTONDOWN (client px) -> wry forwarder -> WebView2 SendMouseInput
#     -> DOM mousedown -> data-tauri-drag-region -> plugin:window|start_dragging
#     -> tao PostMessage(WM_NCLBUTTONDOWN, HTCAPTION)
#     -> user32 non-client move loop -> does the WS_CHILD actually move?
param(
  [string]$ProcessName = "alan-desktop",
  [int]$ClientX = 315,
  [int]$ClientY = 60,
  [int]$Dx = -300,
  [int]$Dy = 200,
  [int]$Steps = 24
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Gate {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
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
[void][Gate]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$status = @(Get-Process msedgewebview2 -ErrorAction SilentlyContinue | Measure-Object).Count
$target = $null
foreach ($h in [Gate]::All()) {
  if ($ProcessName -and (Get-Process -Id ([Gate]::Pid($h)) -ErrorAction SilentlyContinue).ProcessName -eq $ProcessName -and [Gate]::Cls($h) -eq 'Tauri Window') { $target = $h; break }
}
if (-not $target) { throw "product window not found" }

$client = New-Object Gate+R
[void][Gate]::GetClientRect($target, [ref]$client)
$origin = New-Object Gate+P
[void][Gate]::ClientToScreen($target, [ref]$origin)
$screenX = $origin.X + $ClientX
$screenY = $origin.Y + $ClientY

Write-Host "product=0x$($target.ToString('X')) rect_before=$([Gate]::Rect($target)) client=$($client.Right)x$($client.Bottom) origin=($($origin.X),$($origin.Y))"

# Park the real cursor where the drag would be so the user32 move loop, if it
# starts, tracks from the widget instead of from wherever the cursor was.
[void][Gate]::SetCursorPos($screenX, $screenY)
Start-Sleep -Milliseconds 200

$hit = [Gate]::SendMessageW($target, 0x0084, [IntPtr]::Zero, [IntPtr](($screenY -shl 16) -bor ($screenX -band 0xFFFF))).ToInt64()
$hitName = switch ($hit) { 1 {'HTCLIENT'} 2 {'HTCAPTION'} 10 {'HTLEFT'} 11 {'HTRIGHT'} 12 {'HTTOP'} 15 {'HTBOTTOM'} 17 {'HTBOTTOMRIGHT'} default {"code=$hit"} }
Write-Host "hit_test=$hitName"

$lp = [IntPtr](($ClientY -shl 16) -bor ($ClientX -band 0xFFFF))
[void][Gate]::PostMessageW($target, 0x0201, [IntPtr]1, $lp)   # WM_LBUTTONDOWN, MK_LBUTTON
Start-Sleep -Milliseconds 600

for ($i = 1; $i -le $Steps; $i++) {
  $x = [int]($screenX + $Dx * $i / $Steps)
  $y = [int]($screenY + $Dy * $i / $Steps)
  [void][Gate]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 300
[void][Gate]::PostMessageW($target, 0x0202, [IntPtr]0, $lp)   # WM_LBUTTONUP
Start-Sleep -Milliseconds 1200

Write-Host "rect_after=$([Gate]::Rect($target))"
Write-Host "webview2_processes=$status"
