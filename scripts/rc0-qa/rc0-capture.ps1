# RC-0 QA helper: captures a screen region to a PNG using System.Drawing.
#
# The process opts into per-monitor DPI awareness first, otherwise Windows
# virtualises the coordinates and the capture is silently scaled. Window regions
# are padded a little so the result shows whether the frameless window has a
# visible border or shadow.
param(
  [string]$Out = "rc0-capture.png",
  [int]$X, [int]$Y, [int]$W, [int]$H,
  [string]$ProcessName = "alan-desktop",
  [int]$Pad = 24
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Rc0Cap {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
}
"@
if (-not [Rc0Cap]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))) {
  [void][Rc0Cap]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
}

if (-not $X -and $ProcessName) {
  Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Rc0Find {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h,p)=>{l.Add(h);return true;}, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
}
"@
  $ids = (Get-Process -Name $ProcessName -ErrorAction Stop).Id
  $found = $null
  foreach ($h in [Rc0Find]::Top()) {
    if ($ids -contains [int][Rc0Find]::Pid($h) -and [Rc0Find]::Cls($h) -eq 'Tauri Window') { $found = $h; break }
  }
  if (-not $found) { throw "product window not found" }
  $r = New-Object Rc0Find+RECT
  [void][Rc0Find]::GetWindowRect($found, [ref]$r)
  $X = $r.Left - $Pad; $Y = $r.Top - $Pad
  $W = ($r.Right - $r.Left) + 2 * $Pad; $H = ($r.Bottom - $r.Top) + 2 * $Pad
  Write-Host ("window rect: {0},{1} {2}x{3} px" -f $r.Left, $r.Top, ($r.Right - $r.Left), ($r.Bottom - $r.Top))
}

$bmp = New-Object System.Drawing.Bitmap($W, $H)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($X, $Y, 0, 0, (New-Object System.Drawing.Size($W, $H)))
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "saved $Out  region=$X,$Y ${W}x${H}"
