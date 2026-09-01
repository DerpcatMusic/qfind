#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
cargo build --release -p qfind-native
install_name_tool -id '@rpath/libqfind_native.dylib' target/release/libqfind_native.dylib
swift build -c release --package-path apps/macos
app="$root/target/release/Qfind.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks"
install -m755 apps/macos/.build/release/Qfind "$app/Contents/MacOS/Qfind"
install -m755 target/release/libqfind_native.dylib "$app/Contents/Frameworks/libqfind_native.dylib"
install -m644 apps/macos/Info.plist "$app/Contents/Info.plist"
echo "built $app"
