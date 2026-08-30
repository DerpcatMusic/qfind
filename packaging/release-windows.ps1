$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Version = if ($args.Count) { $args[0] } else {
    (Select-String -Path "$Root\Cargo.toml" -Pattern '^version = "(.+)"$').Matches[0].Groups[1].Value
}
$Name = "qfind-$Version-windows-x86_64"
$Stage = "$Root\target\dist\$Name"

Remove-Item $Stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Stage -ItemType Directory -Force | Out-Null
cargo build --release --manifest-path "$Root\Cargo.toml" -p qfind -p qfind-tui
Copy-Item "$Root\target\release\qfind.exe" "$Stage\qfind.exe"
Copy-Item "$Root\target\release\qfind.exe" "$Stage\qfind-cli.exe"
Copy-Item "$Root\target\release\qfind-tui.exe" "$Stage\qfind-tui.exe"
Copy-Item "$Root\LICENSE" "$Stage\LICENSE"
$Archive = "$Root\target\dist\$Name.zip"
Remove-Item $Archive -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Archive
Get-FileHash $Archive -Algorithm SHA256
