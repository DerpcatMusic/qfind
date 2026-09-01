#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)}"
dist="$root/target/dist"
work="$(mktemp -d)"
keychain="$work/signing.keychain-db"
keychain_password="$(uuidgen)"

for name in \
  APPLE_APPLICATION_CERTIFICATE_P12_BASE64 \
  APPLE_INSTALLER_CERTIFICATE_P12_BASE64 \
  APPLE_CERTIFICATE_PASSWORD \
  APPLE_DEVELOPER_ID_APPLICATION \
  APPLE_DEVELOPER_ID_INSTALLER \
  APPLE_ID \
  APPLE_APP_SPECIFIC_PASSWORD \
  APPLE_TEAM_ID; do
  [[ -n "${!name:-}" ]] || { echo "missing required environment variable: $name" >&2; exit 1; }
done

cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

cd "$root"
mkdir -p "$dist" "$work/app/Qfind.app/Contents/MacOS" \
  "$work/app/Qfind.app/Contents/Frameworks"
rustup target add x86_64-apple-darwin aarch64-apple-darwin
for target in x86_64-apple-darwin aarch64-apple-darwin; do
  cargo build --release --target "$target" -p qfind-native
done

lipo -create \
  target/x86_64-apple-darwin/release/libqfind_native.dylib \
  target/aarch64-apple-darwin/release/libqfind_native.dylib \
  -output "$work/libqfind_native.dylib"
install_name_tool -id '@rpath/libqfind_native.dylib' "$work/libqfind_native.dylib"

mkdir -p target/release
install -m755 "$work/libqfind_native.dylib" target/release/libqfind_native.dylib
swift build -c release --package-path apps/macos --arch x86_64 --arch arm64
swift_bin="$(swift build -c release --package-path apps/macos --arch x86_64 --arch arm64 --show-bin-path)"
install -m755 "$swift_bin/Qfind" "$work/app/Qfind.app/Contents/MacOS/Qfind"
install -m755 "$work/libqfind_native.dylib" \
  "$work/app/Qfind.app/Contents/Frameworks/libqfind_native.dylib"
install -m644 apps/macos/Info.plist "$work/app/Qfind.app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" \
  "$work/app/Qfind.app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleVersion string $version" \
  "$work/app/Qfind.app/Contents/Info.plist" 2>/dev/null || \
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" \
    "$work/app/Qfind.app/Contents/Info.plist"

for binary in "$work/app/Qfind.app/Contents/MacOS/Qfind" \
  "$work/app/Qfind.app/Contents/Frameworks/libqfind_native.dylib"; do
  lipo "$binary" -verify_arch x86_64 arm64
done

APPLICATION_P12="$work/application.p12" INSTALLER_P12="$work/installer.p12" python3 - <<'PY'
import base64
import os
import pathlib

pathlib.Path(os.environ["APPLICATION_P12"]).write_bytes(
    base64.b64decode(os.environ["APPLE_APPLICATION_CERTIFICATE_P12_BASE64"])
)
pathlib.Path(os.environ["INSTALLER_P12"]).write_bytes(
    base64.b64decode(os.environ["APPLE_INSTALLER_CERTIFICATE_P12_BASE64"])
)
PY

security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/application.p12" -k "$keychain" \
  -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12
security import "$work/installer.p12" -k "$keychain" \
  -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12
security list-keychains -d user -s "$keychain"
security set-key-partition-list -S apple-tool:,apple: -s \
  -k "$keychain_password" "$keychain"

sign() {
  codesign --force --sign "$APPLE_DEVELOPER_ID_APPLICATION" --keychain "$keychain" \
    --options runtime --timestamp "$1"
}
sign "$work/app/Qfind.app/Contents/Frameworks/libqfind_native.dylib"
sign "$work/app/Qfind.app/Contents/MacOS/Qfind"
sign "$work/app/Qfind.app"
codesign --verify --deep --strict --verbose=2 "$work/app/Qfind.app"

mkdir -p "$work/pkgroot/Applications"
ditto "$work/app/Qfind.app" "$work/pkgroot/Applications/Qfind.app"

pkg="$dist/qfind-$version-macos-universal.pkg"
pkgbuild --root "$work/pkgroot" --identifier music.derpcat.qfind.pkg \
  --version "$version" --install-location / \
  --sign "$APPLE_DEVELOPER_ID_INSTALLER" --keychain "$keychain" "$pkg"

notary_log="$dist/qfind-$version-macos-notary.json"
xcrun notarytool submit "$pkg" --apple-id "$APPLE_ID" \
  --password "$APPLE_APP_SPECIFIC_PASSWORD" --team-id "$APPLE_TEAM_ID" \
  --wait --output-format json > "$notary_log"
python3 - "$notary_log" <<'PY'
import json
import pathlib
import sys

status = json.loads(pathlib.Path(sys.argv[1]).read_text()).get("status")
if status != "Accepted":
    raise SystemExit(f"notarization was not accepted: {status}")
PY
xcrun stapler staple "$pkg"
xcrun stapler validate "$pkg"
pkgutil --check-signature "$pkg"
spctl --assess --type install --verbose=4 "$pkg"
(cd "$dist" && shasum -a 256 "$(basename "$pkg")" > "$(basename "$pkg").sha256")
echo "$pkg"
