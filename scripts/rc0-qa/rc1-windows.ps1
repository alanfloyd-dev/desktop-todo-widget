# List every top-level and desktop-child window owned by the product process.
#
# `rc0-window-probe.ps1` only accepts class 'Tauri Window', so when a mode or a
# shell restyle changes that class the probe cannot see the window at all. This
# enumerator reports the real class, style, rect and taskbar eligibility of every
# window the process owns, which is what the taskbar/Alt+Tab checks need.
param(
  [string]$ProcessName = 'alan-desktop',
  [switch]$AllProcesses,
  # Keep sampling for N seconds and report a taskbar-eligibility verdict, so a
  # mode cycle can be checked for a transient taskbar button.
  [int]$WatchSeconds = 0,
  [int]$IntervalMs = 60,
  [string]$Label = ''
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Win2 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int i);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static string Txt(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1}) {2}x{3}", r.L, r.T, r.Rr - r.L, r.B - r.T); }
  public static long Ex(IntPtr h) { return GetWindowLongPtrW(h, -20).ToInt64(); }
  public static long St(IntPtr h) { return GetWindowLongPtrW(h, -16).ToInt64(); }
  public static IntPtr Owner(IntPtr h) { return GetWindow(h, 4); } // GW_OWNER
}
"@
[void][Win2]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$targets = if ($AllProcesses) { $null } else { @((Get-Process -Name $ProcessName -ErrorAction Stop).Id) }
Write-Output "filter pids: $(if ($targets) { $targets -join ',' } else { '<all>' })"

$rows = @()
foreach ($h in [Win2]::All()) {
  $pid0 = [Win2]::Pid($h)
  if ($targets -and ($targets -notcontains $pid0)) { continue }
  $ex = [Win2]::Ex($h)
  $st = [Win2]::St($h)
  $tool = (($ex -band 0x00000080) -ne 0)
  $app = (($ex -band 0x00040000) -ne 0)
  $child = (($st -band 0x40000000) -ne 0)
  $owner = [Win2]::Owner($h)
  $parent = [Win2]::GetParent($h)
  $visible = [Win2]::IsWindowVisible($h)
  # Windows' own taskbar rule: a visible, unowned top-level window that is not a
  # tool window gets a button; WS_EX_APPWINDOW forces one.
  $eligible = $visible -and (-not $child) -and ($owner -eq [IntPtr]::Zero) -and ((-not $tool) -or $app)
  $rows += [pscustomobject]@{
    hwnd    = ('0x{0:X}' -f $h.ToInt64())
    pid     = $pid0
    class   = [Win2]::Cls($h)
    title   = [Win2]::Txt($h)
    visible = $visible
    st      = ('0x{0:X8}' -f $st)
    ex      = ('0x{0:X8}' -f $ex)
    toolwin = $tool
    appwin  = $app
    child   = $child
    owner   = ('0x{0:X}' -f $owner.ToInt64())
    parent  = ('0x{0:X}' -f $parent.ToInt64())
    taskbar = $eligible
    dpi     = [Win2]::GetDpiForWindow($h)
    rect    = [Win2]::Rect($h)
  }
}
$rows | Format-Table -AutoSize | Out-String -Width 250 | Write-Output
Write-Output "window count: $($rows.Count)"

if ($WatchSeconds -gt 0) {
  Write-Output "=== watching ${WatchSeconds}s at ${IntervalMs}ms for a taskbar button ==="
  $deadline = (Get-Date).AddSeconds($WatchSeconds)
  $samples = 0
  $violations = 0
  $seen = @{}
  while ((Get-Date) -lt $deadline) {
    $samples++
    # Re-enumerate every sample: the window is reparented across mode changes, so
    # a cached handle list would silently stop observing the real widget.
    foreach ($h in [Win2]::All()) {
      $pid0 = [Win2]::Pid($h)
      if ($targets -and ($targets -notcontains $pid0)) { continue }
      $ex = [Win2]::Ex($h)
      $st = [Win2]::St($h)
      $tool = (($ex -band 0x00000080) -ne 0)
      $app = (($ex -band 0x00040000) -ne 0)
      $child = (($st -band 0x40000000) -ne 0)
      $owner = [Win2]::Owner($h)
      $visible = [Win2]::IsWindowVisible($h)
      $eligible = $visible -and (-not $child) -and ($owner -eq [IntPtr]::Zero) -and ((-not $tool) -or $app)
      $cls = [Win2]::Cls($h)
      if ($eligible) {
        $violations++
        Write-Output ("VIOLATION {0} class={1} ex=0x{2:X8} st=0x{3:X8} rect={4}" -f (Get-Date -Format 'HH:mm:ss.fff'), $cls, $ex, $st, [Win2]::Rect($h))
      }
      if ($cls -notin @('Tauri Window', 'TAURI_DRAG_RESIZE_BORDERS', 'WRY_WEBVIEW', 'Chrome_WidgetWin_0')) { continue }
      $key = '{0}|{1}|0x{2:X8}|{3}' -f $cls, [Win2]::Rect($h), $ex, $visible
      if (-not $seen.ContainsKey($key)) {
        $seen[$key] = $true
        Write-Output ("[{0}] {1} class={2} visible={3} rect={4} dpi={5} ex=0x{6:X8} taskbarEligible={7}" -f `
          (Get-Date -Format 'HH:mm:ss.fff'), $Label, $cls, $visible, [Win2]::Rect($h), [Win2]::GetDpiForWindow($h), $ex, $eligible)
      }
    }
    Start-Sleep -Milliseconds $IntervalMs
  }
  Write-Output "watch samples=$samples distinctStates=$($seen.Count) taskbarViolations=$violations"
  if ($violations -eq 0) { Write-Output "PASS: no sample ever put the product window on the taskbar" }
}
