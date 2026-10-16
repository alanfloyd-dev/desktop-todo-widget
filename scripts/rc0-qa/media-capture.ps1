# Capture the product widget: resolve its HWND, optionally focus it, crop to the
# widget rect and/or record a region.
#
# The widget is a transparent frameless window, so a screen-pixel grab inside its
# rect is the only capture that shows the real Acrylic composite. Focusing it
# first matters because the native Acrylic surface can fall back while the window
# is unfocused, and a fallback frame must not be published as the Acrylic asset.
param(
  [string]$ProcessName = 'alan-desktop',
  [string]$Out = '',
  # Vertical pixels to drop from the top of the captured rect (e.g. to exclude a
  # region above the widget). Applied to both stills and rect reporting.
  [int]$TrimTop = 0,
  [Parameter(Mandatory = $true)][ValidateSet('probe', 'focus', 'capture', 'record')][string]$Action,
  [string]$MoveTo = '',
  [int]$RecordSeconds = 20,
  [int]$Fps = 24,
  [string]$Ffmpeg = 'ffmpeg',
  [switch]$NoFocus
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Cap {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out P p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }

  public static bool Dpi() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> Kids(IntPtr parent) {
    var l = new List<IntPtr>();
    EnumChildWindows(parent, delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static List<IntPtr> Top() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  /// Top-level windows plus every descendant of the desktop window. Desktop mode
  /// reparents the product HWND under SHELLDLL_DefView, so it is a child and
  /// EnumWindows alone cannot see it.
  public static List<IntPtr> All() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static bool HasWebview(IntPtr parent) {
    foreach (var k in Kids(parent)) { if (Cls(k) == "WRY_WEBVIEW") return true; }
    return false;
  }
  /// Force the window to the foreground across the foreground lock.
  public static bool Focus(IntPtr h) {
    IntPtr fg = GetForegroundWindow();
    uint target = Pid(h);
    uint fgThread = fg == IntPtr.Zero ? 0 : Pid(fg);
    uint me = GetCurrentThreadId();
    if (fgThread != 0 && fgThread != me) AttachThreadInput(me, fgThread, true);
    if (target != 0 && target != me) AttachThreadInput(me, target, true);
    try { ShowWindow(h, 5); BringWindowToTop(h); SetForegroundWindow(h); }
    finally {
      if (target != 0 && target != me) AttachThreadInput(me, target, false);
      if (fgThread != 0 && fgThread != me) AttachThreadInput(me, fgThread, false);
    }
    return GetForegroundWindow() == h;
  }
}
"@
[void][Cap]::Dpi()

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$hwnd = [IntPtr]::Zero
# Two passes: a top-level window that hosts the WebView (Floating/Sidebar), then a
# desktop-child window (Desktop mode), where the class is not 'Tauri Window'
# anymore and the HWND is only reachable as a shell descendant.
foreach ($h in [Cap]::All()) {
  if ($ids -notcontains [Cap]::Pid($h)) { continue }
  if (-not [Cap]::IsWindowVisible($h)) { continue }
  if ([Cap]::Cls($h) -eq 'Tauri Window') { $hwnd = $h; break }
}
if ($hwnd -eq [IntPtr]::Zero) {
  foreach ($h in [Cap]::All()) {
    if ($ids -notcontains [Cap]::Pid($h)) { continue }
    if ([Cap]::Cls($h) -ne 'Tauri Window') { continue }
    $hwnd = $h; break
  }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "product window not found for $ProcessName" }

if ($MoveTo) {
  $parts = $MoveTo -split ','
  [void][Cap]::SetWindowPos($hwnd, [IntPtr]::Zero, [int]$parts[0], [int]$parts[1], 0, 0, 0x0001 -bor 0x0004)
  Start-Sleep -Milliseconds 800
}

function Get-Rect {
  for ($i = 0; $i -lt 12; $i++) {
    $r = New-Object Cap+R
    [void][Cap]::GetWindowRect($hwnd, [ref]$r)
    $w = $r.Rr - $r.L; $h = $r.B - $r.T
    if ($w -gt 0 -and $h -gt 0 -and $w -lt 20000 -and $h -lt 20000) {
      return [pscustomobject]@{ X = $r.L; Y = $r.T; W = $w; H = $h }
    }
    Start-Sleep -Milliseconds 150
  }
  throw "invalid window rect"
}

if ($Action -eq 'probe') {
  $rect = Get-Rect
  $fg = [Cap]::GetForegroundWindow()
  [pscustomobject]@{
    Hwnd = ('0x{0:X}' -f $hwnd.ToInt64())
    Class = [Cap]::Cls($hwnd)
    Rect = "$($rect.X),$($rect.Y) $($rect.W)x$($rect.H)"
    IsForeground = ($fg -eq $hwnd)
    ForegroundHwnd = ('0x{0:X}' -f $fg.ToInt64())
    ProcessId = [Cap]::Pid($hwnd)
  } | Format-List | Out-String | Write-Output
}

elseif ($Action -eq 'focus') {
  $focused = [Cap]::Focus($hwnd)
  Start-Sleep -Milliseconds 900
  Write-Output "focus requested=$focused foregroundMatches=$([Cap]::GetForegroundWindow() -eq $hwnd)"
}

elseif ($Action -eq 'capture') {
  if (-not $NoFocus) { [void][Cap]::Focus($hwnd); Start-Sleep -Milliseconds 1200 }
  $rect = Get-Rect
  $y = $rect.Y + $TrimTop
  $h = $rect.H - $TrimTop
  $bmp = New-Object System.Drawing.Bitmap($rect.W, $h)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($rect.X, $y, 0, 0, $bmp.Size)
  $g.Dispose()
  $dir = Split-Path -Parent $Out
  if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
  $ms = New-Object System.IO.MemoryStream
  $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  [System.IO.File]::WriteAllBytes($Out, $ms.ToArray())
  $ms.Dispose()
  Write-Output "saved $Out  rect=$($rect.X),$y $($rect.W)x$h  foreground=$([Cap]::GetForegroundWindow() -eq $hwnd)"
}

elseif ($Action -eq 'record') {
  if (-not $NoFocus) { [void][Cap]::Focus($hwnd); Start-Sleep -Milliseconds 800 }
  $rect = Get-Rect
  $y = $rect.Y + $TrimTop
  $h = $rect.H - $TrimTop
  # Crop to an even-sized region: H.264 requires even dimensions.
  $w = $rect.W - ($rect.W % 2)
  $h = $h - ($h % 2)
  $y = $y - ($y % 2)
  $x = $rect.X - ($rect.X % 2)
  Write-Output "recording region ${x},${y} ${w}x${h} for ${RecordSeconds}s at ${Fps}fps"
  $args = @(
    '-y', '-hide_banner', '-loglevel', 'error',
    '-f', 'gdigrab', '-framerate', $Fps, '-draw_mouse', '0',
    '-offset_x', $x, '-offset_y', $y, '-video_size', "${w}x${h}",
    '-t', $RecordSeconds,
    '-i', 'desktop',
    '-c:v', 'libx264', '-preset', 'medium', '-crf', '20',
    '-pix_fmt', 'yuv420p', '-movflags', '+faststart',
    $Out
  )
  & $Ffmpeg @args
  Write-Output "ffmpeg exit=$LASTEXITCODE"
}
