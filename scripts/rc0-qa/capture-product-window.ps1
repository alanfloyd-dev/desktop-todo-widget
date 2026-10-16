# Capture the product window into a PNG so the rendered mode can be inspected
# even when another window covers it.
param(
  [string]$ProcessName = "alan-desktop",
  [string]$Out = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path '.rc0-evidence\restart-diag\window.png')
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Cap {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
}
"@
[void][Cap]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$appIds = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = [IntPtr]::Zero
foreach ($h in [Cap]::All()) {
  if ($appIds -contains [Cap]::Pid($h) -and [Cap]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if ($product -eq [IntPtr]::Zero) { throw 'product window not found' }

$r = New-Object Cap+R
[void][Cap]::GetWindowRect($product, [ref]$r)
$w = $r.Rr - $r.L
$hgt = $r.B - $r.T
Write-Host "product=0x$($product.ToString('X')) rect=($($r.L),$($r.T)) ${w}x${hgt}"
New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
$bmp = New-Object System.Drawing.Bitmap($w, $hgt)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$dc = $g.GetHdc()
# flag 2 = PW_RENDERFULLCONTENT, needed for composited/WebView2 content
[void][Cap]::PrintWindow($product, $dc, 2)
$g.ReleaseHdc($dc)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "saved $Out"
