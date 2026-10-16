# Archive and restore the real product profile around a clean-start test.
#
# The product resolves its profile through the Windows known folder, not
# %APPDATA%, so pointing %APPDATA% at a temp directory does *not* isolate it
# (verified: the isolated run read and wrote the real profile and left the
# isolated database with no rows). A genuinely fresh profile therefore requires
# moving the real one aside, which is only safe with a verified archive.
param(
  [ValidateSet('backup', 'restore', 'status')][string]$Action = 'status',
  [string]$Archive = "$env:TEMP\rc1-profile-archive"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$profile = Join-Path $env:APPDATA 'net.alanfloyd.desktop'

function Get-TreeReport([string]$Path) {
  if (-not (Test-Path $Path)) { return @() }
  Get-ChildItem $Path -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Substring($Path.Length).TrimStart('\')
    [pscustomobject]@{
      Path   = $rel
      Length = $_.Length
      SHA256 = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
    }
  }
}

if ($Action -eq 'backup') {
  Get-Process alan-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
  if (Test-Path $Archive) { Remove-Item $Archive -Recurse -Force }
  New-Item -ItemType Directory -Force -Path $Archive | Out-Null
  $zip = Join-Path $Archive 'net.alanfloyd.desktop.zip'
  [System.IO.Compression.ZipFile]::CreateFromDirectory($profile, $zip)
  Get-TreeReport $profile | ConvertTo-Json -Depth 3 |
    Set-Content (Join-Path $Archive 'manifest.json') -Encoding UTF8
  $count = (Get-TreeReport $profile | Measure-Object).Count
  Write-Output "archived $count files -> $zip ($((Get-Item $zip).Length) bytes)"
  Write-Output "manifest -> $(Join-Path $Archive 'manifest.json')"
}

elseif ($Action -eq 'restore') {
  Get-Process alan-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
  $zip = Join-Path $Archive 'net.alanfloyd.desktop.zip'
  if (-not (Test-Path $zip)) { throw "archive missing: $zip" }
  # Remove whatever the clean-start run created, then restore the archive
  # contents exactly (assets, database, and any WAL/SHM siblings).
  if (Test-Path $profile) { Remove-Item $profile -Recurse -Force }
  [System.IO.Compression.ZipFile]::ExtractToDirectory($zip, $env:APPDATA)
  $restored = Get-TreeReport $profile
  $manifest = Get-Content (Join-Path $Archive 'manifest.json') -Raw | ConvertFrom-Json
  $problems = @()
  foreach ($entry in $manifest) {
    $match = $restored | Where-Object { $_.Path -eq $entry.Path }
    if (-not $match) { $problems += "missing: $($entry.Path)"; continue }
    if ($match.Length -ne $entry.Length) { $problems += "size differs: $($entry.Path)"; continue }
    if ($match.SHA256 -ne $entry.SHA256) { $problems += "hash differs: $($entry.Path)"; continue }
  }
  foreach ($entry in $restored) {
    if (-not ($manifest | Where-Object { $_.Path -eq $entry.Path })) {
      $problems += "unexpected extra file: $($entry.Path)"
    }
  }
  Write-Output "restored $($restored.Count) files from archive"
  if ($problems.Count -eq 0) {
    Write-Output "RESTORE VERIFIED: every file matches the pre-test manifest by size and SHA-256"
  } else {
    Write-Output "RESTORE MISMATCH ($($problems.Count)):"
    $problems | ForEach-Object { Write-Output "  $_" }
  }
}

else {
  if (-not (Test-Path $profile)) { Write-Output "profile absent: $profile"; exit 0 }
  Get-ChildItem $profile -Force | Select-Object Name, Length, LastWriteTime |
    Format-Table -AutoSize | Out-String | Write-Output
}
