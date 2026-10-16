# RC-1 verification: launch the release binary with a chosen APPDATA root.
#
# The product resolves its app-data through %APPDATA%\<identifier>, so an
# isolated %APPDATA% gives a genuine "near-fresh user" run without touching the
# real profile. The launch is a plain Start-Process on the release executable;
# the app's own QA log (written beside the exe) is the evidence source because
# the confined harness cannot capture a piped child's output.
param(
  [Parameter(Mandatory = $true)][string]$AppDataRoot,
  [string]$Exe = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'src-tauri\target\release\alan-desktop.exe'),
  [string]$ExtraArgs = '',
  [int]$SettleSeconds = 14,
  [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'

Get-Process alan-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

$env:APPDATA = $AppDataRoot
Write-Host "APPDATA=$($env:APPDATA)"

$arguments = @()
if ($ExtraArgs) { $arguments = $ExtraArgs.Split(' ', [System.StringSplitOptions]::RemoveEmptyEntries) }

# ArgumentList may not be an empty collection: omit it entirely for a plain launch.
if ($arguments.Count -gt 0) {
  $proc = Start-Process -FilePath $Exe -ArgumentList $arguments -WorkingDirectory (Split-Path $Exe) -PassThru
} else {
  $proc = Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) -PassThru
}
Write-Host "launched pid=$($proc.Id) args=[$($arguments -join ' ')]"

Start-Sleep -Seconds $SettleSeconds

$alive = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
if ($alive) {
  Write-Host "process alive: pid=$($alive.Id) responding=$($alive.Responding)"
} else {
  Write-Host "PROCESS EXITED EARLY"
}

Write-Host "--- product windows ---"
& (Join-Path $PSScriptRoot 'rc1-windows.ps1') 2>&1 | Out-String | Write-Host

Write-Host "--- DB written under isolated appdata ---"
$dbDir = Join-Path $AppDataRoot 'net.alanfloyd.desktop'
if (Test-Path $dbDir) {
  Get-ChildItem $dbDir -Recurse -File | Select-Object Name, Length | Format-Table -AutoSize | Out-String | Write-Host
} else {
  Write-Host "NOT CREATED: $dbDir"
}

if (-not $KeepRunning) {
  Get-Process alan-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
  Write-Host "stopped"
}
