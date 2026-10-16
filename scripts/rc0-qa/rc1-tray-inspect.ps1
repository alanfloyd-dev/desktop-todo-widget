# Inspect and activate rows of the product's native tray menu.
#
# `tray-menu-drive.ps1` opens the notification-area menu (the icon lives in the
# overflow flyout, so the chevron is left-clicked first). This reads that popup's
# UI Automation tree - including any submenu - and can invoke a row by name. The
# tray menu is the same `build_tray_menu` output the widget's own right-click menu
# uses, so activating Quit here is the real product quit path.
param(
  [switch]$Open,
  [string]$Invoke = '',
  [string]$Submenu = '',
  [int]$WaitMs = 900
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class TrayCtl {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  public static void Key(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(70); keybd_event(vk, 0, 2, UIntPtr.Zero); System.Threading.Thread.Sleep(180); }
}
"@
[void][TrayCtl]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

function Get-MenuRows {
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $condition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem)
  $items = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
  $rows = @()
  foreach ($item in $items) {
    $rows += [pscustomobject]@{
      Name     = $item.Current.Name
      Enabled  = $item.Current.IsEnabled
      HasPopup = ($item.GetSupportedPatterns() | Where-Object { $_ -eq [System.Windows.Automation.ExpandCollapsePattern]::Pattern }) -ne $null
      Element  = $item
    }
  }
  $rows
}

if ($Open) {
  & (Join-Path $PSScriptRoot 'tray-menu-drive.ps1') -Open | Out-Null
  Start-Sleep -Milliseconds $WaitMs
}

Write-Output '--- native tray menu rows ---'
$rows = Get-MenuRows
$index = 0
foreach ($row in $rows) {
  Write-Output ("row {0,2} enabled={1,-5} popup={2,-5} name='{3}'" -f $index, $row.Enabled, $row.HasPopup, $row.Name)
  $index++
}

if ($Submenu) {
  $parent = $rows | Where-Object { $_.Name -eq $Submenu } | Select-Object -First 1
  if (-not $parent) { throw "submenu '$Submenu' not found" }
  $expand = $parent.Element.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
  $expand.Expand()
  Start-Sleep -Milliseconds 700
  Write-Output "--- rows after expanding '$Submenu' ---"
  $rows = Get-MenuRows
  $index = 0
  foreach ($row in $rows) {
    Write-Output ("row {0,2} enabled={1,-5} name='{2}'" -f $index, $row.Enabled, $row.Name)
    $index++
  }
}

if ($Invoke) {
  $target = $rows | Where-Object { $_.Name -eq $Invoke } | Select-Object -First 1
  if (-not $target) { throw "menu row '$Invoke' not found" }
  if (-not $target.Enabled) { throw "menu row '$Invoke' is disabled" }
  $invoke = $target.Element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
  $invoke.Invoke()
  Write-Output "invoked '$Invoke'"
}
