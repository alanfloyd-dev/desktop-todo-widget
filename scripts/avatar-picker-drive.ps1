#Requires -Version 5.1
<#
.SYNOPSIS
    Drives the product's native avatar/background file picker (UI Automation helper).

.DESCRIPTION
    `choose_local_asset` opens the Win32 common file dialog titled "Choose a local
    image". This helper is the QA-side counterpart: it finds that dialog, types a
    path into its file-name box and confirms it, so an automated run exercises the
    real picker rather than a stand-in.

    The dialog window is located with `FindWindowW("#32770", title)` and wrapped
    with `AutomationElement::FromHandle`, which is more direct than enumerating the
    desktop root and does not depend on the dialog being an immediate UIA child of
    the root element.

    Modes:
      Wait    wait for the dialog and report whether it appeared (no interaction).
      Select  type -DialogPath into the file-name box and open it.
      Dump    print the dialog's interesting controls (automation id, class, name,
              patterns) for diagnosis.

.PARAMETER DialogPath
    Full path to the image to select. Must already exist.

.PARAMETER DialogTitle
    Window title of the dialog. The product's own title is the default.

.PARAMETER TimeoutSeconds
    How long to wait for the dialog to appear.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/avatar-picker-drive.ps1 -Mode Dump

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/avatar-picker-drive.ps1 -DialogPath C:\tmp\avatar.png
#>
[CmdletBinding()]
param(
    [ValidateSet('Wait', 'Select', 'Dump')][string]$Mode = 'Select',
    [string]$DialogPath = '',
    [string]$DialogTitle = 'Choose a local image',
    [int]$TimeoutSeconds = 25
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PickerWindow {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, IntPtr extra);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  public static void ClickButton(IntPtr h) { SendMessageW(h, 0x00F5 /* BM_CLICK */, IntPtr.Zero, IntPtr.Zero); }
  public static void ClickAt(int x, int y) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(120);
    mouse_event(0x0002 /* LEFTDOWN */, 0, 0, 0, IntPtr.Zero);
    mouse_event(0x0004 /* LEFTUP */, 0, 0, 0, IntPtr.Zero);
  }
  public static string Classes() {
    var seen = new StringBuilder();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      var sb = new StringBuilder(256);
      GetClassNameW(h, sb, sb.Capacity);
      if (sb.ToString() == "#32770" && IsWindowVisible(h)) { seen.Append(h.ToInt64()).Append(' '); }
      return true;
    }, IntPtr.Zero);
    return seen.ToString();
  }
}
"@

function Get-DialogHandle {
    param([int]$Seconds)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $handle = [PickerWindow]::FindWindowW('#32770', $DialogTitle)
        if ($handle -ne [IntPtr]::Zero -and [PickerWindow]::IsWindowVisible($handle)) { return $handle }
        Start-Sleep -Milliseconds 250
    }
    return [IntPtr]::Zero
}

$handle = Get-DialogHandle -Seconds $TimeoutSeconds
if ($handle -eq [IntPtr]::Zero) {
    Write-Output ("{{""ok"":false,""reason"":""dialog-not-found"",""visible32770"":""{0}""}}" -f ([PickerWindow]::Classes()).Trim())
    exit 1
}

$dialog = [System.Windows.Automation.AutomationElement]::FromHandle($handle)
if (-not $dialog) {
    Write-Output '{"ok":false,"reason":"dialog-element-unavailable"}'
    exit 1
}

