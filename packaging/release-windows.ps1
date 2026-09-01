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

Remove-Item $Stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Stage -ItemType Directory -Force | Out-Null
dotnet publish "$Root\apps\windows\Qfind.Windows.csproj" -c Release -r $Rid -p:Platform=$Platform -o $Stage
Copy-Item "$Root\LICENSE" "$Stage\LICENSE"
$Archive = "$Root\target\dist\$Name.zip"
Remove-Item $Archive -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Archive
Get-FileHash $Archive -Algorithm SHA256
