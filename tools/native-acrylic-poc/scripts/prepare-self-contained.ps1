[CmdletBinding()]
param(
    [ValidatePattern('^[a-z0-9][a-z0-9-]{0,39}$')]
    [string]$OutputSlot = 'self-contained'
)

$ErrorActionPreference = 'Stop'

$pocRoot = Split-Path -Parent $PSScriptRoot
$targetRoot = Join-Path $pocRoot 'target'
$selfContainedRoot = Join-Path $targetRoot $OutputSlot
$cargoTarget = Join-Path $selfContainedRoot 'cargo'
$stageRoot = Join-Path $selfContainedRoot 'stage'
$distRoot = Join-Path $selfContainedRoot 'dist'

function Assert-ChildPath {
    param([string]$Path, [string]$Parent)

    $resolvedParent = [IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    $resolvedPath = [IO.Path]::GetFullPath($Path)
    if (-not $resolvedPath.StartsWith($resolvedParent, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside the PoC target directory: $resolvedPath"
    }
}

function Get-VerifiedPackage {
    param(
        [string]$Name,
        [string]$Version,
        [string]$Sha256
    )

    $fileName = "$($Name.ToLowerInvariant()).$Version.nupkg"
    $path = Join-Path $targetRoot $fileName
    if (-not (Test-Path -LiteralPath $path)) {
        $uri = "https://api.nuget.org/v3-flatcontainer/$($Name.ToLowerInvariant())/$Version/$fileName"
        Invoke-WebRequest -Uri $uri -OutFile $path
    }
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
    if ($actualHash -ne $Sha256) {
        throw "SHA256 mismatch for $fileName"
    }
    return $path
}

function Expand-ComponentPackage {
    param([string]$Package, [string]$Destination)

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    & tar.exe -xf $Package -C $Destination
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to extract $Package"
    }
}

function Add-ManifestFragment {
    param(
        [xml]$Fragment,
        [System.Xml.XmlWriter]$Writer
    )

    $namespace = New-Object System.Xml.XmlNamespaceManager($Fragment.NameTable)
    $namespace.AddNamespace('f', 'http://schemas.microsoft.com/appx/manifest/foundation/windows10')
    foreach ($server in $Fragment.SelectNodes('//f:InProcessServer', $namespace)) {
        $pathNode = $server.SelectSingleNode('f:Path', $namespace)
        if ($null -eq $pathNode) { continue }
        $Writer.WriteStartElement('asmv3', 'file', 'urn:schemas-microsoft-com:asm.v3')
        $Writer.WriteAttributeString('name', $pathNode.InnerText)
        foreach ($class in $server.SelectNodes('f:ActivatableClass', $namespace)) {
            $Writer.WriteStartElement('winrtv1', 'activatableClass', 'urn:schemas-microsoft-com:winrt.v1')
            $Writer.WriteAttributeString('name', $class.ActivatableClassId)
            $Writer.WriteAttributeString('threadingModel', $class.ThreadingModel)
            $Writer.WriteEndElement()
        }
        $Writer.WriteEndElement()
    }
}

foreach ($path in @($stageRoot, $distRoot)) {
    Assert-ChildPath -Path $path -Parent $targetRoot
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Recurse -Force
    }
}
New-Item -ItemType Directory -Path $stageRoot, $distRoot -Force | Out-Null

$foundationPackage = Get-VerifiedPackage `
    -Name 'Microsoft.WindowsAppSDK.Foundation' `
    -Version '1.8.260803002' `
    -Sha256 'B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E'
$interactivePackage = Get-VerifiedPackage `
    -Name 'Microsoft.WindowsAppSDK.InteractiveExperiences' `
    -Version '1.8.260708001' `
    -Sha256 '496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76'

$foundationStage = Join-Path $stageRoot 'foundation'
$interactiveStage = Join-Path $stageRoot 'interactive-experiences'
Expand-ComponentPackage -Package $foundationPackage -Destination $foundationStage
Expand-ComponentPackage -Package $interactivePackage -Destination $interactiveStage

foreach ($component in @($foundationStage, $interactiveStage)) {
    $nativeRoot = Join-Path $component 'runtimes-framework\win-x64\native'
    Copy-Item -Path (Join-Path $nativeRoot '*') -Destination $distRoot -Recurse -Force
    Get-ChildItem -Path (Join-Path $component 'metadata') -Filter '*.winmd' -File -Recurse |
        Copy-Item -Destination $distRoot -Force
}

$manifestPath = Join-Path $selfContainedRoot 'native-acrylic-poc.manifest'
$settings = New-Object System.Xml.XmlWriterSettings
$settings.Indent = $true
$settings.Encoding = New-Object System.Text.UTF8Encoding($false)
$writer = [System.Xml.XmlWriter]::Create($manifestPath, $settings)
try {
    $writer.WriteStartDocument()
    $writer.WriteStartElement('assembly', 'urn:schemas-microsoft-com:asm.v1')
    $writer.WriteAttributeString('manifestVersion', '1.0')
    $writer.WriteAttributeString('xmlns', 'asmv3', $null, 'urn:schemas-microsoft-com:asm.v3')
    $writer.WriteAttributeString('xmlns', 'winrtv1', $null, 'urn:schemas-microsoft-com:winrt.v1')
    Add-ManifestFragment -Fragment ([xml](Get-Content -Raw (Join-Path $foundationStage 'runtimes-framework\package.appxfragment'))) -Writer $writer
    Add-ManifestFragment -Fragment ([xml](Get-Content -Raw (Join-Path $interactiveStage 'runtimes-framework\package.appxfragment'))) -Writer $writer
    $writer.WriteEndElement()
    $writer.WriteEndDocument()
}
finally {
    $writer.Dispose()
}

$env:CARGO_TARGET_DIR = $cargoTarget
$separator = [char]0x1f
$env:CARGO_ENCODED_RUSTFLAGS = @(
    '-Clink-arg=/MANIFEST:EMBED',
    "-Clink-arg=/MANIFESTINPUT:$manifestPath"
) -join $separator
& cargo.exe build --manifest-path (Join-Path $pocRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$executable = Join-Path $distRoot 'native-acrylic-poc.exe'
Copy-Item -LiteralPath (Join-Path $cargoTarget 'debug\native-acrylic-poc.exe') -Destination $executable -Force

$invalidSignatures = Get-ChildItem $distRoot -Filter '*.dll' -File |
    Where-Object { (Get-AuthenticodeSignature -LiteralPath $_.FullName).Status -ne 'Valid' }
if ($invalidSignatures) {
    throw "Unsigned or invalid runtime DLLs: $($invalidSignatures.Name -join ', ')"
}

[pscustomobject]@{
    Deployment = 'unpackaged-self-contained'
    Architecture = 'x64'
    Foundation = '1.8.260803002'
    InteractiveExperiences = '1.8.260708001'
    RuntimeFileCount = @(Get-ChildItem $distRoot -File).Count
    Output = $distRoot
}
