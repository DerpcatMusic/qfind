#!/bin/sh
set -eu
case "$(uname -s)" in
    Darwin) ;;
    *) echo "build-macos-app.sh must run on macOS" >&2; exit 1 ;;
esac
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
cargo build --release -p qfind-native
install_name_tool -id '@rpath/libqfind_native.dylib' target/release/libqfind_native.dylib
swift build -c release --package-path apps/macos
swift_bin="$(swift build -c release --package-path apps/macos --show-bin-path)"
app="$root/target/release/Qfind.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks"
install -m755 "$swift_bin/Qfind" "$app/Contents/MacOS/Qfind"
install -m755 target/release/libqfind_native.dylib "$app/Contents/Frameworks/libqfind_native.dylib"
install -m644 apps/macos/Info.plist "$app/Contents/Info.plist"

frameworks="$app/Contents/Frameworks"
executable="$app/Contents/MacOS/Qfind"
# Swift command-line builds can reference the toolchain runtime through
# @rpath. Copy it when the macOS toolchain provides swift-stdlib-tool so the
# app does not depend on a developer installation at runtime.
if command -v xcrun >/dev/null 2>&1; then
    swift_stdlib_tool=$(xcrun --find swift-stdlib-tool 2>/dev/null || true)
    if [ -n "$swift_stdlib_tool" ]; then
        "$swift_stdlib_tool" --copy --scan-executable "$executable" --destination "$frameworks"
    fi
fi
work="${TMPDIR:-/tmp}/megaman-macos-dylibs.$$"
mkdir -p "$work/seen"
trap 'rm -rf "$work"' EXIT INT TERM

is_system_dependency() {
    case "$1" in
        /System/Library/*|/usr/lib/*|/usr/libexec/*|/Library/Apple/*) return 0 ;;
        *) return 1 ;;
    esac
}

expand_path() {
    case "$1" in
        @loader_path/*) printf '%s/%s\n' "$(dirname "$2")" "${1#@loader_path/}" ;;
        @executable_path/*) printf '%s/%s\n' "$(dirname "$executable")" "${1#@executable_path/}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

resolve_dependency() {
    dependency=$1
    owner=$2
    case "$dependency" in
        /*)
            [ -f "$dependency" ] && { printf '%s\n' "$dependency"; return 0; }
            ;;
        @loader_path/*|@executable_path/*)
            candidate=$(expand_path "$dependency" "$owner")
            [ -f "$candidate" ] && { printf '%s\n' "$candidate"; return 0; }
            ;;
        @rpath/*)
            relative=${dependency#@rpath/}
            while IFS= read -r rpath; do
                [ -n "$rpath" ] || continue
                candidate=$(expand_path "$rpath/$relative" "$owner")
                [ -f "$candidate" ] && { printf '%s\n' "$candidate"; return 0; }
            done <<EOF
$(otool -l "$owner" 2>/dev/null | awk '/cmd LC_RPATH/{seen=1; next} seen && $1 == "path" {sub(/^[[:space:]]*path /, ""); sub(/ \(offset.*/, ""); print; seen=0}')
EOF
            for directory in "$frameworks" "$root/target/release" /opt/homebrew/lib /usr/local/lib /opt/homebrew/opt/*/lib /usr/local/opt/*/lib; do
                [ -f "$directory/$relative" ] && { printf '%s\n' "$directory/$relative"; return 0; }
            done
            if command -v brew >/dev/null 2>&1; then
                brew_prefix=$(brew --prefix 2>/dev/null || true)
                [ -f "$brew_prefix/lib/$relative" ] && { printf '%s\n' "$brew_prefix/lib/$relative"; return 0; }
                brew_archive_prefix=$(brew --prefix libarchive 2>/dev/null || true)
                [ -f "$brew_archive_prefix/lib/$relative" ] && { printf '%s\n' "$brew_archive_prefix/lib/$relative"; return 0; }
            fi
            ;;
    esac
    return 1
}

bundle_dylib() {
    source=$1
    base=$(basename "$source")
    destination="$frameworks/$base"
    key=$(printf '%s' "$source" | cksum | awk '{print $1}')
    [ -e "$work/seen/$key" ] && return 0
    : > "$work/seen/$key"
    if [ ! -f "$destination" ]; then
        install -m755 "$source" "$destination"
    fi
    install_name_tool -id "@rpath/$base" "$destination"
    while IFS= read -r dependency; do
        [ -n "$dependency" ] || continue
        is_system_dependency "$dependency" && continue
        resolved=$(resolve_dependency "$dependency" "$source") || {
            echo "Could not resolve non-system dependency $dependency from $source" >&2
            exit 1
        }
        dependency_base=$(basename "$resolved")
        (bundle_dylib "$resolved")
        install_name_tool -change "$dependency" "@rpath/$dependency_base" "$destination"
    done <<EOF
$(otool -L "$source" | awk 'NR > 1 {sub(/^[[:space:]]+/, ""); sub(/ \(compatibility version.*/, ""); print}')
EOF
}

# The executable and Rust bridge both carry their own dependency graph. Any
# non-Apple dylib (including libarchive and its codec dependencies) is copied
# beside the app and referenced through the app's Frameworks rpath.
bundle_dylib "$root/target/release/libqfind_native.dylib"
while IFS= read -r dependency; do
    [ -n "$dependency" ] || continue
    is_system_dependency "$dependency" && continue
    resolved=$(resolve_dependency "$dependency" "$executable") || {
        echo "Could not resolve non-system dependency $dependency from $executable" >&2
        exit 1
    }
    (bundle_dylib "$resolved")
    install_name_tool -change "$dependency" "@rpath/$(basename "$resolved")" "$executable"
done <<EOF
$(otool -L "$executable" | awk 'NR > 1 {sub(/^[[:space:]]+/, ""); sub(/ \(compatibility version.*/, ""); print}')
EOF
install_name_tool -add_rpath '@executable_path/../Frameworks' "$executable" 2>/dev/null || true
for library in "$frameworks"/*.dylib; do
    [ -f "$library" ] && codesign --force --sign - "$library"
done
codesign --force --sign - "$app"
echo "built $app"
