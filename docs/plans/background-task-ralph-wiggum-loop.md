# Plan de Refactorización: Background Task + Widgets como Puros Consumidores

> **Ralph Wiggum Loop™**
>
> ```
> ┌─────────────────────────────────────────────────────┐
> │  🔄 TRY → 💥 BREAK → 😱 PANIC → 🔧 FIX → LOOP     │
> │         ↓                                   ↑      │
> │         └───────────────────────────────────┘      │
> └─────────────────────────────────────────────────────┘
> ```
>
> Cada fase explicita: qué intentamos, qué se rompe, cómo diagnosticar,
> cómo arreglar, y cómo verificar antes de avanzar.

---

## 1. Por qué esta arquitectura es mejor

### Estado actual (acabamos de implementar)

```
Widget-0 ── get_dashboard_data ──→ Backend
                                      │
                                      ├─ ¿Cache válido? → devolver
                                      ├─ Cache miss → fetch API
                                      │                └─ guardar + emit
                                      │
Widget-1 ── get_dashboard_data ──→ Backend
Widget-N ── get_dashboard_data ──→ Backend (siguen llamando todos)
```

**Problema**: Sigue habiendo N llamadas IPC por intervalo. El cache solo
hace que sean baratas, pero el frontend sigue teniendo polling (30s),
staleness watchdog, backoff, guards, etc.

### Arquitectura objetivo

```
RUST BACKEND (el ÚNICO que hace fetch)
  │
  ├─ Background Task (tokio::spawn, cada 10s)
  │   ├─ fetch OpenRouter API
  │   ├─ guardar en last_data (estado compartido)
  │   └─ emit("dashboard-updated")  ──┐
  │                                    │
  └─ get_latest_dashboard (comando)    │
      └─ lee last_data y devuelve      │
                                       │
           ┌───────────────────────────┼──────────────┐
           ▼                           ▼              ▼
     ┌──────────┐               ┌──────────┐   ┌──────────┐
     │ Widget-0 │               │ Widget-1 │   │ Widget-N │
     │          │               │          │   │          │
     │ • init:  │               │ • init:  │   │ • init:  │
     │   get_   │               │   get_   │   │   get_   │
     │   latest │               │   latest │   │   latest │
     │ • luego  │               │ • luego  │   │ • luego  │
     │   solo   │               │   solo   │   │   solo   │
     │   eventos│               │   eventos│   │   eventos│
     └──────────┘               └──────────┘   └──────────┘
```

**Beneficios:**
- **1 llamada API cada 10s** (sin importar cuántos widgets)
- **Cero IPC calls innecesarias** — los widgets solo leen estado
- **Sincronización perfecta** — todos reciben el evento al mismo tiempo
- **Código radicalmente más simple** — sin cache TTL, sin heartbeat, sin backoff, sin guards

---

## 2. Archivos a Modificar

| Archivo | Líneas antes | Líneas después | Diferencia |
|---------|-------------|----------------|------------|
| `src-tauri/src/lib.rs` | 898 | ~830 | -68 (eliminar CachedData, TTL, lógica cache, simplificar comando) |
| `src/main.ts` | 588 | ~250 | -338 (eliminar todo el polling, heartbeat, watchdog, guards) |
| `src/api.ts` | 10 | ~5 | -5 (simplificar a un solo comando) |

---

## 3. Fase 1: Background Task en Rust

> **TRY**: Reemplazar el modelo "cache + emit on command call" por un
> background task que fetchea cada 10s + estado compartido + comando
> de solo lectura.

### 3.1. TRY — Código a implementar

#### Nuevo `AppState`

```rust
#[derive(Debug)]
struct AppState {
  client: reqwest::Client,
  last_data: Arc<RwLock<Option<DashboardData>>>,
}
```

#### Eliminar completamente

