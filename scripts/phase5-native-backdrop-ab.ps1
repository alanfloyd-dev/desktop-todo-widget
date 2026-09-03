param(
  [Parameter(Mandatory = $true)][ValidateSet('None', 'Transient')][string]$Backdrop,
  [Parameter(Mandatory = $true)][ValidateSet('Normal', 'Full')][string]$Frame,
  [Parameter(Mandatory = $true)][string]$ImagePath,
  [string]$JsonPath,
  [string]$ProcessName = 'alan-desktop',
  [ValidateSet('Default', 'False', 'True')][string]$RedirectionAlpha = 'Default',
  [ValidateRange(0, 200)][int]$Margin = 40
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class Phase5BackdropNative {
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);

  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }

  [StructLayout(LayoutKind.Sequential)]
  public struct MARGINS { public int Left, Right, Top, Bottom; }

  [StructLayout(LayoutKind.Sequential)]
  public struct ACCENT_POLICY {
    public int AccentState;
    public int AccentFlags;
    public uint GradientColor;
    public int AnimationId;
  }

  [StructLayout(LayoutKind.Sequential)]
  public struct WINDOWCOMPOSITIONATTRIBDATA {
    public int Attrib;
    public IntPtr Data;
    public UIntPtr SizeOfData;
  }

  [DllImport("user32.dll")]
  public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
  [DllImport("user32.dll")]
  public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
  [DllImport("user32.dll")]
  public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")]
  public static extern bool SetWindowPos(IntPtr hwnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
  public static extern IntPtr GetWindowLongPtr(IntPtr hwnd, int index);
  [DllImport("user32.dll")]
  public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern int GetClassName(IntPtr hwnd, StringBuilder className, int maxCount);
  [DllImport("user32.dll")]
  public static extern bool SetWindowCompositionAttribute(IntPtr hwnd, ref WINDOWCOMPOSITIONATTRIBDATA data);
  [DllImport("user32.dll")]
  public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")]
  public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);

  [DllImport("dwmapi.dll", PreserveSig = true)]
  public static extern int DwmSetWindowAttribute(IntPtr hwnd, int attribute, ref int value, int size);
  [DllImport("dwmapi.dll", PreserveSig = true)]
  public static extern int DwmGetWindowAttribute(IntPtr hwnd, int attribute, out int value, int size);
  [DllImport("dwmapi.dll", PreserveSig = true)]
  public static extern int DwmExtendFrameIntoClientArea(IntPtr hwnd, ref MARGINS margins);
  [DllImport("dwmapi.dll", PreserveSig = true)]
  public static extern int DwmFlush();

  public static IntPtr FindVisibleTopLevelWindow(uint targetProcessId) {
    IntPtr result = IntPtr.Zero;
    long resultArea = 0;
    EnumWindows((hwnd, unused) => {
      uint processId;
      GetWindowThreadProcessId(hwnd, out processId);
      if (processId == targetProcessId && IsWindowVisible(hwnd)) {
        StringBuilder className = new StringBuilder(256);
        GetClassName(hwnd, className, className.Capacity);
        string name = className.ToString();
        RECT rect;
        GetWindowRect(hwnd, out rect);
        long area = Math.Max(0, rect.Right - rect.Left) * (long)Math.Max(0, rect.Bottom - rect.Top);
        if (name != "Tao Thread Event Target" && name != "tray_icon_app" && area > resultArea) {
          result = hwnd;
          resultArea = area;
        }
      }
      return true;
    }, IntPtr.Zero);
    return result;
  }

  public static bool DisableLegacyAccent(IntPtr hwnd) {
    ACCENT_POLICY policy = new ACCENT_POLICY { AccentState = 0 };
    int size = Marshal.SizeOf(policy);
    IntPtr pointer = Marshal.AllocHGlobal(size);
    try {
      Marshal.StructureToPtr(policy, pointer, false);
      WINDOWCOMPOSITIONATTRIBDATA data = new WINDOWCOMPOSITIONATTRIBDATA {
        Attrib = 0x13,
        Data = pointer,
        SizeOfData = (UIntPtr)size
      };
      return SetWindowCompositionAttribute(hwnd, ref data);
    } finally {
      Marshal.FreeHGlobal(pointer);
    }
  }
}
"@

[void][Phase5BackdropNative]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
[void][Phase5BackdropNative]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))

$process = Get-Process -Name $ProcessName -ErrorAction Stop |
  Sort-Object StartTime -Descending |
  Select-Object -First 1
if (-not $process) {
  throw "No visible $ProcessName process was found"
}

$hwnd = [Phase5BackdropNative]::FindVisibleTopLevelWindow([uint32]$process.Id)
if ($hwnd -eq [IntPtr]::Zero) {
  throw "No visible top-level HWND was found for process $($process.Id)"
}

