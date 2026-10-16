# v1 Desktop usability gate, isolating step.
#
# Holds the real left button down over the product's Desktop child and posts the
# non-client caption press (`WM_NCLBUTTONDOWN` + `HTCAPTION`) the way tao does.
#
#   moves   -> the non-client loop CAN move this child, so a failure of the
#              normal drag means the DOM/`start_dragging` half never ran.
#   no move -> the child (as parented under SHELLDLL_DefView) cannot be moved by
#              that loop, so a product-specific move path is required.
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
public static class Hold {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(P p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GTI gti);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct GTI { public int cbSize; public uint flags; public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret; public R rcCaret; }
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), (h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.Left, r.Top, r.Right, r.Bottom, r.Right-r.Left, r.Bottom-r.Top); }
}
"@
[void][Hold]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$appIds = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = $null
foreach ($h in [Hold]::All()) {
  if ($appIds -contains [Hold]::Pid($h) -and [Hold]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if (-not $product) { throw "product window not found" }

$client = New-Object Hold+R
[void][Hold]::GetClientRect($product, [ref]$client)
$origin = New-Object Hold+P
[void][Hold]::ClientToScreen($product, [ref]$origin)
$sx = $origin.X + $ClientX
$sy = $origin.Y + $ClientY

Write-Host "product=0x$($product.ToString('X')) rect_before=$([Hold]::Rect($product))"
[void][Hold]::SetCursorPos($sx, $sy)
Start-Sleep -Milliseconds 300
$pt = New-Object Hold+P; $pt.X = $sx; $pt.Y = $sy
$under = [Hold]::WindowFromPoint($pt)
Write-Host "under_pointer=0x$($under.ToString('X')) class=$([Hold]::Cls($under)) ours=$($appIds -contains [Hold]::Pid($under))"
if (-not ($appIds -contains [Hold]::Pid($under))) { Write-Host 'SAFETY ABORT'; return }

$lp = [IntPtr](($sy -shl 16) -bor ($sx -band 0xFFFF))
[void][Hold]::SendMessageW($product, 0x0084, [IntPtr]::Zero, $lp)

[void][Hold]::PostMessageW($product, 0x0201, [IntPtr]1, [IntPtr](($ClientY -shl 16) -bor ($ClientX -band 0xFFFF)))
Start-Sleep -Milliseconds 150
[void][Hold]::PostMessageW($product, 0x00A1, [IntPtr]2, $lp)   # WM_NCLBUTTONDOWN + HTCAPTION
Start-Sleep -Milliseconds 300

$ownerPid = 0
$threadId = [Hold]::GetWindowThreadProcessId($product, [ref]$ownerPid)
$saw = $false
for ($i = 1; $i -le $Steps; $i++) {
  [void][Hold]::SetCursorPos([int]($sx + $Dx * $i / $Steps), [int]($sy + $Dy * $i / $Steps))
  $gti = New-Object Hold+GTI
  $gti.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf($gti)
  if ([Hold]::GetGUIThreadInfo($threadId, [ref]$gti)) { if (($gti.flags -band 2) -ne 0) { $saw = $true } }
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 300
[void][Hold]::PostMessageW($product, 0x0202, [IntPtr]0, [IntPtr](($ClientY -shl 16) -bor ($ClientX -band 0xFFFF)))

Write-Host "system_move_loop_started=$saw"
Write-Host "rect_after=$([Hold]::Rect($product))"
