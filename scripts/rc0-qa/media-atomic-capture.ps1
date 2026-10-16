# Capture the product widget with the operator's overlapping windows hidden for the
# duration of the frame only.
#
# Order matters and is why this is one script: the session terminal becomes visible
# again whenever the operator's shell runs a command, so hiding it in a previous
# step and capturing in the next always catches it back on screen. Both happen here,
# inside a single process, with no command boundary in between.
param(
  [Parameter(Mandatory = $true)][string]$Out,
  [string]$HideHandles = '',
  [string]$ProcessName = 'alan-desktop',
  [string]$MoveTo = '',
  [switch]$NoFocus
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$hidden = @()

try {
  if ($HideHandles) {
    foreach ($entry in ($HideHandles.Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
      & powershell -ExecutionPolicy Bypass -File (Join-Path $here 'media-window-visibility.ps1') -Action hide -Handle $entry | Out-Null
      $hidden += $entry
    }
    Start-Sleep -Milliseconds 900
  }

  if ($MoveTo) {
    & powershell -ExecutionPolicy Bypass -File (Join-Path $here 'media-capture.ps1') -Action probe -MoveTo $MoveTo | Out-Null
  }

  $captureArgs = @('-Action', 'capture', '-Out', $Out, '-ProcessName', $ProcessName)
  if ($NoFocus) { $captureArgs += '-NoFocus' }
  & powershell -ExecutionPolicy Bypass -File (Join-Path $here 'media-capture.ps1') @captureArgs
}
finally {
  # Always restore, even if the capture threw.
  foreach ($entry in $hidden) {
    & powershell -ExecutionPolicy Bypass -File (Join-Path $here 'media-window-visibility.ps1') -Action show -Handle $entry | Out-Null
  }
}
