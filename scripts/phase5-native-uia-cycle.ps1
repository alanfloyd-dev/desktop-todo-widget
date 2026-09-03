param([ValidateRange(1, 100)][int]$Count = 1)

$target = Get-Process alan-desktop -ErrorAction Stop | Select-Object -First 1
for ($iteration = 1; $iteration -le $Count; $iteration++) {
  & "$PSScriptRoot\phase5-native-uia.ps1" -ProcessId $target.Id -Name 'Open Alan Desktop'
  Start-Sleep -Milliseconds 350
  & "$PSScriptRoot\phase5-native-uia.ps1" -ProcessId $target.Id -Name 'Collapse to Avatar Orb'
  Start-Sleep -Milliseconds 350
  Write-Output "Native presentation cycle $iteration/$Count complete"
}
