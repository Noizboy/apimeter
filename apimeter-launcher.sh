#!/usr/bin/env bash
# Launcher for Apimeter (development binary).
# Sets env vars required on Snap-based Ubuntu systems.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export GDK_BACKEND=x11
export LD_PRELOAD=/lib/x86_64-linux-gnu/libpthread.so.0
exec "${HERE}/src-tauri/target/debug/openrouter-widget" "$@"