```rust
// 🗑️ Eliminar:
#[derive(Debug, Clone)]
struct CachedData { data: DashboardData, fetched_at: Instant }

// 🗑️ Eliminar:
const CACHE_TTL: StdDuration = StdDuration::from_secs(5);

// 🗑️ Eliminar de imports:
use std::time::Instant;
```

#### Nuevo comando `get_latest_dashboard`

```rust
#[tauri::command]
async fn get_latest_dashboard(
  state: tauri::State<'_, AppState>,
) -> Result<Option<DashboardData>, String> {
  Ok(state.last_data.read().await.clone())
}
```

#### Background task en `setup()`

```rust
// Dentro de .setup(|app| { ... }), después de app.manage(AppState {...}):
{
  let app_handle = app.handle().clone();
  tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    // Primer tick inmediato
    interval.tick().await; // el primer tick es inmediato en tokio
    loop {
      interval.tick().await;
      let state = app_handle.state::<AppState>();
      let key = match load_management_key(&app_handle) {
        Ok(k) => k,
        Err(e) => {
          eprintln!("[background] cannot load key: {e}");
          continue;
        }
      };
      match fetch_and_process(&state.client, &key).await {
        Ok(data) => {
          eprintln!("[background] fetch OK — balance={}, models={}",
            data.balance, data.top_models.len());
          *state.last_data.write().await = Some(data.clone());
          if let Err(e) = app_handle.emit("dashboard-updated", &data) {
            eprintln!("[background] emit failed: {e}");
          }
        }
        Err(e) => {
          eprintln!("[background] fetch FAILED: {e}");
          // No tocamos last_data — los widgets siguen con los últimos datos buenos
        }
      }
    }
  });
}
```

#### Simplificar `get_dashboard_data` (opcional: mantener como alias)

Podemos mantener el nombre `get_dashboard_data` para compatibilidad o
renombrarlo a `get_latest_dashboard`. Yo propongo:

- **Comando nuevo**: `get_latest_dashboard` — solo lectura, devuelve `Option`
- **El frontend** lo usa en init: si hay datos, los muestra instantáneamente
- **El frontend** ya no llama `get_dashboard_data` en polling

#### Registrar comando

```rust
.invoke_handler(tauri::generate_handler![
    get_latest_dashboard,
    save_window_position,
    open_openrouter_activity,
])
```

### 3.2. 💥 BREAK — Qué puede romperse

| Síntoma | Causa | Detección |
|---------|-------|-----------|
| Background task nunca arranca | Error en `tokio::spawn` o panic silencioso | No hay logs `[background]` en stderr |
| Widgets nunca reciben datos | Background task falla por key inválida y sigue en loop | Logs `[background] cannot load key` |
| Widgets muestran datos congelados | Background task crashea silenciosamente, `last_data` nunca se actualiza | Logs `[background]` se detienen |
| Primer widget abre y ve loading | Background task tarda 10s en su primer fetch real | Flash de loading en UI |
| El hilo principal se bloquea | Acceso incorrecto a `AppState` desde background task | `cargo check` lanza error de lifetime |

### 3.3. 😱 PANIC — Cómo diagnosticar

```bash
# Ver que el background task corre
cargo run 2>&1 | grep '\[background\]'

# Ver que los eventos llegan
cargo run 2>&1 | grep -E 'dashboard-updated|emit'

# Ver errores de fetch
cargo run 2>&1 | grep -E 'FAILED|cannot load key'

# Ver estado del last_data (si el comando funciona)
# Desde frontend DevTools:
await invoke('get_latest_dashboard')
```

### 3.4. 🔧 FIX — Cómo arreglar

| Problema | Fix |
|----------|-----|
| Background task crashea | Envolver todo el loop body en `catch_unwind` o un `match` externo |
| Primer fetch tarda 10s | Hacer fetch inmediato antes del `loop` (fuera del intervalo) |
| Race condition `last_data` | `tokio::sync::RwLock` protege la escritura |
| Key inválida → loop infinito de errores | Añadir backoff en background task si falla consecutivamente |

