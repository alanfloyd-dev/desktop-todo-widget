# Read the product's native tray HMENU and activate a row by clicking it.
#
# The tray/context menu is a Win32 popup (`#32768`), which UI Automation does not
# expose as MenuItems, and the toggle item has no HMENU handle of its own. This
# reads the popup's real menu items with GetMenuStringW and activates a row by
# clicking the rectangle GetMenuItemRect reports - the same path a user's mouse
# takes, so product handlers run exactly as they do for a user.
param(
  [string]$ProcessName = 'alan-desktop',
  [switch]$Open,
  [string]$Invoke = '',
  [switch]$List
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class TrayMenu {
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
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }

  const uint MF_BYPOSITION = 0x400;
  const uint MF_GRAYED = 0x1;
  const uint MF_SEPARATOR = 0x800;
  const uint MIIM_STATE = 0x1;
  const uint MIIM_ID = 0x2;

  public static bool DpiAware() { return SetProcessDpiAwarenessContext(new IntPtr(-4)); }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }

  /** Every visible #32768 popup owned by the process, plus a text dump of each. */
  public static List<string> Describe(uint pid) {
    var output = new List<string>();
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      if (Pid(h) != pid) return true;
      if (Cls(h) != "#32768") return true;
      if (!IsWindowVisible(h)) return true;
      IntPtr menu = GetMenu(h);
      if (menu == IntPtr.Zero) return true;
      output.Add("WINDOW 0x" + h.ToInt64().ToString("X") + " menu=0x" + menu.ToInt64().ToString("X") + " count=" + GetMenuItemCount(menu));
      Append(output, menu, "  ");
      return true;
    }, IntPtr.Zero);
    return output;
  }

  static void Append(List<string> output, IntPtr menu, string indent) {
    int count = GetMenuItemCount(menu);
    for (int i = 0; i < count; i++) {
      var sb = new StringBuilder(512);
      GetMenuStringW(menu, (uint)i, sb, sb.Capacity, MF_BYPOSITION);
      uint id = GetMenuItemID(menu, i);
      IntPtr sub = GetSubMenu(menu, i);
      output.Add(indent + "row " + i + " id=" + id + " sub=" + (sub != IntPtr.Zero) + " text='" + sb.ToString() + "'");
      if (sub != IntPtr.Zero) Append(output, sub, indent + "    ");
    }
  }

  /** Click the row of the first visible popup whose text matches. */
  public static string Click(uint pid, string wanted) {
    string result = null;
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      if (result != null) return true;
      if (Pid(h) != pid) return true;
      if (Cls(h) != "#32768") return true;
      if (!IsWindowVisible(h)) return true;
      IntPtr menu = GetMenu(h);
      if (menu == IntPtr.Zero) return true;
      int count = GetMenuItemCount(menu);
      for (int i = 0; i < count; i++) {
        var sb = new StringBuilder(512);
        GetMenuStringW(menu, (uint)i, sb, sb.Capacity, MF_BYPOSITION);
        string text = sb.ToString().Replace("&", "");
        if (text != wanted) continue;
        RECT r;
        if (!GetMenuItemRect(h, menu, (uint)i, out r)) { result = "no rect for row " + i; return true; }
        int cx = (r.L + r.R) / 2;
        int cy = (r.T + r.B) / 2;
        SetCursorPos(cx, cy);
        System.Threading.Thread.Sleep(200);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(90);
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
        result = "clicked row " + i + " '" + wanted + "' at " + cx + "," + cy;
        return true;
      }
      return true;
    }, IntPtr.Zero);
    return result ?? "row not found: " + wanted;
  }
}
"@
[void][TrayMenu]::DpiAware()

$pids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)

if ($Open) {
  & (Join-Path $PSScriptRoot 'tray-menu-drive.ps1') -Open | Out-Null
  Start-Sleep -Milliseconds 1000
}

if ($List -or -not $Invoke) {
  foreach ($pid0 in $pids) {
    foreach ($line in [TrayMenu]::Describe([uint32]$pid0)) { Write-Output $line }
  }
}

if ($Invoke) {
  $done = $false
  foreach ($pid0 in $pids) {
    $result = [TrayMenu]::Click([uint32]$pid0, $Invoke)
    Write-Output "pid $pid0 -> $result"
    if ($result -like 'clicked*') { $done = $true; break }
  }
  if (-not $done) { throw "could not activate tray row '$Invoke'" }
}
