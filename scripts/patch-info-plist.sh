#!/usr/bin/env bash
# S6 (R-09/R10): Tauri's fileAssociations config generates CFBundleTypeExtensions
# (extension matching) but not LSItemContentTypes (UTI conformance) on macOS.
# Binding to the system-declared STL UTI -- public.standard-tesselated-geometry-format,
# confirmed present via `mdls` during planning -- needs this patch after every
# `tauri build`, since the bundler overwrites Info.plist each time.
set -euo pipefail

APP_PATH="${1:?usage: patch-info-plist.sh <path-to-.app>}"
PLIST="$APP_PATH/Contents/Info.plist"
UTI="public.standard-tesselated-geometry-format"

if [ ! -f "$PLIST" ]; then
  echo "error: $PLIST not found" >&2
  exit 1
fi

if /usr/libexec/PlistBuddy -c "Print :CFBundleDocumentTypes:0:LSItemContentTypes" "$PLIST" >/dev/null 2>&1; then
  echo "LSItemContentTypes already present, nothing to do"
  exit 0
fi

/usr/libexec/PlistBuddy \
  -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes array" \
  -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes:0 string $UTI" \
  "$PLIST"

echo "patched: CFBundleDocumentTypes:0:LSItemContentTypes = [$UTI]"
