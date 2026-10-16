# RC-0 QA helper: finds a UI Automation element by name in the product window
# and reports (or clicks) its centre.
#
# The WebView2 content tree is exposed to UI Automation on the Standard backend,
# so this drives the real in-app affordances — the same elements a mouse user
# clicks — rather than a test-only path.
param(
  [string]$ProcessName = "alan-desktop",
  [string]$Name,
  [switch]$Click,
  [switch]$List
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Rc0Input {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", SetLastError=true)] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
}
"@
[void][Rc0Input]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

$proc = Get-Process -Name $ProcessName -ErrorAction Stop | Sort-Object Id | Select-Object -First 1
$condition = [System.Windows.Automation.PropertyCondition]::new(
  [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $proc.Id)
$window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
  [System.Windows.Automation.TreeScope]::Children, $condition)
if (-not $window) { throw "product window not found for pid $($proc.Id)" }

$nameCondition = [System.Windows.Automation.PropertyCondition]::new(
  [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
$found = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $nameCondition)
if ($found.Count -eq 0) {
  Write-Host "NOT FOUND: '$Name'"
  if ($List) {
    Write-Host "--- available named elements ---"
    $all = $window.FindAll(
      [System.Windows.Automation.TreeScope]::Descendants,
      [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($e in $all) {
      if ($e.Current.Name) {
        Write-Host ("  [{0}] '{1}'" -f ($e.Current.ControlType.ProgrammaticName -replace '^ControlType\.', ''), $e.Current.Name)
      }
    }
  }
  exit 2
}

foreach ($element in $found) {
  $r = $element.Current.BoundingRectangle
  Write-Host ("found [{0}] '{1}' bounds={2},{3} {4}x{5}" -f `
    ($element.Current.ControlType.ProgrammaticName -replace '^ControlType\.', ''), `
    $element.Current.Name, [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height)

  if ($Click) {
    # Invoke through UI Automation rather than synthesising a click at the
    # reported bounds: WebView2's BoundingRectangle values are not reliable
    # enough here to aim a real mouse at, while InvokePattern reaches the same
    # DOM activation the user's click performs.
    $invoked = $false
    foreach ($pattern in @(
        [System.Windows.Automation.InvokePattern]::Pattern,
        [System.Windows.Automation.SelectionItemPattern]::Pattern,
        [System.Windows.Automation.TogglePattern]::Pattern)) {
      try {
        $p = $element.GetCurrentPattern($pattern)
        if ($pattern -eq [System.Windows.Automation.InvokePattern]::Pattern) { $p.Invoke() }
        elseif ($pattern -eq [System.Windows.Automation.SelectionItemPattern]::Pattern) { $p.Select() }
        else { $p.Toggle() }
        $invoked = $true
        Write-Host ("  invoked via {0}" -f ($pattern.ProgrammaticName -replace '^.*\.', ''))
        break
      } catch { }
    }
    if (-not $invoked) { Write-Host "  no invokable pattern on this element" }
    Start-Sleep -Milliseconds 1500
  }
}
