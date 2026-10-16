# Minimal PE import-table reader.
#
# Answers "which DLLs does this executable actually import?" from the file itself,
# instead of inferring dependencies from filenames in the output directory. Used
# to decide what a distribution archive must contain beside alan-desktop.exe.
param(
  [Parameter(Mandatory = $true)][string]$Path,
  [switch]$Full
)

$bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $Path))

function Read-U16([int]$offset) { [System.BitConverter]::ToUInt16($bytes, $offset) }
function Read-U32([int]$offset) { [System.BitConverter]::ToUInt32($bytes, $offset) }

$peOffset = [int](Read-U32 0x3C)
if ($bytes[$peOffset] -ne 0x50 -or $bytes[$peOffset + 1] -ne 0x45) {
  throw "not a PE file: $Path"
}
$coff = $peOffset + 4
$sectionCount = [int](Read-U16 ($coff + 2))
$optionalSize = [int](Read-U16 ($coff + 16))
$optional = $coff + 20
$magic = Read-U16 $optional
if ($magic -ne 0x20B) { throw ("unexpected PE magic 0x{0:X} (expected PE32+ 0x20B)" -f $magic) }

# Data directories start at optional+112 for PE32+; entry 1 is the import table.
$importRva = [int](Read-U32 ($optional + 112 + 8))
$importSize = [int](Read-U32 ($optional + 112 + 12))

# Section table follows the optional header.
$sectionTable = $optional + $optionalSize
$sections = @()
for ($i = 0; $i -lt $sectionCount; $i++) {
  $s = $sectionTable + ($i * 40)
  $sections += [pscustomobject]@{
    Name    = ([System.Text.Encoding]::ASCII.GetString($bytes, $s, 8)).Trim([char]0)
    Virtual = [int](Read-U32 ($s + 12))
    Size    = [int](Read-U32 ($s + 8))
    RawPtr  = [int](Read-U32 ($s + 20))
  }
}

function Convert-RvaToOffset([int]$rva) {
  foreach ($section in $sections) {
    if ($rva -ge $section.Virtual -and $rva -lt ($section.Virtual + $section.Size)) {
      return $section.RawPtr + ($rva - $section.Virtual)
    }
  }
  return -1
}

Write-Output "file            : $((Resolve-Path $Path).Path)"
Write-Output "machine         : 0x$('{0:X4}' -f (Read-U16 ($coff + 0)))"
Write-Output "sections        : $($sections.Name -join ', ')"
Write-Output "import dir RVA  : 0x$('{0:X}' -f $importRva) (size $importSize)"
Write-Output '--- imported DLLs (static import table) ---'

$descriptor = Convert-RvaToOffset $importRva
$names = @()
if ($descriptor -lt 0) {
  Write-Output '(no import directory)'
} else {
  while ($true) {
    $originalFirstThunk = Read-U32 $descriptor
    $nameRva = Read-U32 ($descriptor + 12)
    $firstThunk = Read-U32 ($descriptor + 16)
    if ($originalFirstThunk -eq 0 -and $nameRva -eq 0 -and $firstThunk -eq 0) { break }
    $nameOffset = Convert-RvaToOffset ([int]$nameRva)
    if ($nameOffset -ge 0) {
      $end = $nameOffset
      while ($bytes[$end] -ne 0) { $end++ }
      $name = [System.Text.Encoding]::ASCII.GetString($bytes, $nameOffset, $end - $nameOffset)
      $names += $name
      Write-Output "  $name"
    }
    $descriptor += 20
  }
}
Write-Output "imported DLL count: $($names.Count)"
