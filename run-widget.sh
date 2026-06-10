#!/usr/bin/env bash
# Launcher for the Apimeter release build.
#
# Forces GDK_BACKEND=x11 so the always-on-top hint (_NET_WM_STATE_ABOVE)
# is managed by Mutter's XWM, which is the reliable path on GNOME/Wayland.
# The native Wayland backend (GDK_BACKEND=wayland, the default on a
# Wayland session) is more prone to dropping the above-stack hint on
# re-stack events, which is why the watchdog alone wasn't enough.
#
# Also preloads the system libpthread to work around Snap/core20 symbol
# conflicts on Ubuntu systems installed via Snap.
#
# Usage:  ./run-widget.sh
# Requirements: a graphical session with DISPLAY and XAUTHORITY set
# (a normal GNOME terminal inherits these from the session).
set -euo pipefail
export GDK_BACKEND=x11
export LD_PRELOAD=/lib/x86_64-linux-gnu/libpthread.so.0
# Force standard light theme so tray icon menu text is visible
# (dark themes like Yaru-*-dark can make menu text invisible)
export GTK_THEME=Adwaita:light
exec /usr/bin/openrouter-widget "$@"
