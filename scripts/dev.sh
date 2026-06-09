#!/usr/bin/env bash
# Wrapper to launch the Tauri dev session with the snap/glibc fix and
# workspace handling for GNOME Wayland.
#
# On GNOME Wayland, the widget can land in a workspace different from the
# user's active one (a Sway/Mutter quirk). After the window appears we:
#   1. Move it to the currently active workspace
#   2. Set _NET_WM_STATE_STICKY so it follows the user across workspaces
#   3. Make sure it is focused
set -u

# --- snap/glibc + cargo env (from the original fix) ---
export PATH="$HOME/.cargo/bin:$PATH"
export LD_PRELOAD="/lib/x86_64-linux-gnu/libpthread.so.0${LD_PRELOAD:+:$LD_PRELOAD}"
cd /home/lex/Documents/Repositories/apimeter

# --- Watcher: pin widget to active workspace + sticky ---
(
  # Wait for the window to appear (poll for up to 30s).
  WIN_ID=""
  for _ in $(seq 1 30); do
    WIN_ID="$(xdotool search --name 'Apimeter' 2>/dev/null | head -1 || true)"
    [ -n "$WIN_ID" ] && break
    sleep 1
  done

  if [ -z "$WIN_ID" ]; then
    echo "[dev.sh] could not find Apimeter window after 30s" >&2
    exit 0
  fi

  # Active workspace index (wmctrl: 0-based).
  ACTIVE_WS="$(wmctrl -d 2>/dev/null | awk '/\*/ { print $1; exit }')"
  if [ -n "$ACTIVE_WS" ]; then
    wmctrl -i -r "$WIN_ID" -t "$ACTIVE_WS" >/dev/null 2>&1 || true
  fi

  # Make it sticky: visible on every workspace. XWayland accepts the
  # EWMH hint even on Wayland apps (Tauri uses XWayland via GTK).
  xprop -id "$WIN_ID" -f _NET_WM_STATE 32a \
    -set _NET_WM_STATE _NET_WM_STATE_STICKY >/dev/null 2>&1 || true

  # Raise + focus.
  wmctrl -i -R "$WIN_ID" >/dev/null 2>&1 || true
  xdotool windowactivate "$WIN_ID" >/dev/null 2>&1 || true
) &

exec npm run tauri dev "$@"
