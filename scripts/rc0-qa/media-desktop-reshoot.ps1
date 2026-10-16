# Re-capture the Desktop-mode still with the desktop icon labels hidden for the
# frame only.
#
# Everything happens inside this one process, for two reasons:
#   * the operator's occluding windows reappear whenever a shell command runs, so a
#     hide in one step and a capture in the next always loses the race;
#   * the desktop icon view must be restored even if the capture throws, which the
#     `finally` block guarantees and verifies.
#
# The wallpaper, the taskbar and the product widget are untouched. Desktop mode
# keeps its real translucent Graphite fallback: nothing here attaches Acrylic.
param(
  [string]$Out = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'docs\assets\desktop.png'),
  [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
  [string]$Exe = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'src-tauri\target\release\alan-desktop.exe'),
  [string]$ParkPid = '23484'
)

$ErrorActionPreference = 'Stop'
$capture = Join-Path $Root 'scripts\rc0-qa\media-capture.ps1'
$icons = Join-Path $Root 'scripts\rc0-qa\media-desktop-icons.ps1'
$settings = Join-Path $Root '.rc0-evidence\rc1-settings.py'

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Cap3 {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, E cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static int[] Snapshot(IntPtr h) { R r; GetWindowRect(h, out r); return new int[] { r.L, r.T, r.Rr-r.L, r.B-r.T }; }
  public static bool Move(IntPtr h, int x, int y, int w, int hh) { return SetWindowPos(h, IntPtr.Zero, x, y, w, hh, 0x0004 | 0x0010); }
  public static IntPtr FindClass(string name) {
    IntPtr result = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr p) { if (Cls(h) == name) { result = h; return false; } return true; }, IntPtr.Zero);
    return result;
  }
  // Descendants of the given root HWND with the requested class name.
  public static List<IntPtr> ChildrenOfClass(IntPtr root, string name) {
    var found = new List<IntPtr>();
    if (root == IntPtr.Zero) return found;
    EnumChildWindows(root, delegate(IntPtr h, IntPtr p) {
      if (Cls(h) == name && !found.Contains(h)) found.Add(h);
      return true;
    }, IntPtr.Zero);
    return found;
  }
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

# --- Desktop mode, fresh start ---
& python $settings set mode=desktop floatingPresentation=expanded | Out-Null
Get-Process alan-desktop, desktop-todo-widget -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 3
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9340'
Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
Start-Sleep -Seconds 16
& powershell -ExecutionPolicy Bypass -File $capture -Action focus | Out-Null

