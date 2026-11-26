#!/usr/bin/env bash
# Assemble the overlay page (what apps/axidps-overlay/install.sh copies) into <page-dest-dir>, and
# the plugin SDK and bundled plugins into `sdk/` and `plugins/` beside it (the exe's folder).
# Usage: packaging/stage-page.sh <page-dest-dir> <version>
set -euo pipefail
[ $# -eq 2 ] || { echo "usage: $0 <page-dest-dir> <version>" >&2; exit 2; }
DEST="$1"
VERSION="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PAGE="$ROOT/apps/axidps-overlay/page"
ICONS="$ROOT/node_modules/gw2-class-icons/wiki/40px"

mkdir -p "$DEST/fonts" "$DEST/icons"
cp "$PAGE/"*.html "$DEST/"
for f in "$PAGE/"*.js; do
  case "$f" in *.test.js) ;; *) cp "$f" "$DEST/" ;; esac
done
cp "$PAGE/fonts/"* "$DEST/fonts/"
SIDE="$(dirname "$DEST")"
mkdir -p "$SIDE/sdk/fonts" "$SIDE/plugins"
cp -r "$ROOT/apps/axidps-overlay/sdk/." "$SIDE/sdk/"
cp "$PAGE/fonts/"* "$SIDE/sdk/fonts/"
cp -r "$ROOT/apps/axidps-overlay/plugins/." "$SIDE/plugins/"
rm -f "$SIDE/sdk/.gitkeep" "$SIDE/plugins/.gitkeep" "$SIDE/sdk/"*.test.js
echo "window.AXIDPS_BUILD = \"$VERSION\";" > "$DEST/version.js"
[ -d "$ICONS" ] || (cd "$ROOT" && npm install --no-audit --no-fund >/dev/null)
cp "$ICONS/"*.png "$DEST/icons/"
