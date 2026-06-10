import './styles.css';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { getDashboardData, saveWindowPosition } from './api';
import type { DashboardData, DashboardState, ModelCost } from './types';

const appRoot = document.querySelector<HTMLDivElement>('#app');

if (!appRoot) {
  throw new Error('App root not found');
}

const app = appRoot;

let state: DashboardState = { status: 'loading' };
let refreshHandle: number | undefined;
let unlistenMove: (() => void) | undefined;
let savePositionHandle: number | undefined;
let resizeFrameHandle: number | undefined;
let alwaysOnTopHandle: number | undefined;
let isRefreshing = false;
let appVersion = '';
let lastUpdatedAt: Date | null = null;

const WINDOW_FRAME_PADDING = 0;
const REFRESH_INTERVAL_MS = 15_000;
// Watchdog interval for re-asserting always-on-top. Some compositors
// (notably Mutter/Wayland) drop the above-stack hint after focus
// changes, workspace switches, drag, or when a spawned window (e.g. the
// browser opened by xdg-open) takes focus. A 1s tick guarantees the
// widget returns to the top within a bounded time. The call is a cheap
// no-op when the window is already on top, so this is safe to run
// continuously.
const ALWAYS_ON_TOP_WATCHDOG_MS = 1_000;

function formatCurrency(value: number) {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: value >= 10 ? 2 : 3,
    maximumFractionDigits: value >= 10 ? 2 : 3,
  }).format(value);
}

