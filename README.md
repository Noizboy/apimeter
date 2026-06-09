# Apimeter

Local desktop widget/overlay for Ubuntu built with Tauri 2 + TypeScript.

## Features

- Always-on-top widget layout
- Current OpenRouter balance
- Top 3 models by usage cost
- Dropdown with more model cost charts
- Clickable icon next to the balance that opens the OpenRouter activity page in the default browser
- Local-only fetches, no separate backend server

## Environment

Create `.env.local` in the project root for development:

```env
OPENROUTER_MANAGEMENT_KEY=your_management_key_here
```

For the installed desktop app, create:

```bash
mkdir -p ~/.config/openrouter-widget
printf 'OPENROUTER_MANAGEMENT_KEY=your_management_key_here\n' > ~/.config/openrouter-widget/.env
```

## Requirements

- Node.js 22+
- npm 9+
- Rust + Cargo
- Linux dependencies required by Tauri/WebKitGTK

## Ubuntu setup

Use the helper script:

```bash
bash scripts/setup-ubuntu-tauri.sh
source "$HOME/.cargo/env"
```

Or install manually:

```bash
sudo apt update
sudo apt install -y build-essential curl file wget libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libwebkit2gtk-4.1-dev
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
```

## Run

```bash
npm install
npm run tauri dev
```

## Frontend only build

```bash
npm run build
```

## Notes

- The app fetches credits from `GET /api/v1/credits`
- The app fetches account usage from `GET /api/v1/key`
- The app fetches per-model daily activity from `GET /api/v1/activity?date=YYYY-MM-DD`
- The management key is resolved in this order: `OPENROUTER_MANAGEMENT_KEY` from the shell, project `.env.local`, project `.env`, `~/.config/openrouter-widget/.env`, `~/.config/openrouter-widget/.env.local`, then bundled resource `.env.local`/`.env` files if present
- Window position writes are debounced to reduce filesystem churn while dragging
- Refresh failures keep the last successful widget data visible and mark it as stale
