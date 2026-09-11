<#
.SYNOPSIS
    Build the Windows MSI installer for Subordinate with the GStreamer runtime
    bundled (TASK-104).

.DESCRIPTION
    Windows counterpart of packaging/linux/build-appimage.sh. The MSI installs
    a self-contained tree under %ProgramFiles%\Subordinate:

        bin\subordinate.exe, subordinate-cli.exe
        bin\*.dll                      the GStreamer 1.28 MSVC runtime
        bin\gst-inspect-1.0.exe, gst-discoverer-1.0.exe, gst-launch-1.0.exe
        bin\vcruntime140.dll, msvcp140.dll, ...   the MSVC CRT, app-local
        lib\gstreamer-1.0\gst*.dll     the plugins in gst-plugins.txt
        libexec\gstreamer-1.0\gst-plugin-scanner.exe

    so a machine with nothing but a graphics driver can install it and export.
    The layout is a GStreamer prefix on purpose -- see the comment at the top
    of main.wxs for why that removes the need for any environment variable.

    WHY WiX DIRECTLY AND NOT cargo-wix
    cargo-wix drives the WiX v3 toolset (candle.exe/light.exe), which is
    end-of-life and is not installed on the windows-latest hosted image, and it
    harvests one crate's own binaries -- it has nothing to say about a few
    hundred files of third-party runtime, which is the actual work here. The
    WiX v5 toolset installs as a .NET global tool in seconds on any runner and
    is pinned like any other dependency. So: `dotnet tool install --global wix`,
    a hand-written main.wxs for the package shape, and a payload fragment this
    script generates from the staged tree. Nothing about the result is
    cargo-wix-shaped; an MSI is an MSI.

    WHY THE PLUGINS ARE CURATED AND THE DLLs ARE NOT
    gst-plugins.txt is an allowlist: the official installer ships ~200 plugin
    modules, Subordinate loads a few dozen, and a missing hardware plugin has
    to be a build failure rather than a silent fallback to software. The DLLs
    in bin\ are copied wholesale instead. Curating them means a transitive
    dependency walk over PE imports, which is both fragile (plugins load some
    of their dependencies at runtime, where an import table does not see them)
    and worth little: the cabinet is compressed and the unused DLLs are a
    fraction of the installed size next to the plugins and the CRT.

.PARAMETER GstRoot
    The GStreamer MSVC prefix to bundle. Defaults to
    $env:GSTREAMER_1_0_ROOT_MSVC_X86_64, which the official installer and
    .github/workflows/ci.yml both set.

.PARAMETER CargoProfile
    Cargo profile whose binaries are packaged. Default `release`.

.PARAMETER OutDir
    Where the MSI is written. Default <repo>\target\wix.

.PARAMETER SkipBuild
    Package the binaries already in target\<profile> instead of running cargo.

.PARAMETER StageOnly
    Stage the tree and generate the payload fragment, then stop without
    invoking WiX. Useful for inspecting what would ship.
#>
[CmdletBinding()]
param(
    [string]$GstRoot,
    [string]$CargoProfile = 'release',
    [string]$OutDir,
    [switch]$SkipBuild,
    [switch]$StageOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$windowsDir = $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'target\wix' }
if (-not $GstRoot) { $GstRoot = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 }

function Fail { param([string]$Message) throw "build-msi: $Message" }

if (-not $GstRoot) {
    Fail 'no GStreamer prefix: pass -GstRoot or set GSTREAMER_1_0_ROOT_MSVC_X86_64'
}
$GstRoot = (Resolve-Path -LiteralPath $GstRoot).Path
foreach ($sub in 'bin', 'lib\gstreamer-1.0', 'libexec\gstreamer-1.0') {
    if (-not (Test-Path -LiteralPath (Join-Path $GstRoot $sub))) {
        Fail "$GstRoot does not look like a GStreamer prefix: no $sub"
    }
}

# The version the package carries is the workspace version, so the MSI, the
# AppImage and the Flatpak can never disagree about what release they are.
$cargoToml = Get-Content -Raw -LiteralPath (Join-Path $repoRoot 'Cargo.toml')
if ($cargoToml -match '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    $version = $Matches[1]
}
else {
    Fail 'cannot read the workspace version from Cargo.toml'
}
Write-Host "build-msi: Subordinate $version"

# MSI ProductVersion is major.minor.build with each field bounded; a
# pre-release suffix like 0.2.0-rc1 is not representable, so it is dropped for
# the installer's own version field only.
$msiVersion = ($version -split '[-+]')[0]
if ($msiVersion -notmatch '^\d+\.\d+(\.\d+)?$') { Fail "version '$version' is not MSI-representable" }

