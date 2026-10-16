# Compare two PNG captures pixel-by-pixel and report the difference.
#
# Used to answer "is this surface actually painting?" without needing to see the
# image: a capture of a live, interactive surface changes when the cursor enters
# it, while a stale or empty surface stays byte-identical.
param(
  [Parameter(Mandatory = $true)][string]$A,
  [Parameter(Mandatory = $true)][string]$B
)

Add-Type -AssemblyName System.Drawing
$imgA = [System.Drawing.Bitmap]::FromFile((Resolve-Path $A))
$imgB = [System.Drawing.Bitmap]::FromFile((Resolve-Path $B))
Write-Output "A: $($imgA.Width)x$($imgA.Height)  B: $($imgB.Width)x$($imgB.Height)"
if ($imgA.Width -ne $imgB.Width -or $imgA.Height -ne $imgB.Height) {
  Write-Output "different dimensions -> surfaces differ"
  $imgA.Dispose(); $imgB.Dispose(); exit 0
}
$different = 0
$total = $imgA.Width * $imgA.Height
for ($y = 0; $y -lt $imgA.Height; $y++) {
  for ($x = 0; $x -lt $imgA.Width; $x++) {
    $pa = $imgA.GetPixel($x, $y)
    $pb = $imgB.GetPixel($x, $y)
    if ($pa.ToArgb() -ne $pb.ToArgb()) { $different++ }
  }
}
$imgA.Dispose(); $imgB.Dispose()
Write-Output "different pixels: $different / $total ($([math]::Round(100.0 * $different / $total, 2))%)"
