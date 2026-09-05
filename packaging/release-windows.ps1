$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Version = if ($args.Count) { $args[0] } else {
    (Select-String -Path "$Root\Cargo.toml" -Pattern '^version = "(.+)"$').Matches[0].Groups[1].Value
}
$Arch = switch ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    "X64" { "x86_64" }
    "Arm64" { "arm64" }
    default { throw "Unsupported architecture: $([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)" }
}
$Platform = if ($Arch -eq "arm64") { "ARM64" } else { "x64" }
$Rid = if ($Arch -eq "arm64") { "win-arm64" } else { "win-x64" }
$Name = "qfind-$Version-windows-$Arch"
$Stage = "$Root\target\dist\$Name"

$VcpkgRoot = $env:VCPKG_ROOT
if ([string]::IsNullOrWhiteSpace($VcpkgRoot)) { $VcpkgRoot = $env:VCPKG_INSTALLATION_ROOT }
if ([string]::IsNullOrWhiteSpace($VcpkgRoot)) { $VcpkgRoot = "C:\vcpkg" }
if (-not (Test-Path "$VcpkgRoot\vcpkg.exe")) {
    throw "vcpkg was not found. Set VCPKG_ROOT (or VCPKG_INSTALLATION_ROOT on GitHub-hosted runners) to a bootstrapped vcpkg checkout."
}
$VcpkgRoot = (Resolve-Path $VcpkgRoot).Path
$env:VCPKG_ROOT = $VcpkgRoot

$Dynamic = -not [string]::IsNullOrWhiteSpace($env:VCPKGRS_DYNAMIC)
$Triplet = $env:VCPKGRS_TRIPLET
if ([string]::IsNullOrWhiteSpace($Triplet)) {
    $Triplet = if ($Dynamic) {
        if ($Arch -eq "arm64") { "arm64-windows" } else { "x64-windows" }
    } elseif ($Arch -eq "arm64") {
        "arm64-windows-static-md"
    } else {
        "x64-windows-static-md"
    }
}
if ($Dynamic -and $Triplet -match "-static") {
    throw "VCPKGRS_DYNAMIC is set but VCPKGRS_TRIPLET='$Triplet' is a static triplet. Use a dynamic Windows triplet or unset VCPKGRS_DYNAMIC."
}
if (-not $Dynamic -and $Triplet -notmatch "-static") {
    throw "VCPKGRS_TRIPLET='$Triplet' is dynamic, but VCPKGRS_DYNAMIC is not set. Set VCPKGRS_DYNAMIC=1 so the runtime DLLs can be bundled."
}
$env:VCPKGRS_TRIPLET = $Triplet

$VcpkgTripletRoot = Join-Path $VcpkgRoot "installed\$Triplet"
$ArchiveHeader = Join-Path $VcpkgTripletRoot "include\archive.h"
$ArchiveImportLibrary = Join-Path $VcpkgTripletRoot "lib\archive.lib"
if (-not (Test-Path $ArchiveHeader) -or -not (Test-Path $ArchiveImportLibrary)) {
    Write-Host "Installing libarchive for vcpkg triplet $Triplet..."
    & (Join-Path $VcpkgRoot "vcpkg.exe") install "libarchive:$Triplet" --disable-metrics
    if ($LASTEXITCODE -ne 0) { throw "vcpkg could not install libarchive for triplet $Triplet (exit code $LASTEXITCODE)." }
}
if (-not (Test-Path $ArchiveHeader) -or -not (Test-Path $ArchiveImportLibrary)) {
    throw "libarchive is not installed for vcpkg triplet $Triplet. Expected $ArchiveHeader and $ArchiveImportLibrary."
}
if ($Dynamic -and -not (Test-Path (Join-Path $VcpkgTripletRoot "bin\archive.dll"))) {
    throw "libarchive archive.dll is missing for dynamic vcpkg triplet $Triplet."
}

Remove-Item $Stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Stage -ItemType Directory -Force | Out-Null
dotnet publish "$Root\apps\windows\Qfind.Windows.csproj" -c Release -r $Rid --self-contained true -p:Platform=$Platform -o $Stage
if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed (exit code $LASTEXITCODE)." }
if (-not (Test-Path (Join-Path $Stage "qfind_native.dll"))) {
    throw "The published app is missing qfind_native.dll; the Rust native bridge was not packaged."
}
if ($Dynamic -and -not (Test-Path (Join-Path $Stage "archive.dll"))) {
    throw "The published app is missing archive.dll; dynamic libarchive runtime packaging failed."
}
Copy-Item "$Root\LICENSE" "$Stage\LICENSE"
$Archive = "$Root\target\dist\$Name.zip"
Remove-Item $Archive -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Archive
$Hash = Get-FileHash $Archive -Algorithm SHA256
$HashLine = "$($Hash.Hash.ToLowerInvariant())  $(Split-Path $Archive -Leaf)"
Set-Content "$Archive.sha256" -Value $HashLine -NoNewline -Encoding ascii
$Hash
