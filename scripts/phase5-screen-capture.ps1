param(
  [Parameter(Mandatory = $true)][int]$X,
  [Parameter(Mandatory = $true)][int]$Y,
  [Parameter(Mandatory = $true)][ValidateRange(1, 10000)][int]$Width,
  [Parameter(Mandatory = $true)][ValidateRange(1, 10000)][int]$Height,
  [Parameter(Mandatory = $true)][string]$Path,
  [ValidateRange(0, 200)][int]$Margin = 0
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Phase5ScreenCaptureDpi {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);
}
"@
[void][Phase5ScreenCaptureDpi]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
[void][Phase5ScreenCaptureDpi]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))

$screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
$left = [Math]::Max($screen.Left, $X - $Margin)
$top = [Math]::Max($screen.Top, $Y - $Margin)
$right = [Math]::Min($screen.Right, $X + $Width + $Margin)
$bottom = [Math]::Min($screen.Bottom, $Y + $Height + $Margin)
$captureWidth = $right - $left
$captureHeight = $bottom - $top

$bitmap = [System.Drawing.Bitmap]::new($captureWidth, $captureHeight)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($left, $top, 0, 0, $bitmap.Size)
$bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "Captured $captureWidth x $captureHeight at $left,$top"
