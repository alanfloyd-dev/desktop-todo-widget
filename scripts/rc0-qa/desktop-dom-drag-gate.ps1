# v1 Desktop usability gate: the real product drag path into the Desktop child.
#
# The physical button is pressed over a small topmost window this script owns
# (so no click can land in another application) and held. Then a client-area
# `WM_LBUTTONDOWN` is posted to the product HWND at the header, which drives the
# genuine product chain:
#
#   wry forwarder -> WebView2 SendMouseInput -> DOM mousedown
#     -> data-tauri-drag-region -> plugin:window|start_dragging
#     -> tao PostMessageW(WM_NCLBUTTONDOWN, HTCAPTION)
#     -> user32 move loop -> the Desktop child follows the cursor
#
# Holding a real button is what lets the modal move loop run, so this works even
# while another window covers the desktop.
param(
  [string]$ProcessName = "alan-desktop",
  [int]$ClientX = 315,
  [int]$ClientY = 150,
  [int]$Dx = -260,
  [int]$Dy = 180,
  [int]$Steps = 24
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class DomGate {
  public delegate IntPtr WndProcD(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct WNDCLASS { public uint style; public WndProcD lpfnWndProc; public int cbClsExtra; public int cbWndExtra; public IntPtr hInstance; public IntPtr hIcon; public IntPtr hCursor; public IntPtr hbrBackground; public string lpszMenuName; public string lpszClassName; }
  [StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  [StructLayout(LayoutKind.Sequential)] public struct GTI { public int cbSize; public uint flags; public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret; public R rcCaret; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern ushort RegisterClassW(ref WNDCLASS c);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr CreateWindowExW(uint ex, string cls, string name, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
  [DllImport("user32.dll")] public static extern IntPtr DefWindowProcW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr GetModuleHandleW(string n);
  [DllImport("user32.dll")] public static extern bool DestroyWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GTI gti);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), (h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.Left, r.Top, r.Right, r.Bottom, r.Right-r.Left, r.Bottom-r.Top); }
  public static IntPtr MakePressPad() {
    WNDCLASS wc = new WNDCLASS();
    wc.lpfnWndProc = (ph, pm, pw, pl) => DefWindowProcW(ph, pm, pw, pl);
    wc.hInstance = GetModuleHandleW(null);
    wc.lpszClassName = "V1DomPressPad";
    RegisterClassW(ref wc);
    IntPtr pad = CreateWindowExW(0x00000008 | 0x00000080, "V1DomPressPad", "v1dompad",
      0x80000000u | 0x10000000u, 40, 60, 180, 90, IntPtr.Zero, IntPtr.Zero, GetModuleHandleW(null), IntPtr.Zero);
    SetForegroundWindow(pad);
    return pad;
  }
}
"@
[void][DomGate]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$appIds = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = $null
foreach ($h in [DomGate]::All()) {
  if ($appIds -contains [DomGate]::Pid($h) -and [DomGate]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if (-not $product) { throw "product window not found" }

$client = New-Object DomGate+R
[void][DomGate]::GetClientRect($product, [ref]$client)
$origin = New-Object DomGate+P
[void][DomGate]::ClientToScreen($product, [ref]$origin)
$sx = $origin.X + $ClientX
$sy = $origin.Y + $ClientY

Write-Host "product=0x$($product.ToString('X')) rect_before=$([DomGate]::Rect($product)) header_screen=($sx,$sy) client=$($client.Right)x$($client.Bottom)"

$pad = [DomGate]::MakePressPad()
Start-Sleep -Milliseconds 500
[void][DomGate]::SetCursorPos(80, 105)
Start-Sleep -Milliseconds 300

[void][DomGate]::SetCursorPos($sx, $sy)
Start-Sleep -Milliseconds 150
[DomGate]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # real press, captured by the pad
Start-Sleep -Milliseconds 200

# Real product chain from the input bridge onward, at the header point.
$clientLp = [IntPtr](($ClientY -shl 16) -bor ($ClientX -band 0xFFFF))
[void][DomGate]::PostMessageW($product, 0x0201, [IntPtr]1, $clientLp)   # WM_LBUTTONDOWN, MK_LBUTTON
Start-Sleep -Milliseconds 700

$ownerPid = 0
$threadId = [DomGate]::GetWindowThreadProcessId($product, [ref]$ownerPid)
$saw = $false
for ($i = 1; $i -le $Steps; $i++) {
  [void][DomGate]::SetCursorPos([int]($sx + $Dx * $i / $Steps), [int]($sy + $Dy * $i / $Steps))
  $gti = New-Object DomGate+GTI
  $gti.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf($gti)
  if ([DomGate]::GetGUIThreadInfo($threadId, [ref]$gti)) { if (($gti.flags -band 2) -ne 0) { $saw = $true } }
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 200
[void][DomGate]::PostMessageW($product, 0x0202, [IntPtr]0, $clientLp)   # WM_LBUTTONUP
[DomGate]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 1000
[void][DomGate]::DestroyWindow($pad)

Write-Host "system_move_loop_started=$saw"
Write-Host "rect_after=$([DomGate]::Rect($product))"
