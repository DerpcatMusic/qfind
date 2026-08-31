$ErrorActionPreference = "Stop"
$Repo = "DerpcatMusic/qfind"
$InstallDir = if ($env:QFIND_INSTALL_DIR) { $env:QFIND_INSTALL_DIR } else {
    Join-Path $env:LOCALAPPDATA "Programs\qfind"
}
$Tag = if ($env:QFIND_VERSION) { $env:QFIND_VERSION } else {
    (Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest").tag_name
}
if (-not $Tag.StartsWith("v")) { $Tag = "v$Tag" }
$Version = $Tag.TrimStart("v")
$Arch = switch ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    "X64" { "x86_64" }
    "Arm64" { "arm64" }
    default { throw "Unsupported architecture: $([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)" }
}

$Temp = Join-Path ([IO.Path]::GetTempPath()) "qfind-$([guid]::NewGuid())"
$Archive = "$Temp\qfind.zip"
New-Item $Temp -ItemType Directory | Out-Null
try {
    $Asset = "qfind-$Version-windows-$Arch.zip"
    Invoke-WebRequest "https://github.com/$Repo/releases/download/$Tag/$Asset" -OutFile $Archive
    Expand-Archive $Archive -DestinationPath $Temp
    $Qfind = Get-ChildItem $Temp -Filter qfind.exe -Recurse | Select-Object -First 1
    $QfindTui = Get-ChildItem $Temp -Filter qfind-tui.exe -Recurse | Select-Object -First 1
    if (-not $Qfind -or -not $QfindTui) { throw "Release archive is incomplete." }
    New-Item $InstallDir -ItemType Directory -Force | Out-Null
    Copy-Item $Qfind.FullName "$InstallDir\qfind.exe" -Force
    Copy-Item $QfindTui.FullName "$InstallDir\qfind-tui.exe" -Force
} finally {
    Remove-Item $Temp -Recurse -Force -ErrorAction SilentlyContinue
}

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($UserPath -split ";") -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", "$InstallDir;$UserPath", "User")
}
$env:Path = "$InstallDir;$env:Path"
Write-Host "Installed Qfind $Version in $InstallDir"
Write-Host "Run: qfind index; qfind"
