#!/usr/bin/env bash
# Regenerate the derived app icons in assets/app-icon/ with generate.py (standard library only).
# On macOS, sips and iconutil also write the .icns. The outputs are committed, so builds and
# packaging never need this.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

python3 -I "$DIR/generate.py" "$DIR/waltcut-1024.png" 1024
python3 -I "$DIR/generate.py" "$TMP/macos-1024.png" 1024 --macos
# Runtime window/Dock icon on macOS (apps/waltcut/src/main.rs embeds it).
sips -z 512 512 "$TMP/macos-1024.png" --out "$DIR/waltcut-macos-512.png" >/dev/null

if command -v iconutil >/dev/null && command -v sips >/dev/null; then
  SET="$TMP/waltcut.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$TMP/macos-1024.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s * 2)) $((s * 2)) "$TMP/macos-1024.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns -o "$DIR/waltcut.icns" "$SET"
else
  echo "note: iconutil/sips not found; waltcut.icns left as is"
fi