#### Primer fetch inmediato

```rust
tokio::spawn(async move {
  // Fetch inmediato al arrancar
  if let Ok(key) = load_management_key(&app_handle) {
    // fetch + store + emit
  }
  // Luego el loop normal
  let mut interval = tokio::time::interval(Duration::from_secs(10));
  loop {
    interval.tick().await;
    // ...
  }
});
```

#### Backoff en background task (opcional)

```rust
let mut consecutive_failures = 0u32;
loop {
  let delay = if consecutive_failures > 0 {
    Duration::from_secs(10 * 2u64.pow(consecutive_failures.min(3)))
  } else {
    Duration::from_secs(10)
  };
  tokio::time::sleep(delay).await;
  // fetch...
  if fetch_ok { consecutive_failures = 0; }
  else { consecutive_failures += 1; }
}
```

### 3.5. 🔄 LOOP — Criterio para avanzar

- [ ] `cargo build` compila sin errores ni warnings
- [ ] `cargo test` pasa las 17 pruebas existentes
- [ ] Si hay pruebas de caché, fallan (esperado — se elimina caché)
- [ ] Al ejecutar, `[background] fetch OK` aparece en stderr
- [ ] `get_latest_dashboard` devuelve `Some(data)` tras el primer fetch

---

## 4. Fase 2: Simplificar Frontend

> **TRY**: Eliminar todo el per-widget polling, heartbeat, staleness
> watchdog, backoff, guards, contadores de error. El frontend solo:
> 1. En init: intenta `get_latest_dashboard` (muestra datos si existen)
> 2. Escucha `dashboard-updated` para actualizaciones
> 3. Si no hay datos en init, muestra "Loading" hasta que llegue el evento

### 4.1. TRY — Código a implementar

#### Código que se elimina de `main.ts`

```typescript
// 🗑️ Eliminar variables:
let refreshHandle: number | undefined;
let isRefreshing = false;
let refreshingSince: number | null = null;
let consecutiveFailures = 0;
let lastUpdatedAt: Date | null = null;

// 🗑️ Eliminar constantes:
const HEARTBEAT_INTERVAL_MS = 30_000;
const MAX_BACKOFF_MS = 120_000;
const STALE_THRESHOLD_MS = 60_000;

// 🗑️ Eliminar funciones enteras:
function syncWindowSize()  // ⚠️ wait, esta sí se necesita
function scheduleNextRefresh()
function cancelNextRefresh()
function getNextDelay()
function checkStaleness()
function withTimeout()      // ya no se usa
```

#### Nuevo `main.ts` simplificado

