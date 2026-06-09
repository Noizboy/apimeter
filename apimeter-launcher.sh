#!/usr/bin/env bash
# Launcher for Apimeter (release binary).
# Sets env vars required on Snap-based Ubuntu systems.
set -euo pipefail
PROJECT_DIR="/home/lex/Documents/Repositories/apimeter"
export GDK_BACKEND=x11
export LD_PRELOAD=/lib/x86_64-linux-gnu/libpthread.so.0
# Force standard light theme so tray icon menu text is visible
# (dark themes like Yaru-*-dark can make menu text invisible)
export GTK_THEME=Adwaita:light
cd "${PROJECT_DIR}"
exec "${PROJECT_DIR}/src-tauri/target/release/openrouter-widget" "$@"
