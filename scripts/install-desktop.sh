#!/usr/bin/env bash
# Install a user-local .desktop entry + hicolor icons so Orrery can be pinned
# to the taskbar / dash when run from a source build (cargo run).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${ORRERY_BIN:-$ROOT/target/debug/orrery}"
APP_ID="com.orrery.app"
NAME="orrery"

if [[ ! -x "$BIN" ]]; then
  echo "binary not found: $BIN" >&2
  echo "build first: cargo build -p orrery" >&2
  exit 1
fi

APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICON_BASE="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
mkdir -p "$APP_DIR"

install_icon() {
  local size="$1" src="$2"
  local dir="$ICON_BASE/${size}/apps"
  mkdir -p "$dir"
  cp -f "$src" "$dir/${NAME}.png"
  # Some shells look up by app id name as well.
  cp -f "$src" "$dir/${APP_ID}.png"
}

install_icon 32x32   "$ROOT/packaging/icons/32x32.png"
install_icon 64x64   "$ROOT/packaging/icons/64x64.png"
install_icon 128x128 "$ROOT/packaging/icons/128x128.png"
install_icon 512x512 "$ROOT/packaging/icons/icon.png"

DESKTOP_SRC="$ROOT/packaging/orrery.desktop"
DESKTOP_DST="$APP_DIR/${APP_ID}.desktop"
# Also keep a short name for menus that look up "orrery".
DESKTOP_ALIAS="$APP_DIR/${NAME}.desktop"

sed -e "s|^Exec=.*|Exec=${BIN}|" \
    -e "s|^Icon=.*|Icon=${NAME}|" \
    "$DESKTOP_SRC" > "$DESKTOP_DST"
cp -f "$DESKTOP_DST" "$DESKTOP_ALIAS"

# Refresh caches when tools exist (best-effort).
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$ICON_BASE" >/dev/null 2>&1 || true
fi

echo "Installed:"
echo "  $DESKTOP_DST"
echo "  icons under $ICON_BASE/*/apps/${NAME}.png"
echo "Pin from the app menu / right-click the running task → Pin to taskbar."
echo "Launch: $BIN   (or open “Orrery” from the applications menu)"
