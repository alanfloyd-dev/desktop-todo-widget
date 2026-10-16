# Record the README demo clip: real product transitions captured from the widget.
#
# The Acrylic compositor only paints a window that is on screen, so the clip cannot
# be assembled from off-screen renders. This records the live screen region while a
# CDP driver performs the sequence, with the operator's own windows parked outside
# the virtual screen for the duration (and put back afterwards).
param(
  [string]$Out = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'docs\assets\demo.mp4'),
  [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
  [string]$Ffmpeg = "$env:TEMP\dtw-ffmpeg\ffmpeg-9.0.1-full_build\bin\ffmpeg.exe",
  # Region and size names are spelled out deliberately: PowerShell variable names
  # are case-insensitive, so a local `$h` silently overwrites a parameter `$H` and
  # the encode then gets an array instead of a number.
  [int]$RegionX = 650, [int]$RegionY = 250, [int]$RegionW = 800, [int]$RegionH = 1150,
  [int]$Seconds = 25,
  [int]$Fps = 24,
  [string]$ParkHandles = '',
  [string]$Exe = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'src-tauri\target\release\alan-desktop.exe')
)

$ErrorActionPreference = 'Stop'
$capture = Join-Path $Root 'scripts\rc0-qa\media-capture.ps1'
$driver = Join-Path $Root 'scripts\rc0-qa\media-demo-drive.mjs'

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Park {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static int[] Snapshot(IntPtr h) { R r; GetWindowRect(h, out r); return new int[] { r.L, r.T, r.Rr-r.L, r.B-r.T }; }
  public static bool Move(IntPtr h, int x, int y, int w, int hh) { return SetWindowPos(h, IntPtr.Zero, x, y, w, hh, 0x0004 | 0x0010); }
  /// Every visible top-level window of `pid`, largest frames first.
  public static List<IntPtr> Frames(uint pid) {
    var found = new List<IntPtr>();
    var sizes = new List<int>();
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      uint q; GetWindowThreadProcessId(h, out q);
      if (q != pid || !IsWindowVisible(h)) return true;
      R r; GetWindowRect(h, out r);
      int w = r.Rr - r.L, hh = r.B - r.T;
      found.Add(h); sizes.Add(w * hh);
      return true;
    }, IntPtr.Zero);
    var order = new List<int>();
    for (int i = 0; i < found.Count; i++) order.Add(i);
    order.Sort((a, b) => sizes[b].CompareTo(sizes[a]));
    var result = new List<IntPtr>();
    foreach (var i in order) result.Add(found[i]);
    return result;
  }
}
"@

# Restart the widget in the state the clip should open on.
& python (Join-Path $Root '.rc0-evidence\rc1-settings.py') set mode=floating floatingPresentation=expanded | Out-Null
Get-Process alan-desktop, desktop-todo-widget -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 3
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9340'
Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
Start-Sleep -Seconds 16
& powershell -ExecutionPolicy Bypass -File $capture -Action focus | Out-Null
& powershell -ExecutionPolicy Bypass -File $capture -Action probe | Select-String 'Rect' | ForEach-Object { $_.Line }

$saved = @()
try {
  if ($ParkHandles) {
    $offset = 2600
    foreach ($entry in ($ParkHandles.Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
      if ($entry -like 'pid:*') {
        # Owning process given instead of a handle: find its largest visible frame.
        $wantPid = [uint32]$entry.Substring(4)
        foreach ($h in [Park]::Frames($wantPid)) {
          $rect = [Park]::Snapshot($h)
          if ($rect[2] -lt 300 -or $rect[3] -lt 200) { continue }
          $saved += [pscustomobject]@{ Handle = ('0x{0:X}' -f $h.ToInt64()); X = $rect[0]; Y = $rect[1]; W = $rect[2]; H = $rect[3] }
          [void][Park]::Move($h, $offset, 40, $rect[2], $rect[3])
          $offset += 80
        }
        continue
      }
      $h = [IntPtr]::new([Convert]::ToInt64($entry, 16))
      $rect = [Park]::Snapshot($h)
      Write-Output ("  handle {0}: rect=({1},{2}) {3}x{4}" -f $entry, $rect[0], $rect[1], $rect[2], $rect[3])
      if ($rect[2] -lt 300 -or $rect[3] -lt 200) { Write-Output "    skipped (too small / stale handle)"; continue }
      $saved += [pscustomobject]@{ Handle = $entry; X = $rect[0]; Y = $rect[1]; W = $rect[2]; H = $rect[3] }
      [void][Park]::Move($h, $offset, 40, $rect[2], $rect[3])
      $offset += 80
    }
    Start-Sleep -Milliseconds 1200
    Write-Output "parked $($saved.Count) window(s) off-screen"
  }

  # Even dimensions: H.264 requires them, and gdigrab honors the offset exactly.
  $encX = $RegionX - ($RegionX % 2)
  $encY = $RegionY - ($RegionY % 2)
  $encW = $RegionW - ($RegionW % 2)
  $encH = $RegionH - ($RegionH % 2)
  Write-Output "recording ${encX},${encY} ${encW}x${encH} for ${Seconds}s at ${Fps}fps -> $Out"

  # NOTE: the parameter array must not be called `$args` - that name is PowerShell's
  # automatic argument array, and assigning to it silently corrupts the values.
  $recordArgs = @(
    '-y', '-hide_banner', '-loglevel', 'error',
    '-f', 'gdigrab', '-framerate', $Fps, '-draw_mouse', '0',
    '-offset_x', $encX, '-offset_y', $encY, '-video_size', "${encW}x${encH}",
    '-t', $Seconds, '-i', 'desktop',
    '-c:v', 'libx264', '-preset', 'medium', '-crf', '20',
    '-pix_fmt', 'yuv420p', '-movflags', '+faststart',
    $Out
  )
  $ff = Start-Process -FilePath $Ffmpeg -ArgumentList $recordArgs -PassThru -NoNewWindow
  Start-Sleep -Seconds 2
  Write-Output "ffmpeg pid=$($ff.Id) recording; running the demo driver"
  $env:CDP_PORT = '9340'
  & node $driver
  Write-Output "driver finished; waiting for ffmpeg"
  $ff.WaitForExit()
  Write-Output "ffmpeg exit=$($ff.ExitCode)"
}
finally {
  Start-Sleep -Milliseconds 400
  foreach ($item in $saved) {
    $h = [IntPtr]::new([Convert]::ToInt64($item.Handle, 16))
    [void][Park]::Move($h, [int]$item.X, [int]$item.Y, [int]$item.W, [int]$item.H)
  }
  Write-Output "restored $($saved.Count) window position(s)"
}