function Get-Descendants($element) {
    return $element.FindAll(
        [System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition)
}

function Get-PatternNames($element) {
    $names = @()
    foreach ($pattern in $element.GetSupportedPatterns()) { $names += $pattern.ProgrammaticName }
    return ($names -join ',')
}

# The common dialog bridges through MSAA, so several of its controls report
# ControlType.Pane rather than Edit/Button. Locate them by automation id plus
# class, and probe the pattern before using it.
function Find-ChildByAutomationIdAndClass($dialog, [string]$id, [string]$className) {
    foreach ($element in Get-Descendants $dialog) {
        if ($element.Current.AutomationId -eq $id -and $element.Current.ClassName -eq $className) { return $element }
    }
    return $null
}

function Find-ChildByAutomationId($dialog, [string]$id) {
    foreach ($element in Get-Descendants $dialog) {
        if ($element.Current.AutomationId -eq $id) { return $element }
    }
    return $null
}

function Test-Pattern($element, $pattern) {
    if (-not $element) { return $false }
    foreach ($supported in $element.GetSupportedPatterns()) {
        if ($supported -eq $pattern) { return $true }
    }
    return $false
}

if ($Mode -eq 'Wait') {
    Write-Output '{"ok":true,"found":true}'
    exit 0
}

if ($Mode -eq 'Dump') {
    Write-Host "dialog: class=$($dialog.Current.ClassName) name='$($dialog.Current.Name)' pid=$($dialog.Current.ProcessId)"
    foreach ($element in Get-Descendants $dialog) {
        $automationId = ''
        try { $automationId = $element.Current.AutomationId } catch { }
        $className = ''
        try { $className = $element.Current.ClassName } catch { }
        if ($automationId -notin @('1148', '1', '2', '1136')) { continue }
        Write-Host ("  {0} | name='{1}' | autoId='{2}' | class={3} | hwnd={4} | patterns={5}" -f `
            $element.Current.ControlType.ProgrammaticName, $element.Current.Name, $automationId, $className,
            $element.Current.NativeWindowHandle, (Get-PatternNames $element))
    }
    exit 0
}

if ([string]::IsNullOrWhiteSpace($DialogPath) -or -not (Test-Path -LiteralPath $DialogPath -PathType Leaf)) {
    Write-Output '{"ok":false,"reason":"missing-file"}'
    exit 1
}

$fileNameBox = Find-ChildByAutomationIdAndClass $dialog '1148' 'Edit'
if (-not $fileNameBox) { $fileNameBox = Find-ChildByAutomationId $dialog '1148' }
if (-not $fileNameBox) {
    Write-Output '{"ok":false,"reason":"file-name-box-not-found"}'
    exit 1
}

# Focus the file-name box the way a user does — a click on it — then paste the
# path from the clipboard and press Enter.
#
# This dialog bridges through MSAA and reports its name box and buttons as
# ControlType.Pane, where ValuePattern/InvokePattern and even SetFocus are not
# reliably available; a real click at the element's own rectangle plus keyboard
# input avoids depending on any of them.
[void][PickerWindow]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds 300
$rect = $fileNameBox.Current.BoundingRectangle
$clicked = $false
if ($rect.Width -gt 1 -and $rect.Height -gt 1) {
    [PickerWindow]::ClickAt([int]($rect.Left + $rect.Width / 2), [int]($rect.Top + $rect.Height / 2))
    $clicked = $true
    Start-Sleep -Milliseconds 300
}

$committed = ''
if ($clicked) {
    [System.Windows.Forms.Clipboard]::SetText($DialogPath)
    [System.Windows.Forms.SendKeys]::SendWait('^a')
    Start-Sleep -Milliseconds 150
    [System.Windows.Forms.SendKeys]::SendWait('^v')
    Start-Sleep -Milliseconds 400
    [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
    $committed = 'click-paste-enter'
}
else {
    # No usable rectangle: fall back to the UIA value pattern, then to the accept
    # button.
    $valuePattern = [System.Windows.Automation.ValuePattern]::Pattern
    if (Test-Pattern $fileNameBox $valuePattern) {
        $fileNameBox.GetCurrentPattern($valuePattern).SetValue($DialogPath)
        $committed = 'value-pattern'
    }
    else {
        Write-Output '{"ok":false,"reason":"no-input-path"}'
        exit 1
    }
    $openButton = Find-ChildByAutomationIdAndClass $dialog '1' 'Button'
    if (-not $openButton) { $openButton = Find-ChildByAutomationId $dialog '1' }
    $invokePattern = [System.Windows.Automation.InvokePattern]::Pattern
    if (Test-Pattern $openButton $invokePattern) {
        $openButton.GetCurrentPattern($invokePattern).Invoke()
        $committed = "$committed+invoke"
    }
    elseif ($openButton -and $openButton.Current.NativeWindowHandle -ne 0) {
        [void][PickerWindow]::ClickButton([IntPtr]$openButton.Current.NativeWindowHandle)
        $committed = "$committed+bm_click"
    }
}

$closed = $false
$deadline = (Get-Date).AddSeconds(10)
while ((Get-Date) -lt $deadline) {
    $still = [PickerWindow]::FindWindowW('#32770', $DialogTitle)
    if ($still -eq [IntPtr]::Zero) { $closed = $true; break }
    Start-Sleep -Milliseconds 300
}

Write-Output ("{{""ok"":{0},""closed"":{0},""committed"":""{1}"",""path"":{2}}}" -f `
    $(if ($closed) { 'true' } else { 'false' }), $committed, ($DialogPath | ConvertTo-Json -Compress))
if ($closed) { exit 0 }
exit 1