# ------------------------------------------------------------------- binaries
$targetDir = Join-Path $repoRoot "target\$CargoProfile"
$exeNames = @('subordinate.exe', 'subordinate-cli.exe')
if (-not $SkipBuild) {
    Write-Host "build-msi: cargo build --profile $CargoProfile -p subordinate -p subordinate-cli"
    & cargo build --profile $CargoProfile -p subordinate -p subordinate-cli
    if ($LASTEXITCODE -ne 0) { Fail "cargo build failed with exit code $LASTEXITCODE" }
}
foreach ($exe in $exeNames) {
    if (-not (Test-Path -LiteralPath (Join-Path $targetDir $exe))) {
        Fail "$targetDir\$exe does not exist; build it or drop -SkipBuild"
    }
}

# --------------------------------------------------------------------- stage
$stage = Join-Path $repoRoot 'target\wix\stage'
if (Test-Path -LiteralPath $stage) { Remove-Item -Recurse -Force -LiteralPath $stage }
$stageBin = Join-Path $stage 'bin'
$stagePlugins = Join-Path $stage 'lib\gstreamer-1.0'
$stageLibexec = Join-Path $stage 'libexec\gstreamer-1.0'
foreach ($dir in $stage, $stageBin, $stagePlugins, $stageLibexec) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

foreach ($exe in $exeNames) {
    Copy-Item -LiteralPath (Join-Path $targetDir $exe) -Destination $stageBin
}

# The whole runtime DLL set, then the GStreamer command-line tools. The tools
# are 300 KB together and they are how a user -- or the packaging job -- asks
# the *installed* runtime what it can do, instead of asking some other
# GStreamer that happens to be on PATH.
Copy-Item -Path (Join-Path $GstRoot 'bin\*.dll') -Destination $stageBin
foreach ($tool in 'gst-inspect-1.0.exe', 'gst-discoverer-1.0.exe', 'gst-launch-1.0.exe') {
    $path = Join-Path $GstRoot "bin\$tool"
    if (Test-Path -LiteralPath $path) { Copy-Item -LiteralPath $path -Destination $stageBin }
    else { Fail "the GStreamer prefix has no $tool" }
}

$scanner = Join-Path $GstRoot 'libexec\gstreamer-1.0\gst-plugin-scanner.exe'
if (-not (Test-Path -LiteralPath $scanner)) { Fail 'the GStreamer prefix has no gst-plugin-scanner.exe' }
Copy-Item -LiteralPath $scanner -Destination $stageLibexec

# ------------------------------------------------------------------- plugins
$listPath = Join-Path $windowsDir 'gst-plugins.txt'
$required = [System.Collections.Generic.List[string]]::new()
$wanted = [System.Collections.Generic.List[string]]::new()
foreach ($line in Get-Content -LiteralPath $listPath) {
    $entry = ($line -replace '#.*', '').Trim()
    if (-not $entry) { continue }
    if ($entry -notmatch '^!?[a-z0-9_]+$') { Fail "malformed entry in gst-plugins.txt: '$entry'" }
    if ($entry.StartsWith('!')) { $entry = $entry.Substring(1); $required.Add($entry) }
    if ($wanted -contains $entry) { Fail "duplicate entry in gst-plugins.txt: '$entry'" }
    $wanted.Add($entry)
}

$missingRequired = [System.Collections.Generic.List[string]]::new()
$absent = [System.Collections.Generic.List[string]]::new()
$copied = 0
foreach ($plugin in $wanted) {
    $dll = Join-Path $GstRoot "lib\gstreamer-1.0\gst$plugin.dll"
    if (Test-Path -LiteralPath $dll) {
        Copy-Item -LiteralPath $dll -Destination $stagePlugins
        $copied++
    }
    elseif ($required -contains $plugin) { $missingRequired.Add($plugin) }
    else { $absent.Add($plugin) }
}
if ($absent.Count) { Write-Host "build-msi: optional plugins not in this runtime: $($absent -join ', ')" }
if ($missingRequired.Count) {
    Fail ("this GStreamer runtime is missing required plugins: $($missingRequired -join ', '). " +
        'Install the full MSVC package (/TYPE=devel or the complete runtime) rather than a minimal one.')
}
Write-Host "build-msi: bundled $copied plugin modules, all $($required.Count) required ones present"

# ----------------------------------------------------------------- MSVC CRT
# Deployed app-local rather than through the VC redistributable: the redist is
# a second installer to chain, and app-local CRT deployment is supported by
# Microsoft for exactly this. The UCRT itself is part of Windows and is never
# bundled.
$crtNames = @(
    'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll',
    'msvcp140_1.dll', 'msvcp140_2.dll', 'msvcp140_codecvt_ids.dll', 'concrt140.dll'
)
$crtDirs = @()
if ($env:VCToolsRedistDir) {
    $crtDirs = @(Get-ChildItem -Path (Join-Path $env:VCToolsRedistDir 'x64\Microsoft.VC*.CRT') -Directory -ErrorAction SilentlyContinue)
}
if ($crtDirs.Count -eq 0) {
    $crtDirs = @(Get-ChildItem -Path 'C:\Program Files*\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT' -Directory -ErrorAction SilentlyContinue)
}
if ($crtDirs.Count -eq 0) { Fail 'cannot find the MSVC CRT redistributable directory' }
$crtDir = ($crtDirs | Sort-Object FullName | Select-Object -Last 1).FullName
$crtCopied = 0
foreach ($name in $crtNames) {
    $path = Join-Path $crtDir $name
    if (Test-Path -LiteralPath $path) { Copy-Item -LiteralPath $path -Destination $stageBin -Force; $crtCopied++ }
}
if ($crtCopied -eq 0) { Fail "no CRT DLLs found in $crtDir" }
Write-Host "build-msi: bundled $crtCopied MSVC CRT DLLs from $crtDir"

