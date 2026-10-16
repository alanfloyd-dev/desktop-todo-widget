# Restart-flow runner for the Floating Orb geometry diagnosis.
#
# Writes a chosen persisted state, launches the product, polls the native window
# rect for a while (so a late overwrite cannot hide), captures the window, and
# prints the geometry log lines that ran.
param(
  [Parameter(Mandatory = $true)][string]$Tag,
  [Parameter(Mandatory = $true)][string]$SettingsJson,
  [string]$Backend = 'standard',
  [int]$Seconds = 16
)

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ev = Join-Path $root '.rc0-evidence\restart-diag'
New-Item -ItemType Directory -Force -Path $ev | Out-Null

Add-Type @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class Rp {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int m);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows(delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); EnumChildWindows(GetDesktopWindow(), delegate(IntPtr h, IntPtr p) { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, sb.Capacity); return sb.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Rect(IntPtr h) { R r; GetWindowRect(h, out r); return string.Format("({0},{1}) {2}x{3}", r.L, r.T, r.Rr - r.L, r.B - r.T); }
}
"@
[void][Rp]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))

function Find-Product {
  $ids = @((Get-Process -Name alan-desktop -ErrorAction SilentlyContinue).Id)
  if (-not $ids) { return [IntPtr]::Zero }
  foreach ($h in [Rp]::All()) {
    if ($ids -contains [Rp]::Pid($h) -and [Rp]::Cls($h) -eq 'Tauri Window') { return $h }
  }
  return [IntPtr]::Zero
}

# --- write the requested persisted state -------------------------------------
$settingsPath = Join-Path $ev "settings-$Tag.json"
[System.IO.File]::WriteAllText($settingsPath, $SettingsJson)
python (Join-Path $root 'scripts\rc0-qa\v1-desktop-settings.py') write $settingsPath | Out-Null
Write-Host "[$Tag] persisted state written"

Get-Process alan-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

$err = Join-Path $ev "run-$Tag.err.txt"
$out = Join-Path $ev "run-$Tag.out.txt"
if ($Backend -eq 'enhanced') {
  $proc = Start-Process -FilePath (Join-Path $root 'src-tauri\target\debug\alan-desktop.exe') -ArgumentList '--qa-composition-controller' -WorkingDirectory $root -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
} else {
  $proc = Start-Process -FilePath (Join-Path $root 'src-tauri\target\debug\alan-desktop.exe') -WorkingDirectory $root -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
}
Write-Host "[$Tag] launched pid $($proc.Id) backend=$Backend"

$seen = @{}
for ($i = 0; $i -lt $Seconds; $i++) {
  Start-Sleep -Seconds 1
  $h = Find-Product
  if ($h -ne [IntPtr]::Zero) {
    $line = "[$Tag] t+$($i+1)s rect=$([Rp]::Rect($h))"
    if (-not $seen.ContainsKey($line)) { $seen[$line] = $true; Write-Host $line }
  }
}
Write-Host "[$Tag] distinct rect states: $($seen.Count)"

& (Join-Path $root 'scripts\rc0-qa\capture-product-window.ps1') -Out (Join-Path $ev "shot-$Tag.png") | Out-Null
Write-Host "[$Tag] --- geometry log ---"
Select-String -Path $err -Pattern 'floating-orb|window-geometry|desktop-lifecycle\] mode transition|attach succeeded|detach|mode transition' | ForEach-Object { $_.Line } | Select-Object -First 8
