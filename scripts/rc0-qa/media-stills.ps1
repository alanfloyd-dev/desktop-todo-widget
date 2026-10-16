# Capture the four README stills, one window mode each.
#
# Each mode is applied through the persisted setting and a restart: the tray menu
# cannot be driven in this session, and a restart is also what guarantees the
# window is in the mode's real startup state rather than a transient one.
param(
  [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
  [string]$OutDir = (Join-Path $Root 'docs\assets'),
  [string]$Exe = (Join-Path $Root 'src-tauri\target\release\alan-desktop.exe')
)

$ErrorActionPreference = 'Stop'
$capture = Join-Path $Root 'scripts\rc0-qa\media-capture.ps1'
$settings = Join-Path $Root '.rc0-evidence\rc1-settings.py'

function Set-Mode([string]$mode, [string]$presentation) {
  & python $settings set "mode=$mode" "floatingPresentation=$presentation" | Out-Null
}

function Launch-App {
  Get-Process alan-desktop, desktop-todo-widget -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 3
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9340'
  Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) | Out-Null
  Start-Sleep -Seconds 16
}

$steps = @(
  @{ Name = 'floating-acrylic'; Mode = 'floating'; Presentation = 'expanded';  Move = '820,60' },
  @{ Name = 'orb';              Mode = 'floating'; Presentation = 'collapsed'; Move = '300,200' },
  @{ Name = 'sidebar';          Mode = 'sidebar';  Presentation = 'expanded';  Move = '0,0' },
  @{ Name = 'desktop';          Mode = 'desktop';  Presentation = 'expanded';  Move = '' }
)

foreach ($step in $steps) {
  Write-Output "==== $($step.Name)  (mode=$($step.Mode) presentation=$($step.Presentation)) ===="
  Set-Mode $step.Mode $step.Presentation
  Launch-App
  if ($step.Move) {
    & powershell -ExecutionPolicy Bypass -File $capture -Action probe -MoveTo $step.Move | Select-String 'Rect' | ForEach-Object { $_.Line }
  }
  # Focus the window so the Acrylic host is in its active state for the capture.
  & powershell -ExecutionPolicy Bypass -File $capture -Action focus | ForEach-Object { $_ }
  $out = Join-Path $OutDir "$($step.Name).png"
  & powershell -ExecutionPolicy Bypass -File $capture -Action capture -Out $out | ForEach-Object { $_ }
}
Write-Output 'done'
