#!/usr/bin/env bash
# Build the Linux AppImage.
# Usage: packaging/linux/build-appimage.sh <version> <stage-dir> <out-dir>
# x86_64 only (the pinned tools and ARCH below).
# <stage-dir> holds the binaries in the installed layout: axidps, axidps-core,
# overlay/axidps-overlay (+ extras), game/d3d11.dll. Output: <out-dir>/axidps-<version>.AppImage
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: $0 <version> <stage-dir> <out-dir>" >&2; exit 2; }
VERSION="$1"
[ -d "$2" ] || { echo "stage dir not found: $2" >&2; exit 1; }
STAGE="$(cd "$2" && pwd)"; mkdir -p "$3"; OUT="$(cd "$3" && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
PKG="$HERE/.."

LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage"
LINUXDEPLOY_SHA256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"

CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/axidps-packaging"
mkdir -p "$CACHE"

fetch() { # url sha256 -> path of verified cached tool
  local file
  file="$CACHE/$(basename "$1")"
  if [ ! -f "$file" ] || ! echo "$2  $file" | sha256sum -c --status; then
    curl -fsSL -o "$file.part" "$1"
    echo "$2  $file.part" | sha256sum -c --status || { echo "checksum mismatch for $1" >&2; rm -f "$file.part"; exit 1; }
    mv "$file.part" "$file"
  fi
  chmod +x "$file"
  echo "$file"
}
LINUXDEPLOY="$(fetch "$LINUXDEPLOY_URL" "$LINUXDEPLOY_SHA256")"
APPIMAGETOOL="$(fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256")"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APPDIR="$WORK/AppDir"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/icons/hicolor/32x32/apps"

cp -a "$STAGE/." "$APPDIR/usr/bin/"
"$PKG/stage-page.sh" "$APPDIR/usr/bin/overlay/page" "$VERSION"
cp "$PKG/icon.png" "$APPDIR/axidps.png"
cp "$PKG/icon.png" "$APPDIR/usr/share/icons/hicolor/32x32/apps/axidps.png"
cp "$HERE/axidps.desktop" "$APPDIR/axidps.desktop"

# NO_STRIP: linuxdeploy's bundled strip cannot read modern (.relr.dyn) binaries.
# Bundle the overlay host's shared libraries (linuxdeploy does not build the AppImage).
NO_STRIP=1 APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY" --appimage-extract-and-run --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/overlay/axidps-overlay" \
  --desktop-file "$APPDIR/axidps.desktop"

# linuxdeploy copies the executable to usr/bin/; the real one stays in overlay/, so point
# its runpath at the bundled libraries (usr/lib) instead of exporting LD_LIBRARY_PATH.
rm -f "$APPDIR/usr/bin/axidps-overlay"
PATCHELF=""
EXTRACT="$CACHE/linuxdeploy-${LINUXDEPLOY_SHA256:0:12}"
if [ ! -x "$EXTRACT/squashfs-root/usr/bin/patchelf" ]; then
  rm -rf "$EXTRACT"; mkdir -p "$EXTRACT"
  (cd "$EXTRACT" && "$LINUXDEPLOY" --appimage-extract >/dev/null 2>&1) || true
fi
if [ -x "$EXTRACT/squashfs-root/usr/bin/patchelf" ]; then
  PATCHELF="$EXTRACT/squashfs-root/usr/bin/patchelf"
elif command -v patchelf >/dev/null; then
  PATCHELF="$(command -v patchelf)"
else
  echo "patchelf not found (not in the pinned linuxdeploy AppImage nor on PATH)" >&2; exit 1
fi
# shellcheck disable=SC2016 # $ORIGIN is literal
"$PATCHELF" --set-rpath '$ORIGIN/../../lib' "$APPDIR/usr/bin/overlay/axidps-overlay"

# Our own AppRun (linuxdeploy may have written a symlink).
rm -f "$APPDIR/AppRun"
cp "$HERE/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --appimage-extract-and-run "$APPDIR" "$OUT/axidps-$VERSION.AppImage"
echo "built $OUT/axidps-$VERSION.AppImage"
