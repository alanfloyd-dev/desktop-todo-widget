# Capture the live product window by raising it above other windows first.
#
# `capture-product-window.ps1` (PrintWindow) returns a stale surface for this
# frameless, transparent, WebView2-composited window, and a plain screen grab
# captures whatever window happens to cover the widget's rect. This resolves the
# product HWND by process (any class name, so Desktop mode and restyles are
# covered), brings it to the foreground, then copies the real screen pixels.
param(
  [string]$ProcessName = 'alan-desktop',
  [Parameter(Mandatory = $true)][string]$Out,
  [int]$SettleMs = 900,
  [switch]$NoRaise,
  # Move the widget to "x,y" for the capture and restore it afterwards. Raising
  # the window is unreliable while another process holds the foreground lock, so
  # relocating it to a clear area is the dependable way to see its pixels. The
  # original rectangle is restored afterwards, so persisted geometry is unchanged.
  [string]$MoveTo = ''
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Cap2 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint attach, uint attachTo, bool attachFlag);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();

  /// Force a window to the foreground across the foreground lock by attaching to
  /// the thread that currently owns the foreground.
  public static bool ForceForeground(IntPtr h) {
    IntPtr fg = GetForegroundWindow();
    uint targetThread = Pid(h);
    uint fgThread = fg == IntPtr.Zero ? 0 : Pid(fg);
    uint me = GetCurrentThreadId();
    bool ok = false;
    if (fgThread != 0 && fgThread != me) { AttachThreadInput(me, fgThread, true); }
    if (targetThread != 0 && targetThread != me) { AttachThreadInput(me, targetThread, true); }
    try {
      ShowWindow(h, 5);
      BringWindowToTop(h);
      SetForegroundWindow(h);
      ok = GetForegroundWindow() == h;
    } finally {
      if (targetThread != 0 && targetThread != me) { AttachThreadInput(me, targetThread, false); }
      if (fgThread != 0 && fgThread != me) { AttachThreadInput(me, fgThread, false); }
    }
    return ok;
  }
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static bool Kids(IntPtr parent) {
    bool found = false;
    EnumChildWindows(parent, delegate(IntPtr h, IntPtr p) { if (Cls(h) == "WRY_WEBVIEW") { found = true; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
[void][Cap2]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$hwnd = [IntPtr]::Zero
# Preferred: the visible top-level window that hosts the WebView — independent of
# the current window class and of the mode's parenting.
foreach ($h in [Cap2]::All()) {
  if ($ids -notcontains [Cap2]::Pid($h)) { continue }
  if (-not [Cap2]::IsWindowVisible($h)) { continue }
  if ([Cap2]::Kids($h)) { $hwnd = $h; break }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "product window not found for $ProcessName" }

$r = New-Object Cap2+R
[void][Cap2]::GetWindowRect($hwnd, [ref]$r)
$w = $r.Rr - $r.L
$h = $r.B - $r.T
if ($w -le 0 -or $h -le 0) { throw "product window has no size: $w x $h" }
Write-Output "product=0x$($hwnd.ToString('X')) class=$([Cap2]::Cls($hwnd)) rect=($($r.L),$($r.T)) ${w}x${h}"

if (-not $NoRaise) {
  # Minimise only top-level windows that actually cover the widget's rectangle.
  # `EnumChildWindows(GetDesktopWindow())` also yields the widget's own children,
  # so skip anything parented into the product and never hide the desktop shell.
  $minimised = @()
  foreach ($h in [Cap2]::All()) {
    if ($h -eq $hwnd) { continue }
    if ([Cap2]::GetParent($h) -ne [IntPtr]::Zero) { continue }
    if (-not [Cap2]::IsWindowVisible($h)) { continue }
    if ([Cap2]::IsIconic($h)) { continue }
    $cr = New-Object Cap2+R
    [void][Cap2]::GetWindowRect($h, [ref]$cr)
    $cw = $cr.Rr - $cr.L
    $ch = $cr.B - $cr.T
    if ($cw -le 0 -or $ch -le 0) { continue }
    $overlaps = -not ($cr.Rr -le $r.L -or $cr.L -ge $r.Rr -or $cr.B -le $r.T -or $cr.T -ge $r.B)
    if (-not $overlaps) { continue }
    if ($cw -ge 2500 -and $ch -ge 1500) { continue }          # never hide the desktop shell
    [void][Cap2]::ShowWindow($h, 6)                            # SW_MINIMIZE
    $minimised += ('0x{0:X}:{1}' -f $h.ToInt64(), [Cap2]::Cls($h))
  }
  Write-Output "minimised overlapping top-level windows: $($minimised.Count) [$($minimised -join ' ')]"
  $raised = [Cap2]::ForceForeground($hwnd)
  Start-Sleep -Milliseconds $SettleMs
  $fg = [Cap2]::GetForegroundWindow()
  Write-Output "ForceForeground=$raised foreground now=0x$($fg.ToString('X')) isProduct=$($fg -eq $hwnd)"
}

$dir = Split-Path -Parent $Out
if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }

$originX = 0; $originY = 0; $moved = $false
if ($MoveTo) {
  $parts = $MoveTo -split ','
  $originX = [int]$parts[0]; $originY = [int]$parts[1]
  $ok = [Cap2]::SetWindowPos($hwnd, [IntPtr]::Zero, $originX, $originY, 0, 0, 0x0001 -bor 0x0004)  # NOSIZE|NOZORDER
  Start-Sleep -Milliseconds 700
  $moved = $true
  Write-Output "SetWindowPos to ($originX,$originY) -> $ok"
}

# Re-read the rect after any raise: a mode transition or a reparent during the
# settle window would otherwise leave the copy reading a stale rectangle.
# `GetWindowRect` also intermittently leaves the out-struct unpopulated while the
# window is being restyled (the framework reaches a garbage Bottom), so the read
# is validated and retried rather than trusted once.
$fx = 0; $fy = 0; $fw = 0; $fh = 0
for ($attempt = 1; $attempt -le 12; $attempt++) {
  $fresh = New-Object Cap2+R
  [void][Cap2]::GetWindowRect($hwnd, [ref]$fresh)
  $fx = $fresh.L; $fy = $fresh.T; $fw = $fresh.Rr - $fresh.L; $fh = $fresh.B - $fresh.T
  if ($fw -gt 0 -and $fh -gt 0 -and $fh -lt 20000 -and $fw -lt 20000) { break }
  if ($attempt -eq 12) { throw "invalid capture rect ${fw}x${fh} at ($fx,$fy) after $attempt attempts" }
  Start-Sleep -Milliseconds 150
}
Write-Output "capture rect=($fx,$fy) ${fw}x${fh} (pre-raise rect was ($($r.L),$($r.T)) ${w}x${h})"

$bmp = $null
try {
  $bmp = New-Object System.Drawing.Bitmap($fw, $fh)
} catch {
  throw "Bitmap($fw,$fh) failed: $($_.Exception.Message)"
}
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($fx, $fy, 0, 0, $bmp.Size)
$gfx.Dispose()
# Save through memory first: a plain File.Save on a path another process still
# holds open fails with a generic GDI+ error and loses the capture.
$stream = New-Object System.IO.MemoryStream
$bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
[System.IO.File]::WriteAllBytes($Out, $stream.ToArray())
$stream.Dispose()
Write-Output "saved $Out ($((Get-Item $Out).Length) bytes)"

if ($moved) {
  # Put the widget back where it was so the run leaves no trace on the desktop.
  Start-Sleep -Milliseconds 400
  $back = [Cap2]::SetWindowPos($hwnd, [IntPtr]::Zero, $r.L, $r.T, $w, $h, 0x0004)  # NOZORDER
  Write-Output "restored widget to ($($r.L),$($r.T)) ${w}x${h} -> $back"
}
