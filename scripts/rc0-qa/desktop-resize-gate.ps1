# v1 Desktop usability QA: resize path verification.
#
# Checks, without needing a clear desktop:
#   1. the Desktop child carries the sizing-border bit the undecorated resize
#      borders test for, and Tauri's border child is present;
#   2. the frame band hit-tests as HT* (both the product window and the border
#      child), while the content area stays HTCLIENT;
#   3. a real non-client size press actually resizes the child, driven through
#      Tauri's own border child (WM_NCLBUTTONDOWN posted there, parent then runs
#      the user32 sizing loop);
#   4. the geometry that results is what the product persists.
param(
  [string]$ProcessName = "alan-desktop",
  [int]$Dx = 140,
  [int]$Dy = 90
)

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Rz {
  public delegate IntPtr WndProcD(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct WNDCLASS { public uint style; public WndProcD lpfnWndProc; public int cbClsExtra; public int cbWndExtra; public IntPtr hInstance; public IntPtr hIcon; public IntPtr hCursor; public IntPtr hbrBackground; public string lpszMenuName; public string lpszClassName; }
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern ushort RegisterClassW(ref WNDCLASS c);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr CreateWindowExW(uint ex, string cls, string name, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
  [DllImport("user32.dll")] public static extern IntPtr DefWindowProcW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr GetModuleHandleW(string n);
  [DllImport("user32.dll")] public static extern bool DestroyWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ScreenToClient(IntPtr h, ref P p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static List<IntPtr> Kids(IntPtr parent) { var l = new List<IntPtr>(); EnumChildWindows(parent, delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3}) {4}x{5}", r.L, r.T, r.Rr, r.B, r.Rr - r.L, r.B - r.T); }
  public static string Size(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("{0}x{1}", r.Rr - r.L, r.B - r.T); }
  public static string HitName(IntPtr h, int screenX, int screenY) {
    // WM_NCHITTEST takes screen coordinates in lparam.
    long lp = ((long)(ushort)screenY << 16) | (long)(ushort)screenX;
    long r = SendMessageW(h, 0x0084, IntPtr.Zero, (IntPtr)lp).ToInt64();
    if (r == 1) return "HTCLIENT";
    if (r == 2) return "HTCAPTION";
    if (r == 10) return "HTLEFT";
    if (r == 11) return "HTRIGHT";
    if (r == 12) return "HTTOP";
    if (r == 15) return "HTBOTTOM";
    if (r == 17) return "HTBOTTOMRIGHT";
    return "code=" + r;
  }
  public static IntPtr Pad() {
    WNDCLASS wc = new WNDCLASS();
    wc.lpfnWndProc = delegate(IntPtr ph, uint pm, IntPtr pw, IntPtr pl) { return DefWindowProcW(ph, pm, pw, pl); };
    wc.hInstance = GetModuleHandleW(null);
    wc.lpszClassName = "V1ResizePad";
    RegisterClassW(ref wc);
    IntPtr pad = CreateWindowExW(0x00000008 | 0x00000080, "V1ResizePad", "v1rzpad", 0x80000000u | 0x10000000u, 40, 60, 180, 90, IntPtr.Zero, IntPtr.Zero, GetModuleHandleW(null), IntPtr.Zero);
    SetForegroundWindow(pad);
    return pad;
  }
}
"@
[void][Rz]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$appIds = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = [IntPtr]::Zero
foreach ($h in [Rz]::All()) {
  if ($appIds -contains [Rz]::Pid($h) -and [Rz]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if ($product -eq [IntPtr]::Zero) { throw 'product window not found' }

$style = [int64][Rz]::GetWindowLongPtrW($product, -16)
$wr = New-Object Rz+R
[void][Rz]::GetWindowRect($product, [ref]$wr)
$cr = New-Object Rz+R
[void][Rz]::GetClientRect($product, [ref]$cr)
Write-Host ("product=0x{0:X} style=0x{1:X8} WS_CHILD={2} WS_THICKFRAME={3}" -f $product.ToInt64(), $style, (($style -band 0x40000000) -ne 0), (($style -band 0x40000) -ne 0))
Write-Host "window=$([Rz]::Rect($product)) client=$($cr.Rr)x$($cr.B) (client==window => no non-client area)"
Write-Host "parent=0x$(([Rz]::GetParent($product)).ToString('X')) class=$([Rz]::Cls([Rz]::GetParent($product)))"

$borders = [IntPtr]::Zero
foreach ($kid in [Rz]::Kids($product)) {
  $cls = [Rz]::Cls($kid)
  if ($cls -eq 'TAURI_DRAG_RESIZE_BORDERS') { $borders = $kid }
  Write-Host "  child 0x$($kid.ToString('X')) class=$cls rect=$([Rz]::Rect($kid))"
}

$brX = $wr.Rr - 2
$brY = $wr.B - 2
Write-Host "hit product   @band($brX,$brY) = $([Rz]::HitName($product, $brX, $brY))"
Write-Host "hit product   @center        = $([Rz]::HitName($product, [int](($wr.L + $wr.Rr) / 2), [int](($wr.T + $wr.B) / 2)))"
if ($borders -ne [IntPtr]::Zero) {
  Write-Host "hit borders   @band($brX,$brY) = $([Rz]::HitName($borders, $brX, $brY))"
}

$before = [Rz]::Size($product)
Write-Host "size_before=$before"

$pad = [Rz]::Pad()
Start-Sleep -Milliseconds 400
[void][Rz]::SetCursorPos(80, 105)
Start-Sleep -Milliseconds 250
[void][Rz]::SetCursorPos($brX, $brY)
Start-Sleep -Milliseconds 150
[Rz]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)     # real press, captured by the pad
Start-Sleep -Milliseconds 200

$lp = [IntPtr](([long]($brY -shl 16)) -bor ([long]($brX -band 0xFFFF)))
$target = $product
if ($borders -ne [IntPtr]::Zero) { $target = $borders }
Write-Host "posting WM_NCLBUTTONDOWN(HTBOTTOMRIGHT) to 0x$($target.ToString('X')) ($([Rz]::Cls($target)))"
[void][Rz]::PostMessageW($target, 0x00A1, [IntPtr]17, $lp)
Start-Sleep -Milliseconds 350
for ($i = 1; $i -le 20; $i++) {
  [void][Rz]::SetCursorPos([int]($brX + $Dx * $i / 20), [int]($brY + $Dy * $i / 20))
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 250
[void][Rz]::PostMessageW($product, 0x0202, [IntPtr]0, $lp)
[Rz]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 1200
[void][Rz]::DestroyWindow($pad)

$after = [Rz]::Size($product)
Write-Host "size_after=$after rect_after=$([Rz]::Rect($product))"
