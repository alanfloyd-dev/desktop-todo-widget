# Screen-capture a window by rectangle.
#
# `capture-product-window.ps1` uses PrintWindow, which returns an empty surface for
# this frameless, transparent, WebView2-composited window. This captures the real
# screen pixels inside the window's rect instead, so the rendered DOM can be
# inspected.
#
#   .\capture-screen.ps1 -Out C:\tmp\widget.png
#   .\capture-screen.ps1 -ProcessName alan-desktop -Out C:\tmp\widget.png
param(
  [string]$ProcessName = "alan-desktop",
  [Parameter(Mandatory = $true)][string]$Out
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Sc {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static bool PerMonitorAware() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
}
"@
[void][Sc]::PerMonitorAware()

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$hwnd = [IntPtr]::Zero
foreach ($h in [Sc]::All()) {
  if ($ids -contains [Sc]::Pid($h) -and [Sc]::Cls($h) -eq 'Tauri Window') { $hwnd = $h; break }
}
if ($hwnd -eq [IntPtr]::Zero) {
  foreach ($h in [Sc]::All()) {
    if ($ids -notcontains [Sc]::Pid($h)) { continue }
    foreach ($child in [Sc]::All()) { }
    $found = $false
    foreach ($c in [Sc]::All()) { if ([Sc]::Cls($c) -eq 'WRY_WEBVIEW' -and [Sc]::Pid($c) -eq [Sc]::Pid($h)) { $found = $true; break } }
    if ($found) { $hwnd = $h; break }
  }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "product window not found for $ProcessName" }

$r = New-Object Sc+R
[void][Sc]::GetWindowRect($hwnd, [ref]$r)
$w = $r.Rr - $r.L
$h = $r.B - $r.T
if ($w -le 0 -or $h -le 0) { throw "product window has no size: $w x $h" }
Write-Host "product=0x$($hwnd.ToString('X')) rect=($($r.L),$($r.T)) ${w}x${h}"

$dir = Split-Path -Parent $Out
if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$gfx.Dispose(); $bmp.Dispose()
Write-Host "saved $Out"