function shortModelName(name: string) {
  return name.replace(/^.*\//, '').replace(/[-_:]/g, ' ').slice(0, 16);
}

function syncWindowSize() {
  const shell = app.firstElementChild;
  if (!shell) {
    return;
  }

  if (resizeFrameHandle) {
    window.cancelAnimationFrame(resizeFrameHandle);
  }

  resizeFrameHandle = window.requestAnimationFrame(() => {
    const rect = shell.getBoundingClientRect();
    void getCurrentWindow().setSize(
      new LogicalSize(
        Math.ceil(rect.width + WINDOW_FRAME_PADDING),
        Math.ceil(rect.height + WINDOW_FRAME_PADDING),
      ),
    ).catch((error) => {
      console.error('Failed to resize window', error);
    });
    resizeFrameHandle = undefined;
  });
}

function createElement<K extends keyof HTMLElementTagNameMap>(
  tagName: K,
  className?: string,
  textContent?: string,
) {
  const element = document.createElement(tagName);

  if (className) {
    element.className = className;
  }

  if (textContent !== undefined) {
    element.textContent = textContent;
  }

  return element;
}

function topModelCard(model: ModelCost, index: number) {
  const accentClass = ['accent-cyan', 'accent-lime', 'accent-orange'][index] ?? '';

  console.log(`[debug] topModelCard #${index}:`, {
    name: model.name,
    shortName: shortModelName(model.name),
    cost: model.cost,
    formatted: formatCurrency(model.cost),
    share: model.share,
  });

  const card = createElement('div', `model-card ${accentClass}`.trim());
  card.append(
    createElement('span', 'model-name', shortModelName(model.name)),
    createElement('span', 'model-cost', formatCurrency(model.cost)),
  );

  return card;
}

function emptyModelCard(accentClass: string) {
  const card = createElement('div', `model-card ${accentClass}`);
  card.append(
    createElement('span', 'empty-state', 'No data'),
    createElement('span', 'model-cost', '—'),
  );

  return card;
}

function createOpenRouterButton() {
  const button = createElement('button', 'balance-icon-btn');
  button.type = 'button';
  button.setAttribute('aria-label', 'Open OpenRouter activity in browser');
  button.title = 'Open OpenRouter activity';
  // External-link glyph (arrow out of a box), drawn at 24x24 and rendered
  // at 12px with stroke-width 3 for a bold, chunky look that matches the
  // widget's brutalist style. `currentColor` keeps it in sync with the
  // button's text color so hover/active states recolor automatically.
  button.innerHTML =
    '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" focusable="false">' +
      '<g fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="square" stroke-linejoin="miter">' +
        '<path d="M14 4h6v6"/>' +
        '<path d="M20 4 10 14"/>' +
        '<path d="M20 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h5"/>' +
      '</g>' +
    '</svg>';
  button.addEventListener('click', () => {
    // The helper invokes the Tauri command AND re-asserts always-on-top,
    // since spawning the browser can cause the compositor to re-stack.
    void openOpenRouterActivity();
  });

  return button;
}

function setupWindowInteractions() {
  document.addEventListener('pointerdown', (event) => {
    if (!event.isPrimary || event.button !== 0) {
      return;
    }

    const target = event.target as HTMLElement | null;
    // Buttons (e.g. the balance icon button) and form controls handle
    // their own activation, so don't swallow the click into a drag.
    if (target?.closest('button, input, textarea, select, a')) {
      return;
    }

    event.preventDefault();
    void getCurrentWindow().startDragging().catch((error) => {
      console.error('Failed to start dragging window', error);
    });
  });
}

function renderReady(data: DashboardData, staleMessage?: string) {
  console.log('[debug] renderReady — DashboardData:', {
    balance: data.balance,
    topModels: data.topModels.map(m => ({ name: m.name, cost: m.cost, share: m.share })),
    otherModelsCount: data.otherModels.length,
    staleMessage,
  });

  const shell = createElement('div', 'widget-shell');
  const widget = createElement('section', 'or-widget');
  widget.setAttribute('aria-label', 'OpenRouter widget');

  const balanceCard = createElement('div', 'balance-card');
  const balanceRight = createElement('div', 'balance-right');
  balanceRight.append(
    createElement('span', 'balance-value', formatCurrency(data.balance)),
    createOpenRouterButton(),
  );
  balanceCard.append(
    createElement('span', 'balance-label', 'Balance'),
    balanceRight,
  );

  const modelsGrid = createElement('div', 'models-grid');
  if (data.topModels.length > 0) {
    data.topModels.slice(0, 3).forEach((model, index) => {
      modelsGrid.append(topModelCard(model, index));
    });
  } else {
    modelsGrid.append(
      emptyModelCard('accent-cyan'),
      emptyModelCard('accent-lime'),
      emptyModelCard('accent-orange'),
    );
  }

  if (staleMessage) {
    const statusLine = createElement('div', 'sync-status is-visible', staleMessage);
    widget.append(balanceCard, modelsGrid, statusLine);
  } else {
    widget.append(balanceCard, modelsGrid);
  }

  const footerParts: string[] = [];
  if (appVersion) {
    footerParts.push(`v${appVersion}`);
  }
  if (lastUpdatedAt) {
    footerParts.push(`Updated ${formatRelativeTime(lastUpdatedAt)}`);
  }
  if (footerParts.length > 0) {
    widget.append(createElement('div', 'app-version', footerParts.join(' · ')));
  }

  shell.append(widget);
  app.replaceChildren(shell);
  syncWindowSize();
}

function formatRelativeTime(date: Date): string {
  const seconds = Math.floor((Date.now() - date.getTime()) / 1000);

  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function renderMessageCard(className: 'status-card' | 'error-card', title: string, meta: string) {
  const section = createElement('section', className);
  const copyWrapper = createElement('div');
  copyWrapper.append(
    createElement('div', className === 'error-card' ? 'error-copy' : 'status-copy', title),
    createElement('div', 'meta-copy', meta),
  );
  section.append(copyWrapper);
  const footerParts: string[] = [];
  if (appVersion) {
    footerParts.push(`v${appVersion}`);
  }
  if (lastUpdatedAt) {
    footerParts.push(`Updated ${formatRelativeTime(lastUpdatedAt)}`);
  }
  if (footerParts.length > 0) {
    section.append(createElement('div', 'app-version', footerParts.join(' · ')));
  }
  app.replaceChildren(section);
  syncWindowSize();
}

function render() {
  if (state.status === 'loading') {
    renderMessageCard('status-card', 'Loading OpenRouter', 'Fetching balance and activity');
    return;
  }

  if (state.status === 'error' && !state.data) {
    renderMessageCard(
      'error-card',
      state.message ?? 'Unable to fetch data',
      'Export OPENROUTER_MANAGEMENT_KEY in your shell, or create a .env file containing it (see the title above for the paths that were searched).',
    );
    return;
  }

  if (state.data) {
    renderReady(state.data, state.staleMessage);
  }
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error('Request timed out')), ms);
    promise
      .then((value) => {
        window.clearTimeout(timer);
        resolve(value);
      })
      .catch((error) => {
        window.clearTimeout(timer);
        reject(error);
      });
  });
}

