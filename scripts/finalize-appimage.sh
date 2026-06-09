#!/usr/bin/env bash
# finalize-appimage.sh
# Repack the Tauri AppImage so its AppRun forces GDK_BACKEND=x11
# (XWayland), making the always-on-top hint reliable on Mutter/GNOME
# Wayland. Double-clicking the AppImage (or launching it from the
# file manager / app menu) goes through the new wrapper AppRun.
#
# Approach (no appimagetool required):
#   1. Read the squashfs offset from the original AppImage
#      (`<AppImage> --appimage-offset`).
#   2. Extract the AppImage (self-extracts to squashfs-root/).
#   3. Replace squashfs-root/AppRun with a wrapper that exports
#      GDK_BACKEND=x11 and execs the original AppRun.wrapped.
#   4. Build a new squashfs with mksquashfs.
#   5. Reassemble: cat <runtime>=head -c OFFSET > New.AppImage
#      (the runtime is the first OFFSET bytes of the original; the
#      new squashfs is appended with no extra padding).
#
# Usage:  finalize-appimage.sh <input.AppImage> <output.AppImage>
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <input.AppImage> <output.AppImage>" >&2
  exit 2
fi

IN="$1"
OUT="$2"

for tool in mksquashfs; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: '$tool' required" >&2; exit 1
  fi
done

[ -f "$IN" ] || { echo "error: not found: $IN" >&2; exit 1; }

chmod +x "$IN"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

echo ">>> reading squashfs offset from original"
OFFSET="$("$IN" --appimage-offset)"
echo "    offset = $OFFSET bytes"

echo ">>> extracting"
"$IN" --appimage-extract >/dev/null
[ -d squashfs-root ] || { echo "error: extraction failed" >&2; exit 1; }

echo ">>> installing wrapper AppRun"
mv squashfs-root/AppRun squashfs-root/AppRun.original
cat > squashfs-root/AppRun <<'EOF'
#!/usr/bin/env bash
# Wrapper: force GDK_BACKEND=x11 so always-on-top is handled by
# Mutter's XWM (reliable on GNOME/Wayland). The native Wayland
# backend of GTK can drop the above-stack hint on re-stack events.
set -e
this_dir="$(readlink -f "$(dirname "$0")")"
export GDK_BACKEND=x11
exec "$this_dir"/AppRun.original "$@"
EOF
chmod +x squashfs-root/AppRun

echo ">>> extracting runtime (first $OFFSET bytes)"
head -c "$OFFSET" "$IN" > runtime

echo ">>> building new squashfs"
# Use zstd (matches what the standard AppImage FUSE runtime can mount
# on Ubuntu; xz-compressed squashfs is not supported by the bundled
# squashfuse and would fail with "this version supports only zlib, zstd").
mksquashfs squashfs-root/ newsqsh \
  -comp zstd -noappend -all-root -no-xattrs

echo ">>> reassembling AppImage"
cat runtime newsqsh > "$OUT"
chmod +x "$OUT"

echo ""
echo "Final AppImage: $OUT"
ls -la "$OUT"