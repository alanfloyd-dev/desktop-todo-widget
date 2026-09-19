#Requires -Version 5.1
# Diagnostic tool (test-only): reproduces the interrupted-install retry and,
# when a replacement is refused, asks Restart Manager which processes hold
# the file. Used to attribute the transient ACCESS_DENIED conflicts to the
# third-party real-time protection service (QQPCMgr RTP) observed on the
# development machine. Run manually; never touches production locations.
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$helperExe = Join-Path $repoRoot 'src-tauri\target\debug\desktop-todo-maintenance.exe'
$productExe = Join-Path $repoRoot 'src-tauri\target\debug\alan-desktop.exe'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class RM {
    [StructLayout(LayoutKind.Sequential)]
    public struct RM_UNIQUE_PROCESS {
        public int dwProcessId;
        public System.Runtime.InteropServices.ComTypes.FILETIME ProcessStartTime;
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct RM_PROCESS_INFO {
        public RM_UNIQUE_PROCESS Process;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string strAppName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string strServiceShortName;
        public int ApplicationType;
        public uint AppStatus;
        public uint TSSessionId;
        [MarshalAs(UnmanagedType.Bool)] public bool bRestartable;
    }
    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    public static extern int RmStartSession(out uint pSessionHandle, int dwSessionFlags, string strSessionKey);
    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    public static extern int RmRegisterResources(uint pSessionHandle, uint nFiles, string[] rgsFilenames, uint nApplications, RM_PROCESS_INFO[] rgApplications, uint nServices, string[] rgsServiceNames);
    [DllImport("rstrtmgr.dll")]
    public static extern int RmGetList(uint dwSessionHandle, out uint pnProcInfoNeeded, ref uint pnProcInfo, [In, Out] RM_PROCESS_INFO[] rgAffectedApps, ref uint lpdwRebootReasons);
    [DllImport("rstrtmgr.dll")]
    public static extern int RmEndSession(uint dwSessionHandle);
}
'@

function Get-FileLockers {
    param([Parameter(Mandatory = $true)][string]$Path)
    $session = 0
    $key = [guid]::NewGuid().ToString()
    $result = [RM]::RmStartSession([ref]$session, 0, $key)
    if ($result -ne 0) { throw "RmStartSession failed: $result" }
    try {
        $result = [RM]::RmRegisterResources($session, 1, @($Path), 0, $null, 0, $null)
        if ($result -ne 0) { throw "RmRegisterResources failed: $result" }
        $needed = 0
        $count = 10
        $info = New-Object 'RM+RM_PROCESS_INFO[]' 10
        $reasons = 0
        $result = [RM]::RmGetList($session, [ref]$needed, [ref]$count, $info, [ref]$reasons)
        if ($result -ne 0) { throw "RmGetList failed: $result" }
        for ($i = 0; $i -lt $count; $i++) {
            $pid2 = $info[$i].Process.dwProcessId
            $name = '?'
            try { $name = (Get-Process -Id $pid2 -ErrorAction Stop).ProcessName } catch { $name = 'exited' }
            Write-Host ("    locker: pid={0} process={1} app='{2}'" -f $pid2, $name, $info[$i].strAppName)
        }
        if ($count -eq 0) { Write-Host '    locker: none (file is free now)' }
    }
    finally {
        [void][RM]::RmEndSession($session)
    }
}

$qaId = [guid]::NewGuid().ToString()
$root = Join-Path $env:LOCALAPPDATA "desktop-todo-maintenance-qa\$qaId"
$install = Join-Path $root 'install'
New-Item -ItemType Directory -Path $install -Force | Out-Null
$payload = Join-Path $root 'payload'
New-Item -ItemType Directory -Path $payload -Force | Out-Null
Copy-Item -LiteralPath $helperExe -Destination (Join-Path $payload 'desktop-todo-maintenance.exe') -Force
Copy-Item -LiteralPath $productExe -Destination (Join-Path $payload 'desktop-todo-widget.exe') -Force
Set-Content -LiteralPath (Join-Path $payload 'README.md') -Value 'r' -Encoding ASCII
Set-Content -LiteralPath (Join-Path $payload 'LICENSE_ZH.md') -Value 'l' -Encoding ASCII

$env:DTW_MAINTENANCE_QA_ID = $qaId
function Invoke-Once {
    $p = Start-Process -FilePath (Join-Path $payload 'desktop-todo-maintenance.exe') `
        -ArgumentList @('--install') -WorkingDirectory $payload -Wait -PassThru -WindowStyle Hidden
    return $p.ExitCode
}

Write-Host "baseline install: $(Invoke-Once)"
$receiptPath = Join-Path $install 'installation-receipt.json'
$json = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
$json.lifecycleState = 'Installing'
[System.IO.File]::WriteAllText($receiptPath, ($json | ConvertTo-Json -Depth 10))
Remove-Item -LiteralPath (Join-Path $install 'desktop-todo-maintenance.exe') -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$qaId" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $root 'menu\desktop-todo-widget.lnk') -Force -ErrorAction SilentlyContinue
Write-Host "crash B retry: $(Invoke-Once)"

$json = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
$json.lifecycleState = 'Installing'
[System.IO.File]::WriteAllText($receiptPath, ($json | ConvertTo-Json -Depth 10))
Remove-Item -LiteralPath (Join-Path $root 'menu\desktop-todo-widget.lnk') -Force -ErrorAction SilentlyContinue
Write-Host 'crash C retry (registry kept):'
$code = Invoke-Once
Write-Host "  exit: $code"
Write-Host '  lockers of the main exe now:'
Get-FileLockers -Path (Join-Path $install 'desktop-todo-widget.exe')
if ($code -ne 0) {
    Start-Sleep -Seconds 2
    Write-Host '  lockers again after 2s:'
    Get-FileLockers -Path (Join-Path $install 'desktop-todo-widget.exe')
}
Remove-Item Env:\DTW_MAINTENANCE_QA_ID -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath "HKCU:\Software\desktop-todo-maintenance-qa\$qaId" -Recurse -Force -ErrorAction SilentlyContinue
