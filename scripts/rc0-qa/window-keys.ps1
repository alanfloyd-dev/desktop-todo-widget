# Send keyboard input to the product window for Quick Links QA.
#
# The window is a WebView2 surface with no UI Automation element for its DOM, so
# focused-element automation is not available. This drives the real window with
# Windows input messages instead: it activates the product window, then sends real
# key events, which is what a user's keyboard does.
#
#   .\window-keys.ps1 -Keys "TAB,TAB,ENTER"
#   .\window-keys.ps1 -Text "Docs" -Keys "TAB"
param(
  [string]$ProcessName = "alan-desktop",
  [string]$Keys = "",
  [string]$Text = "",
  # Absolute screen coordinates for a real left click on the product window.
  [int]$ClickX = -1,
  [int]$ClickY = -1,
  # Mouse wheel notches to send first; negative scrolls up, positive down.
  [int]$Wheel = 0,
  # Milliseconds to wait after activating the window before sending anything.
  [int]$SettleMs = 350
)

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Wk {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern short VkKeyScanW(char ch);
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> All() {
    var l = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static List<IntPtr> Kids(IntPtr parent) {
    var l = new List<IntPtr>();
    EnumChildWindows(parent, delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  const uint KEYEVENTF_KEYUP = 0x0002;
  const uint KEYEVENTF_UNICODE = 0x0004;

  static void Tap(byte vk, bool extended) {
    keybd_event(vk, 0, extended ? 1u : 0u, UIntPtr.Zero);
    System.Threading.Thread.Sleep(28);
    keybd_event(vk, 0, KEYEVENTF_KEYUP | (extended ? 1u : 0u), UIntPtr.Zero);
    System.Threading.Thread.Sleep(45);
  }
  public static void Named(string name) {
    switch (name.ToUpperInvariant()) {
      case "TAB": Tap(0x09, false); break;
      case "ENTER": Tap(0x0D, false); break;
      case "ESC": Tap(0x1B, false); break;
      case "SPACE": Tap(0x20, false); break;
      case "UP": Tap(0x26, true); break;
      case "DOWN": Tap(0x28, true); break;
      case "LEFT": Tap(0x25, true); break;
      case "RIGHT": Tap(0x27, true); break;
      case "BACKSPACE": Tap(0x08, false); break;
      case "DELETE": Tap(0x2E, true); break;
      case "HOME": Tap(0x24, true); break;
      case "END": Tap(0x23, true); break;
      case "SHIFTTAB":
        keybd_event(0x10, 0, 0, UIntPtr.Zero);
        Tap(0x09, false);
        keybd_event(0x10, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        System.Threading.Thread.Sleep(45);
        break;
      case "CTRLA":
        keybd_event(0x11, 0, 0, UIntPtr.Zero);
        Tap(0x41, false);
        keybd_event(0x11, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        System.Threading.Thread.Sleep(45);
        break;
      default: throw new ArgumentException("unsupported key " + name);
    }
  }
  // Unicode text entry: independent of the active keyboard layout, which matters
  // because the UI may be rendering in a language the layout does not have.
  public static void Type(string text) {
    foreach (char ch in text) {
      keybd_event(0, (byte)ch, KEYEVENTF_UNICODE, UIntPtr.Zero);
      System.Threading.Thread.Sleep(22);
      keybd_event(0, (byte)ch, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP, UIntPtr.Zero);
      System.Threading.Thread.Sleep(28);
    }
  }
  public static void ClickAt(int x, int y, int holdMs) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(180);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(holdMs);
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(500);
  }
  // Negative notches scroll up, positive scroll down.
  public static void Wheel(int notches) {
    uint flag = 0x0800; // MOUSEEVENTF_WHEEL
    int steps = Math.Abs(notches);
    int delta = notches < 0 ? 120 : -120;
    for (int i = 0; i < steps; i++) {
      mouse_event(flag, 0, 0, (uint)delta, UIntPtr.Zero);
      System.Threading.Thread.Sleep(90);
    }
    System.Threading.Thread.Sleep(300);
  }
}
"@

$ids = @((Get-Process -Name $ProcessName -ErrorAction Stop).Id)
$all = [Wk]::All()
$hwnd = [IntPtr]::Zero
foreach ($h in $all) {
  if ($ids -contains [Wk]::Pid($h) -and [Wk]::Cls($h) -eq 'Tauri Window') { $hwnd = $h; break }
}
if ($hwnd -eq [IntPtr]::Zero) {
  foreach ($h in $all) {
    if ($ids -notcontains [Wk]::Pid($h)) { continue }
    foreach ($c in [Wk]::Kids($h)) { if ([Wk]::Cls($c) -eq 'WRY_WEBVIEW') { $hwnd = $h; break } }
    if ($hwnd -ne [IntPtr]::Zero) { break }
  }
}
if ($hwnd -eq [IntPtr]::Zero) {
  # Say what was actually seen: a missing window and a class-name mismatch look
  # identical from the caller's side.
  $seen = @()
  foreach ($h in $all) { if ($ids -contains [Wk]::Pid($h)) { $seen += ("0x{0:X}:{1}" -f $h.ToInt64(), [Wk]::Cls($h)) } }
  throw "product window not found for $ProcessName (pids=$($ids -join ','); windows=$($seen -join ' '))"
}

# Focus the WebView itself so key events reach the page rather than the frame.
$webview = [IntPtr]::Zero
foreach ($c in [Wk]::Kids($hwnd)) { if ([Wk]::Cls($c) -eq 'WRY_WEBVIEW') { $webview = $c; break } }
$focusTarget = if ($webview -ne [IntPtr]::Zero) { $webview } else { $hwnd }

[void][Wk]::SetForegroundWindow($focusTarget)
Start-Sleep -Milliseconds $SettleMs
if ([Wk]::GetForegroundWindow() -ne $focusTarget -and [Wk]::GetForegroundWindow() -ne $hwnd) {
  Write-Host "WARNING: product window may not be foreground; keys could go elsewhere"
}

if ($Text) { [Wk]::Type($Text); Write-Host "typed: $Text" }
if ($Keys) {
  foreach ($key in ($Keys -split ',')) {
    $name = $key.Trim()
    if (-not $name) { continue }
    [Wk]::Named($name)
    Write-Host "sent: $name"
  }
}
if ($Wheel -ne 0) {
  [Wk]::Wheel($Wheel)
  Write-Host "scrolled: $Wheel notch(es)"
}
if ($ClickX -ge 0 -and $ClickY -ge 0) {
  [Wk]::ClickAt($ClickX, $ClickY, 60)
  Write-Host "clicked: $ClickX,$ClickY"
}
Start-Sleep -Milliseconds 250