# -------------------------------------------------------------- top-level docs
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md') -Destination $stage

# ------------------------------------------------------- payload WiX fragment
# One component per file, which is what Windows Installer wants: a component
# with a single file keypath can have its GUID derived by WiX, is patchable on
# its own, and is reference-counted correctly if a future package ever shares
# the runtime. Ids have to be valid MSI identifiers and unique, so each is the
# sanitised file name plus a hash of its path within the stage.
$groups = [ordered]@{
    FilesRoot        = @{ Dir = 'INSTALLFOLDER'; Path = $stage}
    FilesBin         = @{ Dir = 'BINFOLDER'; Path = $stageBin}
    FilesGstPlugins  = @{ Dir = 'GSTPLUGINFOLDER'; Path = $stagePlugins}
    FilesGstLibexec  = @{ Dir = 'GSTLIBEXECFOLDER'; Path = $stageLibexec}
}

$sha = [System.Security.Cryptography.SHA256]::Create()
function Get-WixId {
    param([string]$Prefix, [string]$Key)
    $bytes = $sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($Key))
    $hash = ([System.BitConverter]::ToString($bytes) -replace '-', '').Substring(0, 12)
    $safe = ($Key -replace '[^A-Za-z0-9_.]', '_')
    if ($safe.Length -gt 40) { $safe = $safe.Substring($safe.Length - 40) }
    "${Prefix}_${safe}_$hash"
}

$xml = [System.Collections.Generic.List[string]]::new()
$xml.Add('<?xml version="1.0" encoding="utf-8"?>')
$xml.Add('<!-- Generated by packaging/windows/build-msi.ps1. Do not edit. -->')
$xml.Add('<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">')
$xml.Add('  <Fragment>')
$fileCount = 0
$payloadBytes = 0L
foreach ($groupId in $groups.Keys) {
    $group = $groups[$groupId]
    $xml.Add("    <ComponentGroup Id=""$groupId"" Directory=""$($group.Dir)"">")
    $files = Get-ChildItem -LiteralPath $group.Path -File | Sort-Object Name
    foreach ($file in $files) {
        $key = "$groupId/$($file.Name)"
        $componentId = Get-WixId -Prefix 'c' -Key $key
        $fileId = Get-WixId -Prefix 'f' -Key $key
        $source = [System.Security.SecurityElement]::Escape($file.FullName)
        $xml.Add("      <Component Id=""$componentId"">")
        $xml.Add("        <File Id=""$fileId"" Source=""$source"" KeyPath=""yes"" />")
        $xml.Add('      </Component>')
        $fileCount++
        $payloadBytes += $file.Length
    }
    $xml.Add('    </ComponentGroup>')
}
$xml.Add('  </Fragment>')
$xml.Add('</Wix>')

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$payloadWxs = Join-Path $OutDir 'payload.wxs'
Set-Content -LiteralPath $payloadWxs -Value $xml -Encoding utf8
$payloadMiB = [math]::Round($payloadBytes / 1MB, 1)
Write-Host "build-msi: staged $fileCount files, $payloadMiB MiB uncompressed -> $payloadWxs"

if ($StageOnly) {
    Write-Host "build-msi: -StageOnly, stopping with the tree staged in $stage"
    exit 0
}

# ----------------------------------------------------------------- WiX build
# Pinned like any other dependency. `wix` is a .NET global tool; the install is
# a no-op when the right version is already there.
$wixVersion = '5.0.2'
$wix = Get-Command wix -ErrorAction SilentlyContinue
if (-not $wix) {
    Write-Host "build-msi: installing the WiX $wixVersion toolset"
    & dotnet tool install --global wix --version $wixVersion
    if ($LASTEXITCODE -ne 0) { Fail 'dotnet tool install wix failed' }
    $env:PATH = "$env:USERPROFILE\.dotnet\tools;$env:PATH"
    $wix = Get-Command wix -ErrorAction SilentlyContinue
    if (-not $wix) { Fail 'wix is still not on PATH after installing it' }
}
Write-Host "build-msi: $(& wix --version)"

$msi = Join-Path $OutDir "Subordinate-$version-x86_64.msi"
& wix build -arch x64 -d "Version=$msiVersion" `
    -out $msi (Join-Path $windowsDir 'main.wxs') $payloadWxs
if ($LASTEXITCODE -ne 0) { Fail "wix build failed with exit code $LASTEXITCODE" }

$msiMiB = [math]::Round((Get-Item -LiteralPath $msi).Length / 1MB, 1)
Write-Host "build-msi: wrote $msi ($msiMiB MiB)"
