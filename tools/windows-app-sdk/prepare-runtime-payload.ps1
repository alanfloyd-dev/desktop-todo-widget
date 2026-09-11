[CmdletBinding()]
<#
.SYNOPSIS
    Stages a self-contained Windows App SDK runtime payload for the optional
    Windows CompositionController hosting path.

.DESCRIPTION
    Phase 7C.3-B2. The payload is assembled from official, pinned NuGet component
    packages, verified by SHA256, and emitted into a versioned directory so two
    runtimes can coexist for A/B comparison and rollback:

        tools/windows-app-sdk/runtime/1.8/x64/
        tools/windows-app-sdk/runtime/2.x/x64/
            *.dll, *.winmd, *.pri     runtime payload copied beside the exe
            composition-host.manifest generated WinRT activation fragment
        tools/windows-app-sdk/runtime/x64/     <- generated selector consumed by build.rs
            active-runtime.txt                 contains the active payload key

    The activation fragment is generated from the packages' own
    `package.appxfragment` files, so the activatable-class list always matches the
    pinned runtime. Nothing here is committed: the runtime directories are
    git-ignored build input.

.PARAMETER Runtime
    Which pinned payload set to stage. Use `-ListRuntimes` to print the choices.

.PARAMETER OutputRoot
    Override the output root. Defaults to `tools/windows-app-sdk/runtime`.

.PARAMETER Force
    Re-download the NuGet packages even if a cached copy exists.

.PARAMETER NoActivate
    Stage the payload but leave the build.rs selector untouched. Use this to stage
    a candidate runtime without switching the build onto it.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -ListRuntimes

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools/windows-app-sdk/prepare-runtime-payload.ps1 -Runtime 2.x
#>
param(
    [string]$Runtime = '1.8',
    [string]$OutputRoot,
    [switch]$Force,
    [switch]$NoActivate,
    [switch]$ListRuntimes
)

$ErrorActionPreference = 'Stop'

# --- Pinned runtime payload sets -------------------------------------------
# Each set lists the official component packages that make up the self-contained
# payload, with exact version + SHA256. `windows`/`interactive-experiences`/`base`
# are staged; `base` carries no native payload but is pinned because the umbrella
# package declares it as a dependency of the runtime.
#
# The resolved runtime version is what Microsoft's release lifecycle table calls
# the "latest patch version" for that release line.
# See docs/windows-app-sdk-runtime.md for the version/EOL policy.
$RuntimeSets = [ordered]@{
    '1.8' = @{
        Label            = 'Windows App SDK 1.8 (out of support 2026-09-09)'
        ResolvedRuntime  = '1.8.260804001'
        Foundation       = @{
            Name    = 'Microsoft.WindowsAppSDK.Foundation'
            Version = '1.8.260803002'
            Sha256  = 'B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E'
        }
        Interactive      = @{
            Name    = 'Microsoft.WindowsAppSDK.InteractiveExperiences'
            Version = '1.8.260708001'
            Sha256  = '496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76'
        }
    }
    '2.x' = @{
        Label            = 'Windows App SDK 2.4.0 (current stable, EOS 2027-04-29)'
        ResolvedRuntime  = '2.4.0'
        Foundation       = @{
            Name    = 'Microsoft.WindowsAppSDK.Foundation'
            Version = '2.3.9'
            Sha256  = '230BC605A3FC9ED689B2117056C5274923BF58B453FA44EDDE18A168BBF628BE'
        }
        Interactive      = @{
            Name    = 'Microsoft.WindowsAppSDK.InteractiveExperiences'
            Version = '2.1.6'
            Sha256  = 'DE7B5907C63C8A79606CCC8F0D98943B154A2E62312308187E8CDC3304FF3D0B'
        }
        Base             = @{
            Name    = 'Microsoft.WindowsAppSDK.Base'
            Version = '2.0.4'
            Sha256  = 'E3E13478C4C80C59ED5F8F89542FE49A2985DAA484753E93A5858E90C2D46A4D'
        }
        Umbrella         = @{
            Name    = 'Microsoft.WindowsAppSDK'
            Version = '2.4.0'
            Sha256  = '6EC2EBB6ADD33ECEBAC1F5773AD4CABE934B82FB18D7BEA98E011BB0FC0A37B9'
        }
    }
}

$RuntimeArchitecture = 'win-x64'
$SelectorKey = 'x64'

if ($ListRuntimes) {
    $RuntimeSets.GetEnumerator() | ForEach-Object {
        "{0,-5} {1}" -f $_.Key, $_.Value.Label
    }
    return
}

