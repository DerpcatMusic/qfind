#!/usr/bin/env bash
# Push packaging/aur/<name> to aur.archlinux.org/<name>.
# Needs an AUR account whose SSH key is registered:
#   https://aur.archlinux.org/account/
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
name="${1:-}"
if [[ -z "$name" || ! -d "$root/packaging/aur/$name" ]]; then
  echo "usage: $0 qfind-bin|qfind" >&2
  exit 1
fi
src="$root/packaging/aur/$name"
if ! ssh -o BatchMode=yes -o ConnectTimeout=10 aur@aur.archlinux.org 2>&1 | grep -q 'help'; then
  echo "AUR SSH failed. Add your public key at https://aur.archlinux.org/account/" >&2
  echo "then: ssh aur@aur.archlinux.org" >&2
  exit 1
fi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git clone "ssh://aur@aur.archlinux.org/${name}.git" "$work/pkg"
cp "$src/PKGBUILD" "$work/pkg/PKGBUILD"
(cd "$src" && makepkg --printsrcinfo) > "$work/pkg/.SRCINFO"
cd "$work/pkg"
git add PKGBUILD .SRCINFO
git -c user.email="qfind@users.noreply.github.com" -c user.name="qfind" \
  commit -m "qfind $name $(grep ^pkgver= PKGBUILD | cut -d= -f2)"
git push origin master
echo "https://aur.archlinux.org/packages/$name"
