# v1 Desktop usability QA: real physical mouse drag/resize against the product HWND.
#
# Safety gate: the script refuses to press any mouse button unless the window
# under the pointer is the product window (or one of its children/descendants).
# That keeps a stray press from landing in whatever the user has in the
# foreground.
param(
  [string]$ProcessName = "alan-desktop",
  [int]$ClientX = 315,
  [int]$ClientY = 60,
  [int]$Dx = -300,
  [int]$Dy = 200,
  [int]$Steps = 24,
  [switch]$DryRun
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Phys {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint f);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(P p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GTI gti);
  [StructLayout(LayoutKind.Sequential)] public struct GTI { public int cbSize; public uint flags; public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret; public R rcCaret; }
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), (h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.Left, r.Top, r.Right, r.Bottom, r.Right-r.Left, r.Bottom-r.Top); }
}
"@
[void][Phys]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$appIds = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = $null
foreach ($h in [Phys]::All()) {
  if ($appIds -contains [Phys]::Pid($h) -and [Phys]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if (-not $product) { throw "product window not found" }

$client = New-Object Phys+R
[void][Phys]::GetClientRect($product, [ref]$client)
$origin = New-Object Phys+P
[void][Phys]::ClientToScreen($product, [ref]$origin)
$startX = $origin.X + $ClientX
$startY = $origin.Y + $ClientY

Write-Host "product=0x$($product.ToString('X')) rect_before=$([Phys]::Rect($product)) client=$($client.Right)x$($client.Bottom)"

[void][Phys]::SetCursorPos($startX, $startY)
Start-Sleep -Milliseconds 300

$pt = New-Object Phys+P
$pt.X = $startX; $pt.Y = $startY
$under = [Phys]::WindowFromPoint($pt)
$underIsOurs = $appIds -contains [Phys]::Pid($under)
Write-Host "under_pointer=0x$($under.ToString('X')) class=$([Phys]::Cls($under)) pid=$([Phys]::Pid($under)) ours=$underIsOurs"
if (-not $underIsOurs) {
  Write-Host 'SAFETY ABORT: the pointer is not over the product window; no button was pressed.'
  return
}

$hit = [Phys]::SendMessageW($product, 0x0084, [IntPtr]::Zero, [IntPtr](($startY -shl 16) -bor ($startX -band 0xFFFF))).ToInt64()
$hitName = switch ($hit) { 1 {'HTCLIENT'} 2 {'HTCAPTION'} 10 {'HTLEFT'} 11 {'HTRIGHT'} 12 {'HTTOP'} 15 {'HTBOTTOM'} 17 {'HTBOTTOMRIGHT'} default {"code=$hit"} }
Write-Host "hit_test=$hitName drag_from=($startX,$startY) delta=($Dx,$Dy)"
if ($DryRun) { return }

[Phys]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 250
$ownerPid = 0
$threadId = [Phys]::GetWindowThreadProcessId($product, [ref]$ownerPid)
$sawMoveSize = $false
for ($i = 1; $i -le $Steps; $i++) {
  $x = [int]($startX + $Dx * $i / $Steps)
  $y = [int]($startY + $Dy * $i / $Steps)
  [void][Phys]::SetCursorPos($x, $y)
  $gti = New-Object Phys+GTI
  $gti.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf($gti)
  if ([Phys]::GetGUIThreadInfo($threadId, [ref]$gti)) {
    if (($gti.flags -band 0x00000002) -ne 0) { $sawMoveSize = $true }
  }
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 250
[Phys]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 1200

Write-Host "system_move_loop_started=$sawMoveSize"
Write-Host "rect_after=$([Phys]::Rect($product))"