if (-not $RuntimeSets.Contains($Runtime)) {
    throw "Unknown runtime '$Runtime'. Known: $($RuntimeSets.Keys -join ', ')"
}
$set = $RuntimeSets[$Runtime]

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $OutputRoot) {
    $OutputRoot = Join-Path $PSScriptRoot 'runtime'
}
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
$payloadRoot = Join-Path (Join-Path $OutputRoot $Runtime) $SelectorKey

# Downloaded packages are cached outside the payload so a clean restage never has
# to re-download them.
$cacheRoot = Join-Path $PSScriptRoot 'cache'
$stageRoot = Join-Path $cacheRoot "stage-$($Runtime -replace '[^0-9A-Za-z]','_')"

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

# --- Stage the payload ------------------------------------------------------
Assert-ChildPath -Path $payloadRoot -Parent (Join-Path $OutputRoot $Runtime)
if (Test-Path -LiteralPath $payloadRoot) {
    Remove-Item -LiteralPath $payloadRoot -Recurse -Force
}
if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $payloadRoot, $stageRoot -Force | Out-Null

$components = @(
    @{ Key = 'foundation'; Package = $set.Foundation },
    @{ Key = 'interactive-experiences'; Package = $set.Interactive }
)
if ($set.ContainsKey('Base')) {
    # Pinned for completeness; carries no native payload in this deployment model.
    $null = Get-VerifiedPackage -Package $set.Base -Destination $cacheRoot
}
if ($set.ContainsKey('Umbrella')) {
    # Pinned for completeness; the umbrella package only declares dependencies.
    $null = Get-VerifiedPackage -Package $set.Umbrella -Destination $cacheRoot
}

foreach ($component in $components) {
    $package = Get-VerifiedPackage -Package $component.Package -Destination $cacheRoot
    $destination = Join-Path $stageRoot $component.Key
    Expand-ComponentPackage -Package $package -Destination $destination

    $nativeRoot = Join-Path $destination "runtimes-framework\$RuntimeArchitecture\native"
    if (-not (Test-Path -LiteralPath $nativeRoot)) {
        throw "Missing native runtime root for $($component.Package.Name) : $nativeRoot"
    }
    Copy-Item -Path (Join-Path $nativeRoot '*') -Destination $payloadRoot -Recurse -Force
    Get-ChildItem -Path (Join-Path $destination 'metadata') -Filter '*.winmd' -File -Recurse |
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
    #
    # The shape of package.appxfragment has been verified identical between the
    # 1.8 and 2.x component packages (Fragment > Extensions > Extension >
    # InProcessServer > Path/ActivatableClass), so one generator serves both.
    $writer.WriteStartElement('asmv3', 'files', 'urn:schemas-microsoft-com:asm.v3')
    foreach ($component in $components) {
        $fragmentPath = Join-Path $stageRoot "$($component.Key)\runtimes-framework\package.appxfragment"
        if (-not (Test-Path -LiteralPath $fragmentPath)) {
            throw "Missing package.appxfragment for $($component.Package.Name) : $fragmentPath"
        }
        Add-ManifestFragment -Fragment ([xml](Get-Content -Raw $fragmentPath)) -Writer $writer
    }
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

# --- Publish the selector consumed by build.rs ------------------------------
$selectorPath = Join-Path $OutputRoot 'active-runtime.txt'
$previous = if (Test-Path -LiteralPath $selectorPath) { (Get-Content -Raw $selectorPath).Trim() } else { '<none>' }
if (-not $NoActivate) {
    Set-Content -LiteralPath $selectorPath -Value $Runtime -NoNewline -Encoding ascii
}

$files = Get-ChildItem $payloadRoot -File
[pscustomobject]@{
    Runtime                 = $Runtime
    Label                   = $set.Label
    Deployment              = 'unpackaged-self-contained'
    Architecture            = $RuntimeArchitecture
    Foundation              = $set.Foundation.Version
    InteractiveExperiences  = $set.Interactive.Version
    ResolvedRuntimeVersion  = $set.ResolvedRuntime
    FileCount               = $files.Count
    DllCount                = @($files | Where-Object { $_.Extension -eq '.dll' }).Count
    SizeMB                  = [math]::Round((($files | Measure-Object Length -Sum).Sum / 1MB), 2)
    Manifest                = $manifestPath
    Output                  = $payloadRoot
    SelectorPrevious        = $previous
    SelectorActive          = if ($NoActivate) { "$previous (unchanged)" } else { $Runtime }
}
