# Inspect the Windows desktop icon view, and hide/show it for a capture.
#
# The widget in Desktop mode is a child of `SHELLDLL_DefView`, so the same host the
# product attaches to also owns the desktop icons. Hiding that view removes the icon
# labels (which carry the user's folder/app names) while leaving the wallpaper, the
# taskbar and the product widget untouched — unlike toggling the Explorer "show
# desktop icons" setting, which mutates user configuration.
param(
  [Parameter(Mandatory = $true)][ValidateSet('probe', 'hide', 'show')][string]$Action
)

$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Desk {
  public delegate bool E(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, E cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L,T,Rr,B; }
  public static bool Dpi() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static string Txt(IntPtr h) { var sb = new StringBuilder(300); GetWindowTextW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1}) {2}x{3}", r.L, r.T, r.Rr-r.L, r.B-r.T); }

  /// Every `SHELLDLL_DefView` on the desktop, as raw values.
  public static List<IntPtr> DefViews() {
    var found = new List<IntPtr>();
    var all = new List<IntPtr>();
    // Both Progman's direct child and any WorkerW-hosted copy.
    IntPtr progman = FindClass("Progman");
    if (progman != IntPtr.Zero) Collect(progman, found);
    EnumWindows(delegate(IntPtr h, IntPtr p) { all.Add(h); return true; }, IntPtr.Zero);
    foreach (var h in all) Collect(h, found);
    return found;
  }
  static void Collect(IntPtr root, List<IntPtr> found) {
    EnumChildWindows(root, delegate(IntPtr h, IntPtr p) {
      if (Cls(h) == "SHELLDLL_DefView" && !found.Contains(h)) found.Add(h);
      return true;
    }, IntPtr.Zero);
  }
  public static IntPtr FindClass(string name) {
    IntPtr result = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr p) { if (Cls(h) == name) { result = h; return false; } return true; }, IntPtr.Zero);
    return result;
  }
  public static string SetVisible(IntPtr h, bool visible) {
    bool before = IsWindowVisible(h);
    ShowWindow(h, visible ? 5 : 0);
    System.Threading.Thread.Sleep(400);
    return "was=" + before + " now=" + IsWindowVisible(h) + " rect=" + Rect(h);
  }
}
"@
[void][Desk]::Dpi()

$views = [Desk]::DefViews()
Write-Output "SHELLDLL_DefView count: $($views.Count)"
foreach ($h in $views) {
  Write-Output ("  0x{0:X} pid={1} visible={2} rect={3}" -f $h.ToInt64(), [Desk]::Pid($h), [Desk]::IsWindowVisible($h), [Desk]::Rect($h))
}

if ($Action -eq 'hide') {
  foreach ($h in $views) { Write-Output ("hide 0x{0:X} -> {1}" -f $h.ToInt64(), [Desk]::SetVisible($h, $false)) }
} elseif ($Action -eq 'show') {
  foreach ($h in $views) { Write-Output ("show 0x{0:X} -> {1}" -f $h.ToInt64(), [Desk]::SetVisible($h, $true)) }
}
