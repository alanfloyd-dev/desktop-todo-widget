# Select an item from an open native product menu (tray or the Orb's context
# menu - same menu, same handler).
#
# Mouse-driven on purpose: the popup's TrackPopupMenu loop ignores posted keys in
# this environment, while hover opens submenus and a real click on the popup (our
# own process's window) activates the item.
#
# Top-level rows are resolved from the live HMENU item texts, so the menu can gain
# or lose entries (identity heading, Show widget, GitHub link) without silently
# clicking the wrong command. Pass -List to only print the map.
#
# This file must stay pure ASCII: Windows PowerShell 5.1 reads a BOM-less UTF-8
# script as ANSI, and a multi-byte character inside the Add-Type here-string
# truncates the generated C#.
param(
  [string]$ProcessName = "alan-desktop",
  [ValidateSet('orb', 'already-open')]
  [string]$OpenVia = 'already-open',
  # Row index inside the Window mode submenu: 0 = Sidebar, 1 = Floating, 2 = Desktop.
  [int]$SubmenuRow = 0,
  # Click this top-level row instead of a submenu. -1 resolves it by the text in
  # -TopItem; when that is empty as well, the OwnerRow hover path is used.
  [int]$TopRow = -1,
  # Top-level item text to click (substring match, so the check mark is ignored).
  [string]$TopItem = "",
  # Which top-level row owns the submenu to hover.
  [int]$OwnerRow = 0,
  # Print the resolved row map and do not click anything.
  [switch]$List
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Sel {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(P p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern int GetMenuStringW(IntPtr hMenu, uint id, StringBuilder text, int max, uint flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuItemCount(IntPtr hMenu);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern uint GetMenuItemID(IntPtr hMenu, int pos);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern uint GetMenuState(IntPtr hMenu, uint id, uint flags);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetMenuItemRect(IntPtr hwnd, IntPtr hMenu, uint item, out R r);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  const uint MF_BYPOSITION = 0x00000400;
  const uint MF_SEPARATOR = 0x00000800;
  const uint MF_POPUP = 0x00000010;
  const uint MN_GETHMENU = 0x01E1;

  public static List<IntPtr> All() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static List<IntPtr> Children(IntPtr parent) {
    var l = new List<IntPtr>();
    EnumChildWindows(parent, delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1})-({2},{3})", r.L, r.T, r.Rr, r.B); }
  public static List<IntPtr> Menus() {
    var seen = new List<IntPtr>();
    foreach (IntPtr h in All()) { if (Cls(h) == "#32768" && IsWindowVisible(h) && !seen.Contains(h)) seen.Add(h); }
    return seen;
  }

  // The popup's HMENU handle is published through MN_GETHMENU, so items can be
  // read without any cross-process memory access.
  public static IntPtr Menu(IntPtr popup) { return SendMessageW(popup, MN_GETHMENU, IntPtr.Zero, IntPtr.Zero); }

  // Real screen rect of one menu item; the only reliable row target.
  public static bool MenuItemRect(IntPtr hwnd, IntPtr hMenu, int item, out R rect) {
    return GetMenuItemRect(hwnd, hMenu, (uint)item, out rect);
  }

  // "row=N id=I popup=B separator=S text='...'" per top-level row. The row index
  // counts every row, separators included, which is how the click helper targets
  // a pixel row.
  public static List<string> Rows(IntPtr popup) {
    var rows = new List<string>();
    IntPtr menu = Menu(popup);
    if (menu == IntPtr.Zero) { rows.Add("MN_GETHMENU unavailable"); return rows; }
    int count = GetMenuItemCount(menu);
    for (int i = 0; i < count; i++) {
      uint id = GetMenuItemID(menu, i);
      var sb = new StringBuilder(256);
      GetMenuStringW(menu, (uint)i, sb, sb.Capacity, MF_BYPOSITION);
      uint state = GetMenuState(menu, (uint)i, MF_BYPOSITION);
      rows.Add(string.Format("row={0} id={1} popup={2} separator={3} text='{4}'",
        i, id, (state & MF_POPUP) != 0, (state & MF_SEPARATOR) != 0, sb.ToString()));
    }
    return rows;
  }
}
"@
[void][Sel]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$product = [IntPtr]::Zero
foreach ($h in [Sel]::All()) {
  if ($ids -contains [Sel]::Pid($h) -and [Sel]::Cls($h) -eq 'Tauri Window') { $product = $h; break }
}
if ($product -eq [IntPtr]::Zero) {
  # Desktop mode reparents the product window below SHELLDLL_DefView, which
  # replaces its class, so fall back to "any window of the process that hosts a
  # WRY_WEBVIEW child"; that is true in every mode.
  foreach ($h in [Sel]::All()) {
    if ($ids -notcontains [Sel]::Pid($h)) { continue }
    foreach ($child in [Sel]::Children($h)) {
      if ([Sel]::Cls($child) -eq 'WRY_WEBVIEW') { $product = $h; break }
    }
    if ($product -ne [IntPtr]::Zero) { break }
  }
}
if ($product -eq [IntPtr]::Zero) { throw "product window not found (pids=$($ids -join ','))" }

if ($OpenVia -eq 'orb') {
  $wr = New-Object Sel+R; [void][Sel]::GetWindowRect($product, [ref]$wr)
  $cr = New-Object Sel+R; [void][Sel]::GetClientRect($product, [ref]$cr)
  [void][Sel]::SetCursorPos([int](($wr.L + $wr.Rr) / 2), [int](($wr.T + $wr.B) / 2))
  Start-Sleep -Milliseconds 300
  $cx = [int]($cr.Rr / 2); $cy = [int]($cr.B / 2)
  $lp = [IntPtr](([long]($cy -shl 16)) -bor ([long]($cx -band 0xFFFF)))
  [void][Sel]::PostMessageW($product, 0x0204, [IntPtr]2, $lp)
  Start-Sleep -Milliseconds 250
  [void][Sel]::PostMessageW($product, 0x0205, [IntPtr]0, $lp)
  Start-Sleep -Seconds 1
}

$menus = [Sel]::Menus()
if ($menus.Count -eq 0) { Write-Host 'no open menu'; return }
$mainHwnd = $menus[0]
$mr = New-Object Sel+R; [void][Sel]::GetWindowRect($mainHwnd, [ref]$mr)
Write-Host "main menu 0x$($mainHwnd.ToString('X')) rect=$([Sel]::Rect($mainHwnd))"

# The live row map: never guess how many rows the menu has.
$rows = [Sel]::Rows($mainHwnd)
$rowCount = $rows.Count
Write-Host "top-level rows: $rowCount"
foreach ($line in $rows) { Write-Host "  $line" }

function Row-OfItem([string]$needle, [object[]]$map) {
  for ($i = 0; $i -lt $map.Count; $i++) {
    if ($map[$i] -like "*text='*$needle*'") { return $i }
  }
  return -1
}

# Row metrics are measured from the live item rectangles, not assumed to be
# `height / rowCount`. `GetMenuItemRect` reports each item's real rect, which is
# the only way to target a row correctly once the menu contains separators and
# items of different heights - guessing here is how a click aimed at one row lands
# on the next one.
function Get-RowMetrics([IntPtr]$menu, [int]$row, [int]$left, [int]$top, [int]$height, [int]$rowCount) {
  $rect = New-Object Sel+R
  if ([Sel]::MenuItemRect([IntPtr]::Zero, $menu, $row, [ref]$rect)) {
    if ($rect.B -gt $rect.T) {
      return @{ X = [int](($rect.L + $rect.Rr) / 2); Y = [int](($rect.T + $rect.B) / 2); H = $rect.B - $rect.T }
    }
  }
  $rowH = [int]($height / [Math]::Max($rowCount, 1))
  return @{ X = $left + 60; Y = $top + 6 + $rowH * $row + [int]($rowH / 2); H = $rowH }
}

function Click-Row([IntPtr]$menu, [int]$left, [int]$top, [int]$height, [int]$row, [int]$rowCount, [string]$expectedText) {
  $metrics = Get-RowMetrics $menu $row $left $top $height $rowCount
  $x = $metrics.X
  $y = $metrics.Y

  # Safety: the click must land on a window this process owns AND, when the popup
  # publishes item rects, inside the requested row's rectangle. The menu also owns
  # `Quit`, so an off-by-one here would close the product instead of running the
  # requested command.
  $pt = New-Object Sel+P; $pt.X = $x; $pt.Y = $y
  $under = [Sel]::WindowFromPoint($pt)
  if (-not ($ids -contains [Sel]::Pid($under))) {
    Write-Host "SAFETY ABORT at ($x,$y) under=0x$($under.ToString('X')) $([Sel]::Cls($under))"
    return $false
  }
  $rect = New-Object Sel+R
  $haveRect = [Sel]::MenuItemRect([IntPtr]::Zero, $menu, $row, [ref]$rect)
  if ($haveRect -and ($y -lt $rect.T -or $y -gt $rect.B)) {
    Write-Host "SAFETY ABORT: resolved y=$y is outside row $row rect=($($rect.L),$($rect.T))-($($rect.Rr),$($rect.B))"
    return $false
  }
  if ($expectedText -and -not ($expectedText -like "*$($script:rowText[$row])*")) {
    Write-Host "SAFETY ABORT: row $row is '$($script:rowText[$row])', not '$expectedText'"
    return $false
  }
  Write-Host "clicking row $row '$($script:rowText[$row])' at ($x,$y)"
  [void][Sel]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 300
  [Sel]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 120
  [Sel]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
  return $true
}

# Row index -> item text, filled from the live menu so Click-Row can state exactly
# what it is about to activate.
function Set-RowText([object[]]$map) {
  $texts = @{}
  for ($i = 0; $i -lt $map.Count; $i++) {
    if ($map[$i] -match "text='(.*)'$") { $texts[$i] = $Matches[1] } else { $texts[$i] = '' }
  }
  return $texts
}

$script:rowText = Set-RowText $rows

if ($TopItem) {
  $TopRow = Row-OfItem $TopItem $rows
  if ($TopRow -lt 0) { Write-Host "no top-level item matching '$TopItem'"; return }
  Write-Host "resolved '$TopItem' to row $TopRow"
}

if ($TopRow -ge 0) {
  [void](Click-Row ([Sel]::Menu($mainHwnd)) $mr.L $mr.T ($mr.B - $mr.T) $TopRow $rowCount $null)
} else {
  # Hover the owner row until its submenu is on screen, then click SubmenuRow.
  $rowH = [int](($mr.B - $mr.T) / $rowCount)
  $sub = [IntPtr]::Zero
  foreach ($offset in 0..5) {
    $y = $mr.T + 6 + $rowH * $OwnerRow + [int]($rowH / 2) + ($offset * 3)
    [void][Sel]::SetCursorPos($mr.L + 60, $y)
    Start-Sleep -Milliseconds 1100
    foreach ($candidate in [Sel]::Menus()) {
      if ($candidate -ne $mainHwnd) { $sub = $candidate; break }
    }
    if ($sub -ne [IntPtr]::Zero) { Write-Host "submenu open (hover y=$y): $([Sel]::Rect($sub))"; break }
  }
  if ($sub -eq [IntPtr]::Zero) { Write-Host 'submenu never opened'; return }
  $sr = New-Object Sel+R; [void][Sel]::GetWindowRect($sub, [ref]$sr)
  $subRows = [Sel]::Rows($sub)
  Write-Host "submenu rows: $($subRows.Count)"
  foreach ($line in $subRows) { Write-Host "  $line" }
  $script:rowText = Set-RowText $subRows
  [void](Click-Row ([Sel]::Menu($sub)) $sr.L $sr.T ($sr.B - $sr.T) $SubmenuRow $subRows.Count $null)
}

Start-Sleep -Seconds 2
Write-Host "menus open after: $(([Sel]::Menus()).Count)"
Write-Host "product rect=$([Sel]::Rect($product))"
