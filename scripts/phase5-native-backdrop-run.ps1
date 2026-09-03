param(
  [Parameter(Mandatory = $true)][string]$EvidenceDirectory
)

$ErrorActionPreference = 'Stop'
$workspace = 'D:\Documents\Desktop_todo_list'
$testData = Join-Path $workspace 'target\phase5-native-appdata\net.alanfloyd.desktop'
$fixtureScript = Join-Path $workspace 'scripts\phase5-backdrop-fixture.ps1'
$caseScript = Join-Path $workspace 'scripts\phase5-native-backdrop-ab.ps1'
$fixtureHandle = Join-Path $EvidenceDirectory 'fixture-hwnd.txt'
$devStdout = Join-Path $EvidenceDirectory 'tauri-dev.stdout.log'
$devStderr = Join-Path $EvidenceDirectory 'tauri-dev.stderr.log'

New-Item -ItemType Directory -Path $EvidenceDirectory -Force | Out-Null

Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Phase5BackdropPolicy {
  [StructLayout(LayoutKind.Sequential)]
  public struct SYSTEM_POWER_STATUS {
    public byte ACLineStatus, BatteryFlag, BatteryLifePercent, SystemStatusFlag;
    public uint BatteryLifeTime, BatteryFullLifeTime;
  }
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
  [DllImport("dwmapi.dll", PreserveSig = true)] public static extern int DwmIsCompositionEnabled(out int enabled);
  [DllImport("kernel32.dll")] public static extern bool GetSystemPowerStatus(out SYSTEM_POWER_STATUS status);
}
"@
$personalize = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -ErrorAction SilentlyContinue
$compositionEnabled = 0
$compositionHresult = [Phase5BackdropPolicy]::DwmIsCompositionEnabled([ref]$compositionEnabled)
$power = [Phase5BackdropPolicy+SYSTEM_POWER_STATUS]::new()
[void][Phase5BackdropPolicy]::GetSystemPowerStatus([ref]$power)
$policy = [ordered]@{
  enableTransparency = $personalize.EnableTransparency
  highContrast = [System.Windows.Forms.SystemInformation]::HighContrast
  remoteSession = ([Phase5BackdropPolicy]::GetSystemMetrics(0x1000) -ne 0)
  dwmCompositionEnabled = ($compositionEnabled -ne 0)
  dwmCompositionHresult = ('0x{0:X8}' -f ($compositionHresult -band 0xFFFFFFFFL))
  acLineStatus = $power.ACLineStatus
  batteryFlag = $power.BatteryFlag
  batteryLifePercent = $power.BatteryLifePercent
  energySaverStatus = $power.SystemStatusFlag
}
[System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory 'system-policy.json'), ($policy | ConvertTo-Json), [Text.Encoding]::UTF8)

[System.IO.File]::WriteAllText($fixtureHandle, '', [Text.Encoding]::ASCII)
$fixtureArguments = "-NoProfile -ExecutionPolicy Bypass -File `"$fixtureScript`" -HandlePath `"$fixtureHandle`""
$fixtureProcess = Start-Process -FilePath 'powershell.exe' -WindowStyle Hidden -ArgumentList $fixtureArguments -PassThru
$fixtureDeadline = [DateTime]::UtcNow.AddSeconds(15)
$fixtureHwndText = ''
while ([DateTime]::UtcNow -lt $fixtureDeadline) {
  if ($fixtureProcess.HasExited) { throw 'Backdrop fixture exited before creating its HWND' }
  $fixtureHwndText = [System.IO.File]::ReadAllText($fixtureHandle).Trim()
  if ($fixtureHwndText) { break }
  Start-Sleep -Milliseconds 250
}
if (-not $fixtureHwndText) { throw 'Timed out waiting for backdrop fixture HWND' }

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Phase5BackdropFixtureWindow {
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int command);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
}
"@
$fixtureHwnd = [IntPtr]::new([Convert]::ToInt64($fixtureHwndText.Replace('0x', ''), 16))
if (-not [Phase5BackdropFixtureWindow]::IsWindow($fixtureHwnd)) { throw 'Backdrop fixture HWND is invalid' }
[void][Phase5BackdropFixtureWindow]::ShowWindow($fixtureHwnd, 5)
[void][Phase5BackdropFixtureWindow]::SetWindowPos($fixtureHwnd, [IntPtr]::Zero, 0, 0, 0, 0, 0x0010 -bor 0x0002 -bor 0x0001)
[void][Phase5BackdropFixtureWindow]::SetForegroundWindow($fixtureHwnd)

$env:ALAN_PHASE5_BACKDROP_DATA_DIR = $testData
$env:ALAN_PHASE5_BACKDROP_PROBE = '1'
$devProcess = Start-Process -FilePath 'pnpm.cmd' -WindowStyle Hidden -WorkingDirectory $workspace -ArgumentList 'tauri:dev' -RedirectStandardOutput $devStdout -RedirectStandardError $devStderr -PassThru

try {
  $appProcess = $null
  $deadline = [DateTime]::UtcNow.AddMinutes(2)
  while ([DateTime]::UtcNow -lt $deadline) {
    $appProcess = Get-Process -Name 'alan-desktop' -ErrorAction SilentlyContinue |
      Where-Object { $_.Path -eq (Join-Path $workspace 'src-tauri\target\debug\alan-desktop.exe') } |
      Sort-Object StartTime -Descending |
      Select-Object -First 1
    if ($appProcess) { break }
    if ($devProcess.HasExited) {
      throw "tauri:dev exited before Alan Desktop launched; see $devStderr"
    }
    Start-Sleep -Milliseconds 500
  }
  if (-not $appProcess) { throw 'Timed out waiting for Alan Desktop' }
  Start-Sleep -Seconds 15

  $cases = @(
    @{ Name = 'none-normal'; Backdrop = 'None'; Frame = 'Normal' },
    @{ Name = 'transient-normal'; Backdrop = 'Transient'; Frame = 'Normal' },
    @{ Name = 'none-full-frame'; Backdrop = 'None'; Frame = 'Full' },
    @{ Name = 'transient-full-frame'; Backdrop = 'Transient'; Frame = 'Full' },
    @{ Name = 'transient-alpha-normal'; Backdrop = 'Transient'; Frame = 'Normal'; RedirectionAlpha = 'True' },
    @{ Name = 'transient-alpha-full-frame'; Backdrop = 'Transient'; Frame = 'Full'; RedirectionAlpha = 'True' }
  )
  foreach ($case in $cases) {
    $imagePath = Join-Path $EvidenceDirectory ($case.Name + '.png')
    $jsonPath = Join-Path $EvidenceDirectory ($case.Name + '.json')
    $redirectionAlpha = if ($case.RedirectionAlpha) { $case.RedirectionAlpha } else { 'Default' }
    & $caseScript -Backdrop $case.Backdrop -Frame $case.Frame -ImagePath $imagePath -JsonPath $jsonPath -RedirectionAlpha $redirectionAlpha
  }
} finally {
  Get-Process -Name 'alan-desktop' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq (Join-Path $workspace 'src-tauri\target\debug\alan-desktop.exe') } |
    ForEach-Object { $_.Kill() }
  if ($devProcess -and -not $devProcess.HasExited) { $devProcess.Kill() }
  if ($fixtureProcess -and -not $fixtureProcess.HasExited) { $fixtureProcess.Kill() }
}
