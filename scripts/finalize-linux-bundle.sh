#!/usr/bin/env bash
# finalize-linux-bundle.sh
# Post-process the Tauri .deb so launching the app from the desktop /
# app menu forces GDK_BACKEND=x11 (XWayland), which makes the
# always-on-top hint (_NET_WM_STATE_ABOVE) reliable on Mutter / GNOME
# Wayland.
#
# What it does:
#   1. Extracts the .deb (data + control).
#   2. Moves the real binary from /usr/bin/<bin> into
#      /usr/lib/<bin>/<bin>.bin.
#   3. Writes a tiny shell wrapper at /usr/bin/<bin> that exports
#      GDK_BACKEND=x11 and execs the real binary.
#   4. Repacks the .deb with fakeroot so file ownership is recorded
#      as root:root (FHS-correct for /usr/bin and /usr/lib).
#
# Usage:  finalize-linux-bundle.sh <input.deb> <output.deb>
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <input.deb> <output.deb>" >&2
  exit 2
fi

IN_DEB="$1"
OUT_DEB="$2"
BIN_NAME="openrouter-widget"

for tool in dpkg-deb fakeroot; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: '$tool' is required but not installed" >&2
    exit 1
  fi
done

if [ ! -f "$IN_DEB" ]; then
  echo "error: input .deb not found: $IN_DEB" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo ">>> extracting $IN_DEB"
cd "$WORK"
dpkg-deb -x "$IN_DEB" data
dpkg-deb -e "$IN_DEB" control

REAL_BIN="data/usr/bin/${BIN_NAME}"
LIB_DIR="data/usr/lib/${BIN_NAME}"
LIB_BIN="${LIB_DIR}/${BIN_NAME}.bin"
WRAPPER="data/usr/bin/${BIN_NAME}"

if [ ! -f "$REAL_BIN" ]; then
  echo "error: expected binary not found in .deb: $REAL_BIN" >&2
  exit 1
fi

echo ">>> moving real binary to ${LIB_BIN}"
mkdir -p "$LIB_DIR"
mv "$REAL_BIN" "$LIB_BIN"
chmod 0755 "$LIB_BIN"

echo ">>> writing wrapper at data/usr/bin/${BIN_NAME}"
cat > "$WRAPPER" <<EOF
#!/bin/sh
# Wrapper: force GDK_BACKEND=x11 so the always-on-top hint is
# handled by Mutter's XWM (reliable on GNOME/Wayland). The native
# Wayland backend of GTK can drop the above-stack hint on re-stack
# events, so this wrapper is what the .desktop's Exec= resolves to.
# LD_PRELOAD works around Snap/core20 libpthread symbol conflicts.
export GDK_BACKEND=x11
export LD_PRELOAD=/lib/x86_64-linux-gnu/libpthread.so.0
exec "/usr/lib/${BIN_NAME}/${BIN_NAME}.bin" "\$@"
EOF
chmod 0755 "$WRAPPER"

echo ">>> repacking .deb (fakeroot for root:root ownership)"
mkdir -p data/DEBIAN
cp -a control/. data/DEBIAN/
fakeroot dpkg-deb --build data "$OUT_DEB"

echo ""
echo "Final .deb: $OUT_DEB"
ls -la "$OUT_DEB"