async function loadDashboard(isInitialLoad = false) {
  if (isRefreshing) {
    console.warn('[debug] loadDashboard skipped — previous refresh still in progress');
    return;
  }

  isRefreshing = true;

  try {
    const data = await withTimeout(getDashboardData(), 20_000);
    lastUpdatedAt = new Date();
    state = { status: 'ready', data };
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Unable to fetch data';
    if (!isInitialLoad && state.data) {
      state = {
        status: 'ready',
        data: state.data,
        staleMessage: 'Refresh failed — showing last data',
      };
    } else {
      state = { status: 'error', message };
    }
  } finally {
    isRefreshing = false;
  }

  render();
}

function scheduleWindowPositionSave() {
  if (savePositionHandle) {
    window.clearTimeout(savePositionHandle);
  }

  savePositionHandle = window.setTimeout(async () => {
    try {
      const position = await getCurrentWindow().outerPosition();
      await saveWindowPosition(position.x, position.y);
    } catch (error) {
      console.warn('Failed to save window position', error);
    }

    savePositionHandle = undefined;
  }, 250);
}

async function flushWindowPositionSave() {
  // Cancel the debounced save and persist the current position immediately.
  // Used on close/unload so the widget always remembers its last location
  // even if the debounce had not yet fired.
  if (savePositionHandle) {
    window.clearTimeout(savePositionHandle);
    savePositionHandle = undefined;
  }

  try {
    const position = await getCurrentWindow().outerPosition();
    await saveWindowPosition(position.x, position.y);
  } catch (error) {
    console.warn('Failed to flush window position save', error);
  }
}

async function bootstrap() {
  appVersion = await getVersion().catch(() => '');
  setupWindowInteractions();
  // Re-assert above-stack on focus/visibility too: compositors like
  // Mutter/Wayland can drop the always-on-top hint when the window loses
  // focus or stays hidden, so we re-apply it whenever the widget is
  // activated or becomes visible again.
  await ensureAlwaysOnTop();
  render();

  unlistenMove = await getCurrentWindow().onMoved(() => {
    scheduleWindowPositionSave();
  });

  await loadDashboard(true);

  refreshHandle = window.setInterval(() => {
    void loadDashboard();
  }, REFRESH_INTERVAL_MS);

  window.addEventListener('focus', () => {
    void loadDashboard();
    ensureAlwaysOnTop();
  });

  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') {
      void loadDashboard();
      ensureAlwaysOnTop();
    }
  });

  // Watchdog: re-assert always-on-top on a short interval. The focus
  // and visibility hooks above only fire on discrete transitions and
  // miss compositor-driven re-stacks (xdg-open, drag end, workspace
  // switch on Mutter/Wayland). A continuous tick closes that gap.
  alwaysOnTopHandle = window.setInterval(
    ensureAlwaysOnTop,
    ALWAYS_ON_TOP_WATCHDOG_MS,
  );
}

function ensureAlwaysOnTop() {
  return getCurrentWindow().setAlwaysOnTop(true).catch((error) => {
    console.warn('Failed to keep window always on top', error);
  });
}

async function openOpenRouterActivity() {
  // Opens the activity page in the default browser and then re-asserts
  // always-on-top, because spawning the browser causes the compositor
  // (Mutter/Wayland in particular) to re-stack and drop our above hint.
  try {
    await invoke('open_openrouter_activity');
  } catch (error) {
    console.error('Failed to open OpenRouter activity', error);
  }
  ensureAlwaysOnTop();
}

window.addEventListener('beforeunload', () => {
  if (refreshHandle) {
    window.clearInterval(refreshHandle);
  }

  if (alwaysOnTopHandle) {
    window.clearInterval(alwaysOnTopHandle);
  }

  if (unlistenMove) {
    unlistenMove();
  }

  // Best-effort flush of the window position. The widget has no in-app
  // close UI; closing is done via the window manager (Alt+F4) which
  // triggers beforeunload here. Fire-and-forget because beforeunload
  // cannot await, but the debounced onMoved save (250ms) already
  // covers any position the user settled on.
  void flushWindowPositionSave();
});

void bootstrap();
