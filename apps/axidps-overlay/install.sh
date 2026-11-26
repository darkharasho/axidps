#!/usr/bin/env bash
# Install the overlay host (release build + page) to ~/.local/share/axidps/overlay and run it as
# the systemd user service axidps-overlay. It idles until the game's DLL (feature `overlay`)
# attaches. Build first: (cd apps/axidps-overlay && cargo build --release).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="$HOME/.local/share/axidps/overlay"
UNIT="$HOME/.config/systemd/user/axidps-overlay.service"
mkdir -p "$DEST/page" "$(dirname "$UNIT")"
cp "$HERE/target/release/axidps-overlay" "$DEST/axidps-overlay.new"
mv "$DEST/axidps-overlay.new" "$DEST/axidps-overlay"
rm -f "$DEST/page/meter.html"
cp "$HERE/page/"*.html "$HERE/page/"*.js "$DEST/page/"
mkdir -p "$DEST/page/fonts"
cp "$HERE/page/fonts/"* "$DEST/page/fonts/"
# Beside page/: the plugin SDK (with the page's fonts, one source) and the bundled plugins.
mkdir -p "$DEST/sdk/fonts" "$DEST/plugins"
cp -r "$HERE/sdk/." "$DEST/sdk/"
cp "$HERE/page/fonts/"* "$DEST/sdk/fonts/"
cp -r "$HERE/plugins/." "$DEST/plugins/"
rm -f "$DEST/sdk/.gitkeep" "$DEST/plugins/.gitkeep" "$DEST/sdk/"*.test.js
# The build the hub shows (Updates).
echo "window.AXIDPS_BUILD = \"${AXIDPS_VERSION:-$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo dev)}\";" > "$DEST/page/version.js"
# Class icons from the gw2-class-icons npm package (a root dependency), named by spec.
ICONS="$HERE/../../node_modules/gw2-class-icons/wiki/40px"
[ -d "$ICONS" ] || (cd "$HERE/../.." && npm install --no-audit --no-fund >/dev/null)
mkdir -p "$DEST/page/icons"
cp "$ICONS/"*.png "$DEST/page/icons/"
cat > "$UNIT" <<UNIT
[Unit]
Description=axidps overlay host (Servo renders the in-game windows)
After=graphical-session.target

[Service]
ExecStart=$DEST/axidps-overlay $DEST/page/index.html
Restart=always
RestartSec=5
Nice=5

[Install]
WantedBy=default.target
UNIT
# The host's GL window can't be hidden on Wayland; keep it off KDE's taskbar, pager and Alt+Tab.
if command -v kwriteconfig6 >/dev/null; then
  RULE=axidps-overlay-hidden
  kw() { kwriteconfig6 --file kwinrulesrc --group "$RULE" --key "$1" "$2"; }
  kw Description "axidps overlay host: hidden"
  kw wmclass axidps-overlay; kw wmclassmatch 1
  for k in skiptaskbar skippager skipswitcher; do kw "$k" true; kw "${k}rule" 2; done
  kw minimize true; kw minimizerule 2
  ORDER="$(kreadconfig6 --file kwinrulesrc --group General --key Order)"
  case ",$ORDER," in *",$RULE,"*) ;; *)
    kwriteconfig6 --file kwinrulesrc --group General --key Order "${ORDER:+$ORDER,}$RULE" ;;
  esac
  dbus-send --session --type=method_call --dest=org.kde.KWin /KWin org.kde.KWin.reconfigure || true
fi
systemctl --user daemon-reload
systemctl --user enable axidps-overlay.service >/dev/null
systemctl --user restart axidps-overlay.service
echo "installed $DEST; service axidps-overlay restarted"