$hwndTopmost = [IntPtr]::new(-1)
$showNoActivate = 0x0010 -bor 0x0002 -bor 0x0001
[void][Phase5BackdropNative]::SetWindowPos($hwnd, $hwndTopmost, 0, 0, 0, 0, $showNoActivate)
[void][Phase5BackdropNative]::SetForegroundWindow($hwnd)

$className = [Text.StringBuilder]::new(256)
[void][Phase5BackdropNative]::GetClassName($hwnd, $className, $className.Capacity)
$rect = [Phase5BackdropNative+RECT]::new()
if (-not [Phase5BackdropNative]::GetWindowRect($hwnd, [ref]$rect)) {
  throw 'GetWindowRect failed'
}

$attribute = 38 # DWMWA_SYSTEMBACKDROP_TYPE
$style = [Phase5BackdropNative]::GetWindowLongPtr($hwnd, -16).ToInt64()
$extendedStyle = [Phase5BackdropNative]::GetWindowLongPtr($hwnd, -20).ToInt64()
$beforeValue = -1
$beforeGetHresult = [Phase5BackdropNative]::DwmGetWindowAttribute($hwnd, $attribute, [ref]$beforeValue, 4)
$legacyAccentDisabled = [Phase5BackdropNative]::DisableLegacyAccent($hwnd)

$redirectionAlphaHresult = $null
if ($RedirectionAlpha -ne 'Default') {
  $redirectionAlphaValue = if ($RedirectionAlpha -eq 'True') { 1 } else { 0 }
  $redirectionAlphaHresult = [Phase5BackdropNative]::DwmSetWindowAttribute($hwnd, 39, [ref]$redirectionAlphaValue, 4)
}

$margins = [Phase5BackdropNative+MARGINS]::new()
if ($Frame -eq 'Full') {
  $margins.Left = -1
  $margins.Right = -1
  $margins.Top = -1
  $margins.Bottom = -1
}
$extendHresult = [Phase5BackdropNative]::DwmExtendFrameIntoClientArea($hwnd, [ref]$margins)

$requestedValue = if ($Backdrop -eq 'Transient') { 3 } else { 1 }
$setHresult = [Phase5BackdropNative]::DwmSetWindowAttribute($hwnd, $attribute, [ref]$requestedValue, 4)
$effectiveValue = -1
$getHresult = [Phase5BackdropNative]::DwmGetWindowAttribute($hwnd, $attribute, [ref]$effectiveValue, 4)
$flushHresult = [Phase5BackdropNative]::DwmFlush()
Start-Sleep -Milliseconds 1500

$screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
$left = [Math]::Max($screen.Left, $rect.Left - $Margin)
$top = [Math]::Max($screen.Top, $rect.Top - $Margin)
$right = [Math]::Min($screen.Right, $rect.Right + $Margin)
$bottom = [Math]::Min($screen.Bottom, $rect.Bottom + $Margin)
$width = $right - $left
$height = $bottom - $top
$bitmap = [System.Drawing.Bitmap]::new($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
  $graphics.CopyFromScreen($left, $top, 0, 0, $bitmap.Size)
  $bitmap.Save($ImagePath, [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
  $graphics.Dispose()
  $bitmap.Dispose()
}

function Format-HResult([int]$value) {
  return ('0x{0:X8}' -f ($value -band 0xFFFFFFFFL))
}

$result = [ordered]@{
  processId = $process.Id
  hwnd = ('0x{0:X}' -f $hwnd.ToInt64())
  className = $className.ToString()
  style = ('0x{0:X}' -f $style)
  extendedStyle = ('0x{0:X}' -f $extendedStyle)
  wsExLayered = (($extendedStyle -band 0x00080000) -ne 0)
  wsExNoRedirectionBitmap = (($extendedStyle -band 0x00200000) -ne 0)
  rect = [ordered]@{ left = $rect.Left; top = $rect.Top; right = $rect.Right; bottom = $rect.Bottom }
  currentBackendBeforeExperiment = [ordered]@{
    getHresult = (Format-HResult $beforeGetHresult)
    effectiveValue = $beforeValue
  }
  legacyAccentDisabled = $legacyAccentDisabled
  redirectionAlpha = $RedirectionAlpha
  redirectionAlphaHresult = if ($null -eq $redirectionAlphaHresult) { $null } else { Format-HResult $redirectionAlphaHresult }
  requestedBackdrop = $Backdrop
  requestedValue = $requestedValue
  frame = $Frame
  extendFrameHresult = (Format-HResult $extendHresult)
  setBackdropHresult = (Format-HResult $setHresult)
  getBackdropHresult = (Format-HResult $getHresult)
  effectiveValue = $effectiveValue
  dwmFlushHresult = (Format-HResult $flushHresult)
  screenshot = $ImagePath
}
$json = $result | ConvertTo-Json -Depth 5
if ($JsonPath) {
  [System.IO.File]::WriteAllText($JsonPath, $json, [Text.Encoding]::UTF8)
}
$json
