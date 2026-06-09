#!/usr/bin/env bash

set -euo pipefail

printf '\n[1/3] Installing Ubuntu packages for Tauri...\n'
sudo apt update
sudo apt install -y \
  build-essential \
  curl \
  file \
  wget \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libwebkit2gtk-4.1-dev

printf '\n[2/3] Installing Rust via rustup...\n'
if ! command -v cargo >/dev/null 2>&1; then
  curl https://sh.rustup.rs -sSf | sh -s -- -y
fi

printf '\n[3/3] Final notes...\n'
printf 'Run this in your shell before starting Tauri:\n'
printf '  source "$HOME/.cargo/env"\n\n'
printf 'Then run:\n'
printf '  npm run tauri dev\n\n'
