param(
  [string]$ScreenshotPath = "",
  [string]$HwndHex = "",
  [switch]$All,
  [switch]$Fast,
  [string]$ProcessName = "alan-desktop"
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Phase5NativeProbe {
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumWindowsProc callback, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, System.Text.StringBuilder name, int count);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
  [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hwnd, int index);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string className, string windowName);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
  public static IntPtr FindByProcessId(uint targetPid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((top, unused) => {
      uint topPid;
      GetWindowThreadProcessId(top, out topPid);
      if (topPid == targetPid) { found = top; return false; }
      EnumChildWindows(top, (child, nested) => {
        uint childPid;
        GetWindowThreadProcessId(child, out childPid);
        if (childPid == targetPid) { found = child; return false; }
        return true;
      }, IntPtr.Zero);
      return found == IntPtr.Zero;
    }, IntPtr.Zero);
    return found;
  }
}
"@
[void][Phase5NativeProbe]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$process = Get-Process $ProcessName -ErrorAction Stop | Sort-Object StartTime -Descending | Select-Object -First 1
$candidates = [System.Collections.Generic.List[object]]::new()
$topHandles = [System.Collections.Generic.List[IntPtr]]::new()
$topCallback = [Phase5NativeProbe+EnumWindowsProc]{
  param([IntPtr]$hwnd, [IntPtr]$unused)
  $topHandles.Add($hwnd)
  return $true
}
$callback = [Phase5NativeProbe+EnumWindowsProc]{
  param([IntPtr]$hwnd, [IntPtr]$unused)
  $ownerPid = 0
  [void][Phase5NativeProbe]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
  if ($ownerPid -eq $process.Id) {
    $rect = New-Object Phase5NativeProbe+RECT
    [void][Phase5NativeProbe]::GetWindowRect($hwnd, [ref]$rect)
    $class = [System.Text.StringBuilder]::new(256)
    [void][Phase5NativeProbe]::GetClassName($hwnd, $class, $class.Capacity)
    $candidates.Add([pscustomobject]@{
      Hwnd = ('0x{0:X}' -f $hwnd.ToInt64())
      RawHwnd = $hwnd
      Class = $class.ToString()
      Parent = ('0x{0:X}' -f ([Phase5NativeProbe]::GetParent($hwnd)).ToInt64())
      Visible = [Phase5NativeProbe]::IsWindowVisible($hwnd)
      Dpi = [Phase5NativeProbe]::GetDpiForWindow($hwnd)
      X = $rect.Left
      Y = $rect.Top
      Width = $rect.Right - $rect.Left
      Height = $rect.Bottom - $rect.Top
      Style = ('0x{0:X}' -f ([Phase5NativeProbe]::GetWindowLongPtr($hwnd, -16)).ToInt64())
      ExStyle = ('0x{0:X}' -f ([Phase5NativeProbe]::GetWindowLongPtr($hwnd, -20)).ToInt64())
    })
  }
  return $true
}
if (-not $HwndHex) {
  [void][Phase5NativeProbe]::EnumWindows($topCallback, [IntPtr]::Zero)
  foreach ($top in $topHandles) {
    $topPid = 0
    [void][Phase5NativeProbe]::GetWindowThreadProcessId($top, [ref]$topPid)
    if (-not $Fast -or $topPid -eq $process.Id) {
      [void]$callback.Invoke($top, [IntPtr]::Zero)
      [void][Phase5NativeProbe]::EnumChildWindows($top, $callback, [IntPtr]::Zero)
    }
  }
}
$window = $candidates | Where-Object { $_.Visible -and $_.Width -gt 0 -and $_.Height -gt 0 } | Sort-Object { $_.Width * $_.Height } -Descending | Select-Object -First 1
if (-not $HwndHex -and -not $window) {
  $found = [Phase5NativeProbe]::FindByProcessId([uint32]$process.Id)
  if ($found -eq [IntPtr]::Zero) { $found = [Phase5NativeProbe]::FindWindow($null, "Alan Desktop") }
  if ($found -ne [IntPtr]::Zero) { $HwndHex = ('0x{0:X}' -f $found.ToInt64()) }
}
if ($HwndHex) {
  $raw = [IntPtr]::new([Convert]::ToInt64($HwndHex.Replace('0x', ''), 16))
  $rect = New-Object Phase5NativeProbe+RECT
  [void][Phase5NativeProbe]::GetWindowRect($raw, [ref]$rect)
  $class = [System.Text.StringBuilder]::new(256)
  [void][Phase5NativeProbe]::GetClassName($raw, $class, $class.Capacity)
  $window = [pscustomobject]@{
    Hwnd = ('0x{0:X}' -f $raw.ToInt64())
    RawHwnd = $raw
    Class = $class.ToString()
    Parent = ('0x{0:X}' -f ([Phase5NativeProbe]::GetParent($raw)).ToInt64())
    Visible = [Phase5NativeProbe]::IsWindowVisible($raw)
    Dpi = [Phase5NativeProbe]::GetDpiForWindow($raw)
    X = $rect.Left
    Y = $rect.Top
    Width = $rect.Right - $rect.Left
    Height = $rect.Bottom - $rect.Top
    Style = ('0x{0:X}' -f ([Phase5NativeProbe]::GetWindowLongPtr($raw, -16)).ToInt64())
    ExStyle = ('0x{0:X}' -f ([Phase5NativeProbe]::GetWindowLongPtr($raw, -20)).ToInt64())
  }
}
if ($All) {
  $candidates | Select-Object -Property Hwnd,Class,Parent,Visible,Dpi,X,Y,Width,Height,Style,ExStyle | ConvertTo-Json -Compress
  return
}
if (-not $window) { throw "Alan Desktop visible HWND was not found." }

if ($ScreenshotPath) {
  $bitmap = [System.Drawing.Bitmap]::new($window.Width, $window.Height)
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $hdc = $graphics.GetHdc()
  [void][Phase5NativeProbe]::PrintWindow($window.RawHwnd, $hdc, 2)
  $graphics.ReleaseHdc($hdc)
  $bitmap.Save($ScreenshotPath, [System.Drawing.Imaging.ImageFormat]::Png)
  $graphics.Dispose()
  $bitmap.Dispose()
}

$window.PSObject.Properties.Remove('RawHwnd')
$child = [IntPtr]::new([Convert]::ToInt64($window.Hwnd.Replace('0x', ''), 16))
$root = [Phase5NativeProbe]::GetAncestor($child, 2)
$rootRect = New-Object Phase5NativeProbe+RECT
[void][Phase5NativeProbe]::GetWindowRect($root, [ref]$rootRect)
$rootClass = [System.Text.StringBuilder]::new(256)
[void][Phase5NativeProbe]::GetClassName($root, $rootClass, $rootClass.Capacity)
$window | Add-Member -NotePropertyName RootHwnd -NotePropertyValue ('0x{0:X}' -f $root.ToInt64())
$window | Add-Member -NotePropertyName RootClass -NotePropertyValue $rootClass.ToString()
$window | Add-Member -NotePropertyName RootRect -NotePropertyValue ("{0},{1} {2}x{3}" -f $rootRect.Left, $rootRect.Top, ($rootRect.Right - $rootRect.Left), ($rootRect.Bottom - $rootRect.Top))
$window | ConvertTo-Json -Compress
