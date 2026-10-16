# RC-0 QA helper: performs a real left-button drag with intermediate motion
# events, so the window manager's modal resize/sizing loop is driven exactly as a
# physical mouse drag drives it.
#
# The drag starts at the product window's named edge and ends at an absolute
# target, both measured from the live window rect.
param(
  [string]$ProcessName = "alan-desktop",
  [ValidateSet('left', 'right')]
  [string]$Edge = 'left',
  [int]$Dx = 0,
  [int]$Dy = 0,
  [int]$Steps = 24
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Rc0Drag {
  public delegate bool EWP(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EWP cb, IntPtr p);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", SetLastError=true)] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", SetLastError=true)] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
}
"@
[void][Rc0Drag]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$target = $null
foreach ($p in (Get-Process -Name $ProcessName -ErrorAction Stop)) {
  foreach ($h in [Rc0Drag]::Top()) {
    if ([Rc0Drag]::Pid($h) -eq $p.Id -and [Rc0Drag]::Cls($h) -eq 'Tauri Window') { $target = $h; break }
  }
  if ($target) { break }
}
if (-not $target) { throw "product window not found" }

$r = New-Object Rc0Drag+R
[void][Rc0Drag]::GetWindowRect($target, [ref]$r)
$startX = if ($Edge -eq 'left') { $r.Left + 3 } else { $r.Right - 4 }
$startY = $r.Top + [int](($r.Bottom - $r.Top) / 2)
$endX = $startX + $Dx
$endY = $startY + $Dy

Write-Host ("window ({0},{1})-({2},{3}) {4}x{5}" -f $r.Left, $r.Top, $r.Right, $r.Bottom, ($r.Right - $r.Left), ($r.Bottom - $r.Top))
Write-Host ("drag {0} edge from ({1},{2}) to ({3},{4}) in {5} steps" -f $Edge, $startX, $startY, $endX, $endY, $Steps)

[void][Rc0Drag]::SetCursorPos($startX, $startY)
Start-Sleep -Milliseconds 350

# Confirm the press point is on a resizing border before pressing.
$lp = [IntPtr](($startY -shl 16) -bor ($startX -band 0xFFFF))
$hit = [Rc0Drag]::SendMessageW($target, 0x0084, [IntPtr]::Zero, $lp).ToInt64()
$hitName = switch ($hit) { 10 {'HTLEFT'} 11 {'HTRIGHT'} 12 {'HTTOP'} 15 {'HTBOTTOM'} 1 {'HTCLIENT'} 2 {'HTCAPTION'} default {"code=$hit"} }
Write-Host "hit test at press point: $hitName"

[Rc0Drag]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)  # LEFTDOWN
Start-Sleep -Milliseconds 200
for ($i = 1; $i -le $Steps; $i++) {
  $x = [int]($startX + ($endX - $startX) * $i / $Steps)
  $y = [int]($startY + ($endY - $startY) * $i / $Steps)
  [void][Rc0Drag]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 200
[Rc0Drag]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)  # LEFTUP
Start-Sleep -Milliseconds 1500

$after = New-Object Rc0Drag+R
[void][Rc0Drag]::GetWindowRect($target, [ref]$after)
Write-Host ("after drag: ({0},{1})-({2},{3}) {4}x{5}" -f $after.Left, $after.Top, $after.Right, $after.Bottom, ($after.Right - $after.Left), ($after.Bottom - $after.Top))