```typescript
import './styles.css';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { listen } from '@tauri-apps/api/event';
import { saveWindowPosition } from './api';
import type { DashboardData, DashboardState, ModelCost } from './types';

const appRoot = document.querySelector<HTMLDivElement>('#app');
if (!appRoot) throw new Error('App root not found');
const app = appRoot;

let state: DashboardState = { status: 'loading' };
let unlistenMove: (() => void) | undefined;
let unlistenDashboard: (() => void) | undefined;
let savePositionHandle: number | undefined;
let resizeFrameHandle: number | undefined;
let alwaysOnTopHandle: number | undefined;
let appVersion = '';
let lastUpdatedAt: Date | null = null;

const WINDOW_FRAME_PADDING = 0;
const ALWAYS_ON_TOP_WATCHDOG_MS = 1_000;

// ── Formateo y render (se mantienen igual) ──
function formatCurrency(value: number) { /* igual */ }
function shortModelName(name: string) { /* igual */ }
function syncWindowSize() { /* igual */ }
function createElement(...) { /* igual */ }
function topModelCard(...) { /* igual */ }
function emptyModelCard(...) { /* igual */ }
function createOpenRouterButton() { /* igual */ }
function setupWindowInteractions() { /* igual */ }
function renderReady(data, staleMessage?) { /* igual */ }
function formatRelativeTime(date) { /* igual */ }
function renderMessageCard(...) { /* igual */ }
function render() { /* igual */ }

// ── Persistencia de posición (se mantiene igual) ──
function scheduleWindowPositionSave() { /* igual */ }
async function flushWindowPositionSave() { /* igual */ }

// ── Bootstrap simplificado ──
async function bootstrap() {
  appVersion = await getVersion().catch(() => '');
  setupWindowInteractions();
  await ensureAlwaysOnTop();
  render(); // muestra "Loading"

  unlistenMove = await getCurrentWindow().onMoved(() => {
    scheduleWindowPositionSave();
  });

  // 1. Intentar obtener datos instantáneos del estado compartido
  try {
    const cached = await invoke<DashboardData | null>('get_latest_dashboard');
    if (cached) {
      state = { status: 'ready', data: cached };
      lastUpdatedAt = new Date();
      render();
    }
  } catch (error) {
    console.warn('[init] get_latest_dashboard failed:', error);
  }

  // 2. Escuchar eventos del background task
  unlistenDashboard = await listen<DashboardData>('dashboard-updated', (event) => {
    const data = event.payload;
    lastUpdatedAt = new Date();
    state = { status: 'ready', data };
    try {
      render();
    } catch (error) {
      console.error('[event] render() threw:', error);
    }
  });

  // 3. Focus/visibility: solo re-assert always-on-top, sin fetch
  window.addEventListener('focus', () => ensureAlwaysOnTop());
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') ensureAlwaysOnTop();
  });

  // Always-on-top watchdog (igual que antes, sin staleness check)
  alwaysOnTopHandle = window.setInterval(() => {
    ensureAlwaysOnTop();
  }, ALWAYS_ON_TOP_WATCHDOG_MS);
}

function ensureAlwaysOnTop() { /* igual */ }
async function openOpenRouterActivity() { /* igual */ }

// Cleanup (simplificado)
window.addEventListener('beforeunload', () => {
  if (unlistenDashboard) { unlistenDashboard(); unlistenDashboard = undefined; }
  if (alwaysOnTopHandle) { window.clearInterval(alwaysOnTopHandle); alwaysOnTopHandle = undefined; }
  if (resizeFrameHandle) { window.cancelAnimationFrame(resizeFrameHandle); resizeFrameHandle = undefined; }
  if (savePositionHandle) { window.clearTimeout(savePositionHandle); savePositionHandle = undefined; }
  if (unlistenMove) { unlistenMove(); unlistenMove = undefined; }
  void flushWindowPositionSave();
});

void bootstrap();
```

### 4.2. 💥 BREAK — Qué puede romperse

| Síntoma | Causa | Detección |
|---------|-------|-----------|
| Widget se queda en "Loading" | Background task no ha fetcheado aún + no hay datos en last_data | UI congelada en loading |
| Widget nunca se actualiza | `listen` no registrado o evento no llega | Sin logs `[event]`, datos estáticos |
| Datos desaparecen al hacer focus/visibility | Ya no hay fetch en focus | Antes se refrescaba, ahora no |
| Error "invoke not found" | `get_latest_dashboard` no registrado en invoke_handler | Console error en DevTools |
| `lastUpdatedAt` nunca se actualiza | El evento no setea `lastUpdatedAt` | Footer muestra "Updated ... ago" incorrecto |
| Sin internet: widget se queda congelado | No hay heartbeat que detecte la falta de eventos | Usuario no sabe si los datos son actuales |

### 4.3. 😱 PANIC — Cómo diagnosticar

```typescript
// En DevTools:
console.log({ state, lastUpdatedAt });
// Ver estado actual y cuándo fue la última actualización

await invoke('get_latest_dashboard');
// Ver si hay datos en el estado compartido

// Verificar que el listener está vivo
// El callback de listen debe loguear "[event] dashboard-updated received"
```

### 4.4. 🔧 FIX — Cómo arreglar

