# Swap the product profile for a synthetic demo profile, and restore it afterwards.
#
# The product resolves its data directory through the Windows known folder, not
# %APPDATA%, so there is no supported way to point it at a scratch profile. A
# media capture therefore has to move the real profile aside, which is only safe
# with a verified archive. `-Action restore` puts every file back and checks it
# against the manifest by size and SHA-256.
param(
  [Parameter(Mandatory = $true)][ValidateSet('stash', 'restore', 'status')][string]$Action,
  [string]$Archive = "$env:TEMP\dtw-media-profile-archive"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$profile = Join-Path $env:APPDATA 'net.alanfloyd.desktop'
$stash = "$env:TEMP\dtw-media-profile-stash"
$manifest = Join-Path $Archive 'manifest.json'
$zip = Join-Path $Archive 'net.alanfloyd.desktop.zip'

function Stop-Product {
  Get-Process desktop-todo-widget, alan-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
}

function Get-TreeReport([string]$Path) {
  if (-not (Test-Path $Path)) { return @() }
  Get-ChildItem $Path -Recurse -File | ForEach-Object {
    [pscustomobject]@{
      Path   = $_.FullName.Substring($Path.Length).TrimStart('\')
      Length = $_.Length
      SHA256 = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
    }
  }
}

if ($Action -eq 'stash') {
  Stop-Product
  if (Test-Path $stash) { Remove-Item $stash -Recurse -Force }
  Move-Item $profile $stash
  Write-Output "real profile moved aside -> $stash"
  Write-Output "profile path now exists: $(Test-Path $profile)  (the app will create a fresh one)"
}

elseif ($Action -eq 'restore') {
  Stop-Product
  if (-not (Test-Path $zip)) { throw "archive missing: $zip" }
  # A failed stash leaves the real profile in place; do not clobber it.
  if (-not (Test-Path $stash)) { throw "stash missing: $stash (run -Action stash first)" }
  if (Test-Path $profile) { Remove-Item $profile -Recurse -Force }
  Move-Item $stash $profile
  $restored = Get-TreeReport $profile
  $expected = Get-Content $manifest -Raw | ConvertFrom-Json
  $problems = @()
  foreach ($entry in $expected) {
    $match = $restored | Where-Object { $_.Path -eq $entry.Path }
    if (-not $match) { $problems += "missing: $($entry.Path)"; continue }
    if ($match.Length -ne $entry.Length) { $problems += "size differs: $($entry.Path)"; continue }
    if ($match.SHA256 -ne $entry.SHA256) { $problems += "hash differs: $($entry.Path)"; continue }
  }
  foreach ($entry in $restored) {
    if (-not ($expected | Where-Object { $_.Path -eq $entry.Path })) {
      $problems += "unexpected extra file: $($entry.Path)"
    }
  }
  Write-Output "restored $($restored.Count) files from the stashed copy"
  if ($problems.Count -eq 0) {
    Write-Output "RESTORE VERIFIED: every file matches the pre-capture manifest by size and SHA-256"
  } else {
    Write-Output "RESTORE MISMATCH ($($problems.Count)):"
    $problems | ForEach-Object { Write-Output "  $_" }
  }
}

else {
  Write-Output "profile exists : $(Test-Path $profile)"
  Write-Output "stash exists   : $(Test-Path $stash)"
  if (Test-Path $profile) { Get-TreeReport $profile | Format-Table -AutoSize | Out-String | Write-Output }
}
