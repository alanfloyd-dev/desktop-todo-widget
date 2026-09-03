param(
  [string]$HwndHex = "",
  [int]$ProcessId = 0,
  [string]$Name
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$roots = if ($HwndHex) {
  $raw = [IntPtr]::new([Convert]::ToInt64($HwndHex.Replace('0x', ''), 16))
  @([System.Windows.Automation.AutomationElement]::FromHandle($raw))
} else {
  $processCondition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ProcessIdProperty,
    $ProcessId
  )
  [System.Windows.Automation.AutomationElement]::RootElement.FindAll(
    [System.Windows.Automation.TreeScope]::Descendants,
    $processCondition
  )
}
if (-not $roots -or $roots.Count -eq 0) { throw "Automation root unavailable" }
$condition = [System.Windows.Automation.PropertyCondition]::new(
  [System.Windows.Automation.AutomationElement]::NameProperty,
  $Name
)
$element = $null
foreach ($candidate in $roots) {
  if ($candidate.Current.Name -eq $Name) {
    $element = $candidate
    break
  }
  $element = $candidate.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
  if ($element) { break }
}
if (-not $element) { throw "Automation element '$Name' unavailable" }
$pattern = $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
$pattern.Invoke()
Start-Sleep -Milliseconds 500
Write-Output "Invoked: $Name"
