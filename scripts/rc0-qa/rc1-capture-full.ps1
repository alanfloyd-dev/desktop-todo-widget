# Capture the whole virtual screen plus the product window rect, so a capture can
# always be interpreted against the real desktop layout.
param(
  [Parameter(Mandatory = $true)][string]$Out,
  [string]$ProcessName = 'alan-desktop'
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Full {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
}
"@
[void][Full]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
Write-Output "virtual screen: $($bounds.X),$($bounds.Y) $($bounds.Width)x$($bounds.Height)"

$ids = @()
try { $ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id) } catch { }
foreach ($h in [Full]::All()) {
  if ($ids -notcontains [Full]::Pid($h)) { continue }
  $r = New-Object Full+R
  [void][Full]::GetWindowRect($h, [ref]$r)
  Write-Output ("product window 0x{0:X} class={1} rect=({2},{3}) {4}x{5}" -f $h.ToInt64(), [Full]::Cls($h), $r.L, $r.T, ($r.Rr - $r.L), ($r.B - $r.T))
}

$dir = Split-Path -Parent $Out
if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bmp.Size)
$gfx.Dispose()
$stream = New-Object System.IO.MemoryStream
$bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
[System.IO.File]::WriteAllBytes($Out, $stream.ToArray())
$stream.Dispose()
Write-Output "saved $Out ($((Get-Item $Out).Length) bytes)"
