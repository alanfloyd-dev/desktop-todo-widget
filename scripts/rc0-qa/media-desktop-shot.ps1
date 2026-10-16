# Re-capture the Desktop-mode still: real Desktop mode, real translucent fallback,
# wallpaper and desktop icons left exactly as they are.
#
# Three things make this different from a plain capture:
#
#   1. The widget is temporarily sized to fit the work area. The user's saved
#      Desktop height (1067 DIP) is taller than the work area (1019 DIP), so the
#      task list falls below the screen and a crop of the window rect shows only
#      empty material.
#   2. The operator's own windows are parked outside the virtual screen for the
#      frame, inside this process, because a shell command of its own brings the
#      terminal back. Nothing here hides SHELLDLL_DefView.
#   3. The page is nudged and the pointer moved over the widget. A desktop-hosted
#      child is treated as occluded by Chromium until it actually repaints, and an
#      un-repainted frame composites as bare material with no content.
#
# The saved Desktop geometry is snapshotted first and restored at the end, then
# verified field by field.
param(
  [string]$Out = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'docs\assets\desktop.png'),
  [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
  [string]$Exe = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'src-tauri\target\release\alan-desktop.exe'),
  [string]$ParkPid = '23484',
  # Capture size, in DIP, inside a 1019-DIP-tall work area.
  [int]$DipWidth = 400,
  [int]$DipHeight = 660
)

$ErrorActionPreference = 'Stop'
$capture = Join-Path $Root 'scripts\rc0-qa\media-capture.ps1'
$settings = Join-Path $Root '.rc0-evidence\rc1-settings.py'
$geometry = Join-Path $Root '.rc0-evidence\media-desktop-geometry.py'

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Shot {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static bool Dpi() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static int[] Snapshot(IntPtr h) { R r; GetWindowRect(h, out r); return new int[] { r.L, r.T, r.Rr-r.L, r.B-r.T }; }
  public static bool Move(IntPtr h, int x, int y, int w, int hh) { return SetWindowPos(h, IntPtr.Zero, x, y, w, hh, 0x0004 | 0x0010); }
  // Largest visible top-level windows of the process, biggest first.
  public static List<IntPtr> Frames(uint pid) {
    var found = new List<IntPtr>(); var sizes = new List<int>();
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      uint q; GetWindowThreadProcessId(h, out q);
      if (q != pid || !IsWindowVisible(h)) return true;
      R r; GetWindowRect(h, out r);
      found.Add(h); sizes.Add((r.Rr - r.L) * (r.B - r.T));
      return true;
    }, IntPtr.Zero);
    var order = new List<int>(); for (int i = 0; i < found.Count; i++) order.Add(i);
    order.Sort((a, b) => sizes[b].CompareTo(sizes[a]));
    var result = new List<IntPtr>(); foreach (var i in order) result.Add(found[i]);
    return result;
  }
}
"@
[void][Shot]::Dpi()

# --- 1. snapshot the user's Desktop geometry ---------------------------------
& python $geometry snapshot | ForEach-Object { $_ }

# --- 2. temporary capture geometry ------------------------------------------
& python $settings set "desktopX=520" "desktopY=40" "desktopWidth=$DipWidth" "desktopHeight=$DipHeight" | Out-Null
Write-Output "capture geometry: 520,40 ${DipWidth}x${DipHeight} DIP"

# --- 3. launch Desktop mode --------------------------------------------------
Get-Process alan-desktop, desktop-todo-widget -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 3
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9340'
Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
Start-Sleep -Seconds 18

$parked = @()
try {
  # --- 4. park the operator's windows off the virtual screen -----------------
  $offset = 2600
  foreach ($h in [Shot]::Frames([uint32]$ParkPid)) {
    $rect = [Shot]::Snapshot($h)
    if ($rect[2] -lt 300 -or $rect[3] -lt 200) { continue }
    $parked += [pscustomobject]@{ H = $h; X = $rect[0]; Y = $rect[1]; W = $rect[2]; Ht = $rect[3] }
    [void][Shot]::Move($h, $offset, 40, $rect[2], $rect[3])
    $offset += 80
  }
  Write-Output "parked $($parked.Count) occluding window(s)"
  Start-Sleep -Milliseconds 1500

  # --- 5. let the rect settle ------------------------------------------------
  $stable = $null
  for ($i = 0; $i -lt 20; $i++) {
    $probe = & powershell -ExecutionPolicy Bypass -File $capture -Action probe 2>&1 | Out-String
    if ($probe -match 'Rect\s+:\s+(\d+),(\d+)\s+(\d+)x(\d+)') {
      $candidate = "$($matches[1]),$($matches[2]) $($matches[3])x$($matches[4])"
      if ($candidate -eq $stable) { break }
      $stable = $candidate
    }
    Start-Sleep -Milliseconds 700
  }
  Write-Output "settled widget rect: $stable"

  # --- 6. nudge the page, then point the pointer at the widget --------------
  $nudgePath = Join-Path $env:TEMP 'dtw-nudge2.mjs'
  $nudge = @'
const port = 9340;
const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = list.find(t => t.type === 'page' && t.url.includes('tauri.localhost'));
if (!page) { console.log('no target'); process.exit(0); }
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise(r => ws.addEventListener('open', r));
let id = 0;
const send = (method, params) => new Promise(res => {
  const mid = ++id;
  const on = (e) => { const m = JSON.parse(e.data); if (m.id === mid) { ws.removeEventListener('message', on); res(m.result); } };
  ws.addEventListener('message', on);
  ws.send(JSON.stringify({ id: mid, method, params }));
});
const r = await send('Runtime.evaluate', { expression: `(() => {
  const shell = document.querySelector('.app-shell');
  if (shell) { shell.style.opacity = '0.999'; void shell.offsetHeight; shell.style.opacity = ''; }
  window.dispatchEvent(new Event('resize'));
  return JSON.stringify({ cls: document.querySelector('.app-shell')?.className,
    view: window.innerWidth + 'x' + window.innerHeight,
    rows: document.querySelectorAll('.pending-row').length });
})()`, returnByValue: true });
console.log('page:', r?.result?.value);
ws.close();
'@
  Set-Content -Path $nudgePath -Value $nudge -Encoding UTF8
  & node $nudgePath
  Start-Sleep -Seconds 2

  if ($stable -match '(\d+),(\d+)\s+(\d+)x(\d+)') {
    $wx = [int]$matches[1]; $wy = [int]$matches[2]; $ww = [int]$matches[3]; $wh = [int]$matches[4]
    $cx = $wx + [int]($ww / 2)
    $cy = $wy + [Math]::Min(200, [int]($wh / 4))
    Write-Output "hovering the widget at $cx,$cy (no click: no data change)"
    [void][Shot]::SetCursorPos($cx, $cy)
    Start-Sleep -Seconds 3
  }

  # --- 7. capture ------------------------------------------------------------
  & powershell -ExecutionPolicy Bypass -File $capture -Action capture -Out $Out -NoFocus
}
finally {
  # --- 8. restore the user's geometry, then verify it ------------------------
  Start-Sleep -Milliseconds 300
  foreach ($item in $parked) {
    [void][Shot]::Move($item.H, [int]$item.X, [int]$item.Y, [int]$item.W, [int]$item.Ht)
  }
  Write-Output "restored $($parked.Count) window position(s)"
  Get-Process alan-desktop, desktop-todo-widget -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
  & python $geometry restore | ForEach-Object { $_ }
  Write-Output "verified geometry:"
  & python $geometry show | ForEach-Object { $_ }
}
