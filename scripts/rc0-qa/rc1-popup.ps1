# Dump every visible #32768 popup menu on the desktop with its text, owning PID,
# and rectangle. Diagnostic for the tray-menu inspection path.
param(
  [switch]$Open,
  [string]$ClickText = '',
  [int]$ClickRow = -1
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Popup {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuStringW(IntPtr menu, uint item, StringBuilder text, int max, uint flags);
  [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr menu, int item);
  [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr menu, int item);
  [DllImport("user32.dll")] public static extern bool GetMenuItemRect(IntPtr hwnd, IntPtr menu, uint item, out RECT rect);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT rect);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  const uint MF_BYPOSITION = 0x400;
  public static bool DpiAware() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  static public List<string> Lines = new List<string>();
  public static List<IntPtr> Popups() {
    var found = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      if (Cls(h) == "#32768" && IsWindowVisible(h)) found.Add(h);
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static void Describe(IntPtr h) {
    RECT wr; GetWindowRect(h, out wr);
    Lines.Add("POPUP 0x" + h.ToInt64().ToString("X") + " pid=" + Pid(h) + " rect=(" + wr.L + "," + wr.T + ") " + (wr.R - wr.L) + "x" + (wr.B - wr.T));
    IntPtr menu = GetMenu(h);
    Lines.Add("  menu=0x" + menu.ToInt64().ToString("X") + " count=" + GetMenuItemCount(menu));
    Append(menu, "  ");
  }
  static void Append(IntPtr menu, string indent) {
    int count = GetMenuItemCount(menu);
    for (int i = 0; i < count; i++) {
      var sb = new StringBuilder(512);
      GetMenuStringW(menu, (uint)i, sb, sb.Capacity, MF_BYPOSITION);
      IntPtr sub = GetSubMenu(menu, i);
      Lines.Add(indent + "row " + i + " id=" + GetMenuItemID(menu, i) + " sub=" + (sub != IntPtr.Zero) + " text='" + sb.ToString() + "'");
      if (sub != IntPtr.Zero) Append(sub, indent + "    ");
    }
  }
  public static string ClickRow(IntPtr h, IntPtr menu, int row) {
    RECT r;
    if (!GetMenuItemRect(h, menu, (uint)row, out r)) return "no rect";
    int cx = (r.L + r.R) / 2, cy = (r.T + r.B) / 2;
    SetCursorPos(cx, cy);
    System.Threading.Thread.Sleep(200);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(90);
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    return "clicked row " + row + " at " + cx + "," + cy;
  }
  public static string ClickText(IntPtr h, IntPtr menu, string wanted) {
    int count = GetMenuItemCount(menu);
    for (int i = 0; i < count; i++) {
      var sb = new StringBuilder(512);
      GetMenuStringW(menu, (uint)i, sb, sb.Capacity, MF_BYPOSITION);
      if (sb.ToString().Replace("&", "") == wanted) return ClickRow(h, menu, i);
    }
    return "text not found: " + wanted;
  }
}
"@
[void][Popup]::DpiAware()

if ($Open) {
  & (Join-Path $PSScriptRoot 'tray-menu-drive.ps1') -Open | Out-Null
  Start-Sleep -Milliseconds 1200
}

$popups = [Popup]::Popups()
Write-Output "visible #32768 popups: $($popups.Count)"
foreach ($h in $popups) {
  [void][Popup]::Describe($h)
  foreach ($line in [Popup]::Lines) { Write-Output $line }
  [Popup]::Lines.Clear()
  if ($ClickText) {
    $menu = [Popup]::GetMenu($h)
    Write-Output ("  -> " + [Popup]::ClickText($h, $menu, $ClickText))
  } elseif ($ClickRow -ge 0) {
    $menu = [Popup]::GetMenu($h)
    Write-Output ("  -> " + [Popup]::ClickRow($h, $menu, $ClickRow))
  }
}