$iconViews = @()
$parked = @()
$iconStateBefore = @{}
try {
  # Record the icon view state so the restore can be verified against it.
  # `SHELLDLL_DefView` is a descendant of Progman (verified by media-desktop-icons.ps1),
  # so the lookup has to walk Progman's children rather than top-level windows.
  $progman = [Cap3]::FindClass('Progman')
  Write-Output "Progman hwnd: 0x$($progman.ToInt64().ToString('X'))"
  $iconViews = [System.Collections.Generic.List[IntPtr]]::new()
  foreach ($h in [Cap3]::ChildrenOfClass($progman, 'SHELLDLL_DefView')) { $iconViews.Add($h) }
  Write-Output "desktop icon views found: $($iconViews.Count)"
  if ($iconViews.Count -eq 0) { throw 'no SHELLDLL_DefView found; refusing to capture with unknown icon state' }
  foreach ($h in $iconViews) {
    $iconStateBefore[$h.ToInt64()] = [Cap3]::IsWindowVisible($h)
    Write-Output ("  before 0x{0:X} visible={1}" -f $h.ToInt64(), $iconStateBefore[$h.ToInt64()])
  }

  # Park the operator's own windows outside the virtual screen.
  $offset = 2600
  foreach ($h in [Cap3]::Frames([uint32]$ParkPid)) {
    $rect = [Cap3]::Snapshot($h)
    if ($rect[2] -lt 300 -or $rect[3] -lt 200) { continue }
    $parked += [pscustomobject]@{ H = $h; X = $rect[0]; Y = $rect[1]; W = $rect[2]; Ht = $rect[3] }
    [void][Cap3]::Move($h, $offset, 40, $rect[2], $rect[3])
    $offset += 80
  }
  Write-Output "parked $($parked.Count) occluding window(s)"
  Start-Sleep -Milliseconds 1200

  # Hide the desktop icons (labels only; wallpaper and widget stay visible).
  foreach ($h in $iconViews) {
    [void][Cap3]::ShowWindow($h, 0)
  }
  Start-Sleep -Milliseconds 1400
  foreach ($h in $iconViews) {
    Write-Output ("  hidden 0x{0:X} visible={1}" -f $h.ToInt64(), [Cap3]::IsWindowVisible($h))
  }

  & powershell -ExecutionPolicy Bypass -File $capture -Action focus | Out-Null
  Start-Sleep -Milliseconds 800

  # Read the widget rect, wait for it to settle, then capture. The Desktop widget is
  # reparented under SHELLDLL_DefView during startup, so its rect can still be the
  # transient pre-attach one for a moment after the window first appears.
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

  # Nudge the page so the compositor has definitely painted. In Desktop mode the
  # widget is a desktop child and Chromium treats it as occluded by default, so the
  # first frames can be empty until the page actually repaints; a real interaction
  # (or this equivalent style invalidation) is what makes the content appear.
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
await send('Runtime.evaluate', { expression: `(() => {
  const shell = document.querySelector('.app-shell');
  if (shell) { shell.style.opacity = '0.999'; void shell.offsetHeight; shell.style.opacity = ''; }
  window.dispatchEvent(new Event('resize'));
  return document.querySelector('.app-shell')?.className ?? 'none';
})()`, returnByValue: true });
console.log('paint nudge sent');
ws.close();
'@
  $nudgePath = Join-Path $env:TEMP 'dtw-nudge.mjs'
  Set-Content -Path $nudgePath -Value $nudge -Encoding UTF8
  & node $nudgePath
  Start-Sleep -Seconds 2

  # Re-activate the widget last: the Desktop child loses activation when the shell
  # or a parked-away window changes foreground, and an inactive desktop child is not
  # composited. Focusing after everything else is what makes the frame show content.
  & powershell -ExecutionPolicy Bypass -File $capture -Action focus | ForEach-Object { $_ }
  Start-Sleep -Seconds 3

  & powershell -ExecutionPolicy Bypass -File $capture -Action capture -Out $Out -NoFocus

  # Also dump the whole desktop while the icons are hidden, so the real layout can be
  # inspected even if the widget rect is not where the probe reported it.
  $full = [System.IO.Path]::ChangeExtension($Out, '.full.png')
  Add-Type -AssemblyName System.Drawing
  Add-Type -AssemblyName System.Windows.Forms
  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $gfx.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bmp.Size)
  $gfx.Dispose()
  $bmp.Save($full, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Write-Output "saved full-desktop reference $full"
}
finally {
  Start-Sleep -Milliseconds 300
  # 1) restore the desktop icons, 2) restore the parked windows, 3) verify.
  foreach ($h in $iconViews) {
    [void][Cap3]::ShowWindow($h, 5)
  }
  Start-Sleep -Milliseconds 800
  $ok = $true
  foreach ($h in $iconViews) {
    $now = [Cap3]::IsWindowVisible($h)
    $was = [bool]$iconStateBefore[$h.ToInt64()]
    Write-Output ("  restore 0x{0:X} was={1} now={2}" -f $h.ToInt64(), $was, $now)
    if ($was -ne $now) { $ok = $false }
  }
  foreach ($item in $parked) {
    [void][Cap3]::Move($item.H, [int]$item.X, [int]$item.Y, [int]$item.W, [int]$item.Ht)
  }
  Write-Output "restored $($parked.Count) window position(s)"
  if ($ok) {
    Write-Output "DESKTOP ICON RESTORE VERIFIED: visibility matches the pre-capture state"
  } else {
    Write-Output "DESKTOP ICON RESTORE MISMATCH"
  }
}
