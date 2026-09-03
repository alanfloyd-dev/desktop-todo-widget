param(
  [ValidateSet('win-d','alt-tab','activate','topmost','not-topmost','left','right','post-left','tab','shift-tab','enter','escape')][string]$Action,
  [int]$X = 0,
  [int]$Y = 0,
  [string]$HwndHex = ""
)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Phase5NativeInput {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hwnd, int command);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
}
"@

if ($Action -in @('topmost','not-topmost')) {
  $raw = [IntPtr]::new([Convert]::ToInt64($HwndHex.Replace('0x', ''), 16))
  $insertAfter = if ($Action -eq 'topmost') { [IntPtr]::new(-1) } else { [IntPtr]::new(-2) }
  $changed = [Phase5NativeInput]::SetWindowPos($raw, $insertAfter, 0, 0, 0, 0, 0x0013)
  Write-Output "Temporary z-order $Action applied: $changed"
  Start-Sleep -Milliseconds 400
  return
}

if ($Action -eq 'post-left') {
  $raw = [IntPtr]::new([Convert]::ToInt64($HwndHex.Replace('0x', ''), 16))
  $lParam = [IntPtr]::new((($Y -band 0xFFFF) -shl 16) -bor ($X -band 0xFFFF))
  $down = [Phase5NativeInput]::PostMessage($raw, 0x0201, [UIntPtr]::new(1), $lParam)
  $up = [Phase5NativeInput]::PostMessage($raw, 0x0202, [UIntPtr]::Zero, $lParam)
  Write-Output "Posted client click: down=$down up=$up at $X,$Y"
  Start-Sleep -Milliseconds 500
  return
}

if ($Action -eq 'win-d') {
  [Phase5NativeInput]::keybd_event(0x5B, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event(0x44, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event(0x44, 0, 2, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event(0x5B, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 500
  return
}

if ($Action -eq 'alt-tab') {
  [Phase5NativeInput]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event(0x09, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event(0x09, 0, 2, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 500
  return
}

if ($Action -eq 'activate') {
  $raw = [IntPtr]::new([Convert]::ToInt64($HwndHex.Replace('0x', ''), 16))
  [void][Phase5NativeInput]::ShowWindowAsync($raw, 9)
  [void][Phase5NativeInput]::BringWindowToTop($raw)
  [void][Phase5NativeInput]::SetForegroundWindow($raw)
  Start-Sleep -Milliseconds 500
  return
}

if ($Action -in @('tab','shift-tab','enter','escape')) {
  $key = if ($Action -in @('tab','shift-tab')) { 0x09 } elseif ($Action -eq 'enter') { 0x0D } else { 0x1B }
  if ($Action -eq 'shift-tab') { [Phase5NativeInput]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero) }
  [Phase5NativeInput]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
  if ($Action -eq 'shift-tab') { [Phase5NativeInput]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero) }
  Start-Sleep -Milliseconds 200
  return
}

[void][Phase5NativeInput]::SetCursorPos($X, $Y)
$cursor = New-Object Phase5NativeInput+POINT
[void][Phase5NativeInput]::GetCursorPos([ref]$cursor)
Write-Output "Cursor physical position: $($cursor.X),$($cursor.Y)"
Start-Sleep -Milliseconds 120
if ($Action -eq 'right') {
  [Phase5NativeInput]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)
} else {
  [Phase5NativeInput]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
  [Phase5NativeInput]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}
Start-Sleep -Milliseconds 350
