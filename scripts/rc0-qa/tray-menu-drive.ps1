# Drive the product's tray menu for QA.
#
# The tray icon lives in the taskbar notification area (explorer.exe), so the
# click is a real mouse click on the taskbar - never over another application.
#
# The icon's accessible name / tooltip is the product identity
# (`desktop-todo-widget`), not the executable's `productName`.
param(
  [string]$Name = "desktop-todo-widget",
  [switch]$Open,
  [string]$Keys = "",
  # Park the cursor here before opening the menu. With `Invoke` (no mouse) the
  # popup appears at the cursor, which keeps the whole menu clear of the overflow
  # flyout that would otherwise cover its lower rows.
  [string]$CursorAt = ""
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Tray {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", SetLastError=true)] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  public static void Key(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(70); keybd_event(vk, 0, 2, UIntPtr.Zero); System.Threading.Thread.Sleep(160); }
}
"@
[void][Tray]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

function Find-TrayIcon([string]$ElementName) {
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $cond = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::NameProperty, $ElementName)
  $found = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $cond)
  # The product window carries the same accessible name, so only elements owned by
  # the Shell (explorer.exe) can be the tray icon.
  $explorer = (Get-Process explorer -ErrorAction SilentlyContinue | Select-Object -First 1).Id
  $result = New-Object System.Collections.ArrayList
  foreach ($e in $found) {
    if ($explorer -and $e.Current.ProcessId -ne $explorer) { continue }
    # The tooltip carries the same name as the icon; only the icon is a button.
    if ($e.Current.ControlType.ProgrammaticName -ne 'ControlType.Button') { continue }
    [void]$result.Add($e)
  }
  return $result
}

if ($Open) {
  $elements = Find-TrayIcon $Name
  if ($elements.Count -eq 0) {
    # The icon is in the notification-area overflow flyout. Its chevron is the
    # left-most `SystemTrayIcon` button; the candidates are tried in that order
    # and the flyout is confirmed by the icon becoming visible. Nothing here
    # depends on localized names (the script has no BOM, so PowerShell 5.1 would
    # read non-ASCII literals as ANSI and never match them).
    Write-Host "icon not in the visible tray; opening the overflow flyout"
    $explorer = (Get-Process explorer -ErrorAction SilentlyContinue | Select-Object -First 1).Id
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $candidates = New-Object System.Collections.ArrayList
    foreach ($e in $root.FindAll(
        [System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.PropertyCondition]::new(
          [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $explorer))) {
      if ($e.Current.AutomationId -eq 'SystemTrayIcon' -and $e.Current.ControlType.ProgrammaticName -eq 'ControlType.Button') {
        [void]$candidates.Add($e)
      }
    }
    $ordered = $candidates | Sort-Object { $_.Current.BoundingRectangle.X }
    foreach ($candidate in $ordered) {
      $r = $candidate.Current.BoundingRectangle
      if ($r.Width -le 0 -or $r.Height -le 0) { continue }
      $cx = [int]($r.X + $r.Width / 2); $cy = [int]($r.Y + $r.Height / 2)
      [void][Tray]::SetCursorPos($cx, $cy)
      Start-Sleep -Milliseconds 300
      [Tray]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 120
      [Tray]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 1200
      $elements = Find-TrayIcon $Name
      Write-Host "  tried tray button at ($cx,$cy): icon visible = $($elements.Count -gt 0)"
      if ($elements.Count -gt 0) { break }
      [Tray]::Key(0x1B)   # close whatever opened and try the next candidate
      Start-Sleep -Milliseconds 400
    }
  }
  Write-Host "tray elements named '$Name': $($elements.Count)"
  if ($CursorAt) {
    $parts = $CursorAt -split ','
    [void][Tray]::SetCursorPos([int]$parts[0], [int]$parts[1])
    Write-Host "cursor parked at $CursorAt before opening the menu"
  }
  $clicked = $false
  foreach ($e in $elements) {
    $r = $e.Current.BoundingRectangle
    Write-Host ("  [{0}] bounds={1},{2} {3}x{4}" -f `
      ($e.Current.ControlType.ProgrammaticName -replace '^ControlType\.', ''), `
      [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height)
    # Prefer UI Automation's Invoke: it opens the menu at the *cursor* instead of
    # synthesising a click, so the popup can be placed away from the flyout.
    try {
      $invoke = $e.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
      $invoke.Invoke()
      Write-Host "  opened tray menu via InvokePattern"
      $clicked = $true
      break
    } catch { }
    if ($r.Width -le 0 -or $r.Height -le 0) { continue }
    $cx = [int]($r.X + $r.Width / 2)
    $cy = [int]($r.Y + $r.Height / 2)
    [void][Tray]::SetCursorPos($cx, $cy)
    Start-Sleep -Milliseconds 300
    [Tray]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
    [Tray]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Write-Host "  clicked tray icon at ($cx,$cy)"
    $clicked = $true
    break
  }
  if (-not $clicked) { Write-Host "no clickable tray element found (icon may be in the overflow flyout)"; exit 2 }
  Start-Sleep -Milliseconds 900
}

if ($Keys) {
  foreach ($key in ($Keys -split ',')) {
    $k = $key.Trim().ToUpperInvariant()
    $vk = switch ($k) {
      'DOWN'  { 0x28 }
      'UP'    { 0x26 }
      'RIGHT' { 0x27 }
      'LEFT'  { 0x25 }
      'ENTER' { 0x0D }
      'ESC'   { 0x1B }
      default { throw "unsupported key $k" }
    }
    [Tray]::Key([byte]$vk)
    Write-Host "  sent $k"
  }
}
