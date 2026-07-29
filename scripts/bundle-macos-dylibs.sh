#!/usr/bin/env bash
# Make a built macOS Yapper.app launchable standalone.
#
# sherpa-onnx + onnxruntime are dynamic libraries. Tauri does not bundle them,
# and the binary references them via @rpath with NO LC_RPATH, so dyld aborts at
# launch anywhere except `tauri dev` (where cargo injects a dylib search path).
# This copies the (universal) dylibs into Contents/Frameworks, adds an
# @executable_path/../Frameworks rpath, and re-signs (ad-hoc) so the signature
# stays valid.
#
# Usage: scripts/bundle-macos-dylibs.sh <path/to/Yapper.app>
# Run it after `tauri build ... --bundles app`, then build the .dmg from the
# patched .app.
set -euo pipefail

APP="${1:?usage: bundle-macos-dylibs.sh <path/to/Yapper.app>}"
BIN="$APP/Contents/MacOS/yapper"
FW="$APP/Contents/Frameworks"
REPO="$(cd "$(dirname "$0")/.." && pwd)"

mkdir -p "$FW"
for lib in libonnxruntime.1.17.1.dylib libonnxruntime.dylib libsherpa-onnx-c-api.dylib; do
  src="$(find "$REPO/src-tauri/target" -name "$lib" -print -quit 2>/dev/null || true)"
  if [ -n "$src" ]; then
    cp -a "$src" "$FW/"
  else
    echo "warning: $lib not found under src-tauri/target" >&2
  fi
done

# add the rpath (idempotent — install_name_tool errors if it already exists)
install_name_tool -add_rpath @executable_path/../Frameworks "$BIN" 2>/dev/null || true

# re-sign the dylibs then the whole bundle (modifying the binary invalidated it)
codesign --force --sign - "$FW"/*.dylib
codesign --force --deep --sign - "$APP"

echo "Bundled sherpa/onnxruntime dylibs into $APP and re-signed."
