[CmdletBinding()]
<#
.SYNOPSIS
    Stages the self-contained Windows App SDK runtime payload used by the
    optional Windows CompositionController hosting path.

.DESCRIPTION
    Phase 7C.3-A. This replaces the earlier spike shortcut that copied the
    payload out of the Phase 7C.0 PoC's `target/self-contained/dist` directory.

    The payload is assembled from the two official, pinned NuGet component
    packages that the Phase 7A/7C.0 proof already validated, verified by SHA256,
    and emitted into a single deterministic output directory:

        tools/windows-app-sdk/runtime/x64/
            *.dll, *.winmd, *.pri          runtime payload copied beside the exe
            composition-host.manifest      generated WinRT activation manifest

    The manifest is generated from the packages' own `package.appxfragment`
    files (exactly as the PoC did) instead of being maintained by hand, so the
    activatable-class list always matches the pinned runtime.

    Nothing here is committed: the output directory is git-ignored build input.

.PARAMETER OutputRoot
    Override the output root. Defaults to `tools/windows-app-sdk/runtime`.

.PARAMETER Force
    Re-download the NuGet packages even if a cached copy exists.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1
#>
param(
    [string]$OutputRoot,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# --- Pinned Windows App SDK deployment inputs -------------------------------
# These are the Microsoft Windows App SDK 1.8.11 component packages. The
# resolved runtime version is 1.8.260804001. See docs/windows-app-sdk-runtime.md
# for the version and end-of-support policy.
$FoundationPackage = @{
    Name    = 'Microsoft.WindowsAppSDK.Foundation'
    Version = '1.8.260803002'
    Sha256  = 'B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E'
}
$InteractivePackage = @{
    Name    = 'Microsoft.WindowsAppSDK.InteractiveExperiences'
    Version = '1.8.260708001'
    Sha256  = '496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76'
}
$RuntimeArchitecture = 'win-x64'
$ResolvedRuntimeVersion = '1.8.260804001'

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $OutputRoot) {
    $OutputRoot = Join-Path $PSScriptRoot 'runtime'
}
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$payloadRoot = Join-Path $OutputRoot 'x64'

# Downloaded packages are cached outside the payload so a clean rebuild of the
# payload never has to re-download them.
$cacheRoot = Join-Path $PSScriptRoot 'cache'

function Assert-ChildPath {
    param([string]$Path, [string]$Parent)

    $resolvedParent = [IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    $resolvedPath = [IO.Path]::GetFullPath($Path)
    if (-not $resolvedPath.StartsWith($resolvedParent, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside $resolvedParent : $resolvedPath"
    }
}

function Get-VerifiedPackage {
    param([hashtable]$Package, [string]$Destination)

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    $fileName = "$($Package.Name.ToLowerInvariant()).$($Package.Version).nupkg"
    $path = Join-Path $Destination $fileName
    if ($Force -or -not (Test-Path -LiteralPath $path)) {
        $uri = "https://api.nuget.org/v3-flatcontainer/$($Package.Name.ToLowerInvariant())/$($Package.Version)/$fileName"
        Write-Host "Downloading $fileName"
        Invoke-WebRequest -Uri $uri -OutFile $path
    }
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
    if ($actualHash -ne $Package.Sha256) {
        throw "SHA256 mismatch for $fileName : expected $($Package.Sha256) but got $actualHash"
    }
    return $path
}

function Expand-ComponentPackage {
    param([string]$Package, [string]$Destination)

    if (Test-Path -LiteralPath $Destination) {
        Remove-Item -LiteralPath $Destination -Recurse -Force
    }
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

# --- Build the payload ------------------------------------------------------
Assert-ChildPath -Path $payloadRoot -Parent $OutputRoot
if (Test-Path -LiteralPath $payloadRoot) {
    Remove-Item -LiteralPath $payloadRoot -Recurse -Force
}
$stageRoot = Join-Path $OutputRoot 'stage'
if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $payloadRoot, $stageRoot -Force | Out-Null

$foundationPackage = Get-VerifiedPackage -Package $FoundationPackage -Destination $cacheRoot
$interactivePackage = Get-VerifiedPackage -Package $InteractivePackage -Destination $cacheRoot

$foundationStage = Join-Path $stageRoot 'foundation'
$interactiveStage = Join-Path $stageRoot 'interactive-experiences'
Expand-ComponentPackage -Package $foundationPackage -Destination $foundationStage
Expand-ComponentPackage -Package $interactivePackage -Destination $interactiveStage

foreach ($component in @($foundationStage, $interactiveStage)) {
    $nativeRoot = Join-Path $component "runtimes-framework\$RuntimeArchitecture\native"
    if (-not (Test-Path -LiteralPath $nativeRoot)) {
        throw "Missing native runtime root in $component : $nativeRoot"
    }
    Copy-Item -Path (Join-Path $nativeRoot '*') -Destination $payloadRoot -Recurse -Force
    Get-ChildItem -Path (Join-Path $component 'metadata') -Filter '*.winmd' -File -Recurse |
        Copy-Item -Destination $payloadRoot -Force
}

$manifestPath = Join-Path $payloadRoot 'composition-host.manifest'
$settings = New-Object System.Xml.XmlWriterSettings
$settings.Indent = $true
$settings.Encoding = New-Object System.Text.UTF8Encoding($false)
$settings.OmitXmlDeclaration = $true
$writer = [System.Xml.XmlWriter]::Create($manifestPath, $settings)
try {
    # This file is an XML *fragment*: only the <asmv3:file> activation blocks,
    # with no XML declaration and no root <assembly> element. build.rs splices
    # these elements into the base application manifest. Emitting a fragment
    # (instead of a second complete manifest) is what makes the merge a real XML
    # operation rather than a text concatenation.
    $writer.WriteStartElement('asmv3', 'files', 'urn:schemas-microsoft-com:asm.v3')
    Add-ManifestFragment -Fragment ([xml](Get-Content -Raw (Join-Path $foundationStage "runtimes-framework\package.appxfragment"))) -Writer $writer
    Add-ManifestFragment -Fragment ([xml](Get-Content -Raw (Join-Path $interactiveStage "runtimes-framework\package.appxfragment"))) -Writer $writer
    $writer.WriteEndElement()
    $writer.WriteEndDocument()
}
finally {
    $writer.Dispose()
}

Remove-Item -LiteralPath $stageRoot -Recurse -Force

# Fail loudly if anything shipped unsigned: this payload is loaded into the
# product process, so every DLL must carry a valid Microsoft signature.
$invalidSignatures = Get-ChildItem $payloadRoot -Filter '*.dll' -File |
    Where-Object { (Get-AuthenticodeSignature -LiteralPath $_.FullName).Status -ne 'Valid' }
if ($invalidSignatures) {
    throw "Unsigned or invalid runtime DLLs: $($invalidSignatures.Name -join ', ')"
}

$files = Get-ChildItem $payloadRoot -File
[pscustomobject]@{
    Deployment              = 'unpackaged-self-contained'
    Architecture            = $RuntimeArchitecture
    Foundation              = $FoundationPackage.Version
    InteractiveExperiences  = $InteractivePackage.Version
    ResolvedRuntimeVersion  = $ResolvedRuntimeVersion
    FileCount               = $files.Count
    DllCount                = @($files | Where-Object { $_.Extension -eq '.dll' }).Count
    Manifest                = $manifestPath
    Output                  = $payloadRoot
}