| Problema | Fix |
|----------|-----|
| Widget en "Loading" infinito | Añadir timeout en init: si tras 15s no hay evento, mostrar error |
| Sin heartbeat para detectar muerte | Añadir un heartbeat SIMPLE (solo timer, sin backoff ni guards) que verifique `lastUpdatedAt` cada 30s y si >60s muestre advertencia |
| Focus sin fetch → datos congelados | El background task ya fetchea cada 10s, focus es innecesario. Si el usuario quiere datos frescos, el background task los traerá en <10s |
| Sin conexión | Mantener los últimos datos buenos + indicador visual de "desconectado" |

#### Heartbeat mínimo (opcional, para UX)

```typescript
// Un único setInterval que solo muestra advertencia si no hay actualizaciones
// NO hace fetch — solo es un indicador visual
const STALE_WARNING_MS = 60_000;
alwaysOnTopHandle = window.setInterval(() => {
  if (lastUpdatedAt && Date.now() - lastUpdatedAt.getTime() > STALE_WARNING_MS) {
    // Actualizar footer o mostrar indicador de desconexión
    // No hace fetch, solo informa al usuario
  }
  ensureAlwaysOnTop();
}, ALWAYS_ON_TOP_WATCHDOG_MS);
```

### 4.5. 🔄 LOOP — Criterio para avanzar

- [ ] `npx tsc --noEmit` compila sin errores
- [ ] `npx vite build` produce bundle correcto
- [ ] En runtime: widget muestra datos instantáneamente (sin flash de loading)
- [ ] En runtime: widget se actualiza cuando llega el evento
- [ ] En runtime: al hacer focus, widget NO hace fetch (solo re-assert always-on-top)
- [ ] En runtime: beforeunload limpia el listener

---

## 5. Fase 3: Limpiar Dead Code

> **TRY**: Eliminar todo el código que ya no se usa: import, función,
> estructura, constante, y dependencia.

### 5.1. TRY — Lista de eliminación

**En `src-tauri/src/lib.rs`:**
- `use std::time::Instant;` 🗑️
- `use tauri::Emitter;` 🗑️ (sigue usándose en background task — mantener)
- `struct CachedData { ... }` 🗑️
- `const CACHE_TTL` 🗑️
- `fetch_and_process` → se mantiene (lo usa el background task) ✅

**En `src/main.ts`:**
- `import { getDashboardData } from './api';` 🗑️
- `import { invoke } from '@tauri-apps/api/core';` → se mantiene ✅
- `let refreshHandle` 🗑️
- `let isRefreshing` 🗑️
- `let refreshingSince` 🗑️
- `let consecutiveFailures` 🗑️
- `HEARTBEAT_INTERVAL_MS` 🗑️
- `MAX_BACKOFF_MS` 🗑️
- `STALE_THRESHOLD_MS` 🗑️
- `loadDashboard()` 🗑️ (reemplazado por `invoke('get_latest_dashboard')`)
- `withTimeout()` 🗑️
- `scheduleWindowPositionSave()` y `flushWindowPositionSave()` → se mantienen ✅
- `cancelNextRefresh()`, `getNextDelay()`, `scheduleNextRefresh()` 🗑️
- `checkStaleness()` 🗑️
- Focus handler ya no hace fetch 🗑️
- Visibility handler ya no hace fetch 🗑️
- `openOpenRouterActivity()` → se mantiene ✅

**En `src/api.ts`:**
- `getDashboardData` 🗑️ (ya no se usa)
- `saveWindowPosition` → se mantiene ✅
- Opcional: añadir wrapper `getLatestDashboard()`

### 5.2. 💥 BREAK

Riesgo bajo. Cada cosa que se elimina tiene un reemplazo claro.
Si algo se elimina por error, el compilador/typecheck lo detecta.

### 5.3. 🔄 LOOP

- [ ] `cargo build` compila
- [ ] `cargo test` pasa
- [ ] `npx tsc --noEmit` compila
- [ ] `npx vite build` produce bundle
- [ ] No hay imports sin usar ni variables huérfanas

---

## 6. Resumen de Archivos Finales

### `src-tauri/src/lib.rs`

```rust
struct AppState {
  client: reqwest::Client,
  last_data: Arc<RwLock<Option<DashboardData>>>,
}

// Comando simple
#[tauri::command]
async fn get_latest_dashboard(...) -> Result<Option<DashboardData>, String> {
  Ok(state.last_data.read().await.clone())
}

// Background task en setup()
tokio::spawn(async move {
  // Fetch inmediato + loop cada 10s
  // store in last_data + emit("dashboard-updated")
});

// fetch_and_process() se mantiene (lo usa el background task)
```

### `src/main.ts`

```typescript
let state: DashboardState = { status: 'loading' };
let lastUpdatedAt: Date | null = null;

async function bootstrap() {
  render();                                         // loading
  const cached = await invoke('get_latest_dashboard'); // init rápido
  if (cached) { state = { status: 'ready', data: cached }; render(); }
  listen('dashboard-updated', (event) => {           // updates
    state = { status: 'ready', data: event.payload };
    render();
  });
}

// Sin polling, sin heartbeat, sin backoff, sin guards
// Sin focus/visibility fetch
// Sin loadDashboard, withTimeout, staleness watchdog
```

### `src/api.ts`

```typescript
export async function getLatestDashboard(): Promise<DashboardData | null> {
  return invoke<DashboardData | null>('get_latest_dashboard');
}
export async function saveWindowPosition(x, y): Promise<void> {
  return invoke('save_window_position', { x, y });
}
```

---

## 7. ¿Qué ganas vs qué pierdes?

| Aspecto | Antes (caché) | Después (background task) |
|---------|---------------|---------------------------|
| API calls por intervalo | N (por widget) | **1 (total)** |
| Latencia en init | 100-500ms (IPC + cache read) | **<1ms** (lectura de memoria) |
| Código backend | ~100 líneas extra (CachedData, TTL, cache logic) | **~30 líneas** (background task) |
| Código frontend | ~200 líneas (polling, heartbeat, backoff, guards) | **~30 líneas** (listen + render) |
| Resiliciencia ante API down | Media (heartbeat detecta, con delay) | **Alta** (background task reintenta, datos no se pierden) |
| Consistencia multi-widget | Alta (eventos) | **Perfecta** (todos ven mismo last_data) |
| Complejidad mental | Media | **Baja** |
| Dependencia de sistema de eventos | Sí | **Sí** (igual) |

---

## 8. Riesgos y Mitigaciones

| Riesgo | Prob. | Impacto | Mitigación |
|--------|-------|---------|------------|
| Background task crashea y no se reinicia | Baja | Alto — datos congelados | Añadir `tokio::spawn` con supervisor o al menos loguear crashes. Como mínimo, el usuario puede cerrar y abrir la app |
| Primer widget tarda 10s en ver datos si el background task no ha fetcheado aún | Media | Medio — flash de loading | Fetch inmediato antes del loop. El widget llama `get_latest_dashboard` que devuelve `None` hasta que el primer fetch termine |
| App se cierra y el background task no se limpia | Baja | Bajo — tarea huérfana | `tokio::spawn` muere con el runtime de Tauri |
| Sin internet por largos períodos: UI parece congelada | Media | Medio — usuario confundido | Añadir indicador de "última actualización" en footer (ya existe con `lastUpdatedAt`) |
| Race: background task escribe `last_data` mientras `get_latest_dashboard` lee | Baja | Ninguno — RwLock garantiza consistencia | `tokio::sync::RwLock` |

---

> **Fin del plan.** La simplificación elimina ~400 líneas de código entre
> backend y frontend, reduciendo la superficie de bugs y la complejidad
> mental. El background task es el ÚNICO punto que toca la API externa.
