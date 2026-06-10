# Plan de Refactorización: Sincronización de Balance y Top 3 Modelos entre Widgets

> **Metodología: Ralph Wiggum Loop™**
>
> ```
> ┌─────────────────────────────────────────────┐
> │  🔄 TRY → 💥 BREAK → 😱 PANIC → 🔧 FIX     │
> │         ↓        ↑                          │
> │         └── LOOP ┘                          │
> └─────────────────────────────────────────────┘
> ```
>
> Cada fase documenta explícitamente: qué intentamos, qué puede romperse,
> cómo diagnosticarlo, cómo arreglarlo y cuándo hacer loop a la siguiente.

---

## 📋 Índice

1. [Diagnóstico](#1-diagnóstico)
2. [Arquitectura Objetivo](#2-arquitectura-objetivo)
3. [Fase 1: Caché en Backend](#3-fase-1-caché-en-backend-con-ttl)
4. [Fase 2: Permisos de Eventos](#4-fase-2-permisos-de-eventos-en-tauri)
5. [Fase 3: Frontend Event-Driven](#5-fase-3-frontend-event-driven)
6. [Fase 4: Singleton Fetcher (Post-MVP)](#6-fase-4-singleton-fetcher-post-mvp)
7. [Estrategia de Pruebas](#7-estrategia-de-pruebas)
8. [Riesgos y Mitigaciones](#8-riesgos-y-mitigaciones)
9. [Checklist de Liberación](#9-checklist-de-liberación)

---

## 1. Diagnóstico

### Arquitectura Actual

```
Widget-0                    Widget-1                    Widget-N
  │                            │                            │
  ├─ GET /credits              ├─ GET /credits              ├─ GET /credits
  ├─ GET /activity             ├─ GET /activity             ├─ GET /activity
  │                            │                            │
  └─ Render                    └─ Render                    └─ Render
```

### Problemas

| # | Problema | Impacto |
|---|----------|---------|
| 1 | **N llamadas API por intervalo** — 5 widgets = 5 llamadas cada 10s | Rate limiting, costo, latencia |
| 2 | **Inconsistencia visual** — Cada widget viaja en su propio ciclo polling | Widget-0 muestra T=0s, Widget-1 muestra T=3s |
| 3 | **Sin caché** — `get_dashboard_data` siempre golpea API externa | Datos duplicados, sin beneficio de temporal locality |
| 4 | **Sin broadcast** — No hay mecanismo para que un widget notifique a otros | Cada widget vive en su propio silo |
| 5 | **Sin aislamiento de errores** — Si un widget falla, los otros no lo saben | Experiencia inconsistente |

### Estado Actual de Archivos

```
src/
  main.ts          ← 548 lines: polling, state, render (vanilla TS)
  api.ts           ← 10 lines: wrappers de invoke
  types.ts         ← 18 lines: DashboardData, DashboardState, ModelCost
  styles.css       ← 240 lines: brutalist theme

src-tauri/src/
  lib.rs           ← 831 lines: commands, window mgmt, tray, fetch, tests
  openrouter.rs    ← 25 lines: structs de API response

src-tauri/
  capabilities/default.json   ← permisos core
  Cargo.toml                  ← tauri 2.1.1, tokio, reqwest, chrono
```

---

## 2. Arquitectura Objetivo

### Flujo Nuevo

```
Widget-0                   Rust Backend               Widget-1
  │                            │                          │
  │── get_dashboard_data ─────→│                          │
  │                            ├─ ¿Cache válido? ─── NO ──┤
  │                            ├─ Fetch OpenRouter API    │
  │                            ├─ Guardar en cache        │
  │                            ├─ emit("dashboard-updated")──→│
  │←─── DashboardData ────────┤                          │
  │                            │                          │←── Recibe evento
  │                            │                          ├── state = payload
  │                            │                          └── render()
  │                            │                          │
  │                            │  (3 segundos después)    │
  │                            │                          │
  │── get_dashboard_data ─────→│                          │
  │                            ├─ Cache HIT (TTL vivo)    │
  │←─── DashboardData (cache) ─┤                          │
  │                            │                          │
  │                            │  (simultáneo)            │
  │── get_dashboard_data ─────→│                          │
  │  (Widget-2 recién abierto) │                          │
  │←─── DashboardData (cache) ─┤                          │
```

### Mapa de Estados de Cada Widget

```
                ┌──────────────────────────┐
                │  Esperando evento         │◄──── heartbeat timer
                │  (heartbeat 30s)          │────► fetch propio
                └──────────┬───────────────┘
                           │
            ┌──────────────┴──────────────────┐
            │                                  │
    ┌───────▼───────┐                ┌─────────▼─────────┐
    │ Evento recibido│                │ Fetch propio       │
    │ → state = data │                │ (fallback)         │
    │ → render()     │                │ → state = data     │
    │ → reset fail=0 │                │ → render()         │
    └───────┬───────┘                │ o state = error    │
            │                        └─────────┬─────────┘
            │                                  │
            └──────────────┬───────────────────┘
                           │
                ┌──────────▼──────────┐
                │ Reseteo heartbeat    │
                │ Esperar 30s         │
                └─────────────────────┘
```

---

## 3. Fase 1: Caché en Backend con TTL

> **TRY**: Añadir `Arc<RwLock<Option<CachedData>>>` en `AppState` para cachear
> `DashboardData` con TTL de 5 segundos. Si hay cache válido, se devuelve
> inmediatamente. Si no, se hace fetch, se guarda en cache, se emite evento.

### Archivos a Modificar

- `src-tauri/src/lib.rs`

### 3.1. TRY — Código a implementar

#### Nueva estructura `CachedData`

```rust
use std::time::Instant;

#[derive(Clone, Serialize)]
struct CachedData {
    data: DashboardData,
    fetched_at: Instant,
}
```

#### Constante TTL

```rust
const CACHE_TTL: Duration = Duration::from_secs(5);
```

#### `AppState` extendido

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

struct AppState {
    client: reqwest::Client,
    data_cache: Arc<RwLock<Option<CachedData>>>,
}
```

#### Nueva dependencia en `Cargo.toml`

Ninguna — `tokio::sync` ya está en features. Verificar que `Cargo.toml` incluya:

```toml
tokio = { version = "1.43.0", features = ["macros", "sync"] }
```

✅ Ya está presente.

#### `get_dashboard_data` con lógica de caché

```rust
#[tauri::command]
async fn get_dashboard_data(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<DashboardData, String> {
    // ── Leer cache ──
    {
        let cache = state.data_cache.read().await;
        if let Some(cached) = cache.as_ref() {
            if cached.fetched_at.elapsed() < CACHE_TTL {
                eprintln!("[debug] cache HIT — returning cached data ({}ms old)",
                    cached.fetched_at.elapsed().as_millis());
                return Ok(cached.data.clone());
            }
        }
    }

    // ── Cache miss: fetch real ──
    let key = load_management_key(&app)?;
    let (credits_result, activity_result) = tokio::join!(
        fetch_json::<CreditsEnvelope>(&state.client, "https://openrouter.ai/api/v1/credits", &key, "credits"),
        fetch_json::<openrouter::ActivityEnvelope>(&state.client, "https://openrouter.ai/api/v1/activity", &key, "activity"),
    );

    let credits = credits_result?;
    let activity = match activity_result {
        Ok(envelope) => envelope,
        Err(error) => {
            eprintln!("[warn] activity fetch failed: {error}");
            openrouter::ActivityEnvelope { data: Vec::new() }
        }
    };

    // ── Procesamiento (igual que ahora) ──
    let now = Utc::now().date_naive();
    let month_start = now.with_day(1).unwrap();
    let filtered: Vec<ActivityItem> = activity.data
        .into_iter()
        .filter(|item| {
            item.date.len() >= 10
                && &item.date[..10] >= month_start.to_string().as_str()
                && &item.date[..10] <= now.to_string().as_str()
        })
        .collect();

    let mut aggregated = aggregate_models(filtered);
    aggregated.sort_by(|left, right| right.cost.total_cmp(&left.cost));

    let total_model_cost: f64 = aggregated.iter().map(|item| item.cost).sum();
    let model_costs = aggregated.into_iter()
        .map(|item| ModelCost {
            name: item.name,
            cost: round_money(item.cost),
            share: if total_model_cost > 0.0 {
                ((item.cost / total_model_cost) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            },
        })
        .collect::<Vec<_>>();

    let dashboard = DashboardData {
        balance: round_money(credits.data.total_credits - credits.data.total_usage),
        top_models: model_costs.iter().take(3).cloned().collect(),
        other_models: model_costs.iter().skip(3).cloned().collect(),
    };

    // ── Guardar en cache ──
    {
        let mut cache = state.data_cache.write().await;
        *cache = Some(CachedData {
            data: dashboard.clone(),
            fetched_at: Instant::now(),
        });
    }

    // ── Emitir evento a TODAS las ventanas ──
    use tauri::Emitter;
    if let Err(e) = app.emit("dashboard-updated", &dashboard) {
        eprintln!("[warn] failed to emit dashboard-updated event: {e}");
    }

    Ok(dashboard)
}
```

#### Inicialización en `run()`

```rust
.setup(|app| {
    // ... setup existente ...

    app.manage(AppState {
        client,
        data_cache: Arc::new(RwLock::new(None)),
    });

    // ... resto del setup ...
})
```

### 3.2. 💥 BREAK — Qué puede romperse

| Síntoma | Causa | Detección |
|---------|-------|-----------|
| Todos los widgets muestran "Refresh failed" | Cache corrupto o error de serialización | Logs de Rust muestran error de cache |
| Widgets nunca se actualizan | Cache siempre HIT, nunca fetch | Log `cache HIT` repetido sin `fetch` |
| Data salta hacia atrás | TTL muy corto + race condition escritura/lectura | Dos fetches simultáneos, el segundo sobreescribe con datos más viejos |
| Pánico en `app.emit` | Event payload no serializable | Console error en Rust |

### 3.3. 😱 PANIC — Cómo diagnosticar

```bash
# 1. Compilar y ver errores de tipo
cargo build 2>&1

# 2. Ejecutar tests existentes
cargo test

# 3. Logs en tiempo real (stderr)
cargo run 2>&1 | grep '\[debug\]\|\[warn\]'

# 4. Verificar que el cache guarda/lee
cargo run 2>&1 | grep -E 'cache HIT|cache MISS'
```

### 3.4. 🔧 FIX — Cómo arreglar

| Problema | Fix |
|----------|-----|
| Cache corrupto | Envolver en `match` con `serde_json::from_str` fallible |
| Race condition | Usar `tokio::sync::RwLock` (escritura exclusiva) |
| Evento no serializable | Asegurar `#[derive(Clone, Serialize)]` en `DashboardData` |
| TTL incorrecto | Ajustar constante `CACHE_TTL` |

### 3.5. 🔄 LOOP — Criterio para avanzar

- [ ] `cargo build` compila sin errores ni warnings
- [ ] `cargo test` pasa todas las pruebas
- [ ] Al ejecutar, logs muestran `cache MISS` en primer fetch
- [ ] Al ejecutar, logs muestran `cache HIT` en fetch inmediato posterior
- [ ] `app.emit` no produce errores en stderr

---

## 4. Fase 2: Permisos de Eventos en Tauri

> **TRY**: Añadir `core:event:default` al capability para que el frontend
> pueda hacer `listen()` de eventos.

### Archivos a Modificar

- `src-tauri/capabilities/default.json`

### 4.1. TRY

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Capability for widget windows",
  "windows": ["*"],
  "permissions": [
    "core:default",
    "core:event:default",
    "core:window:allow-start-dragging",
    "core:window:allow-outer-position",
    "core:window:allow-set-size",
    "core:window:allow-set-always-on-top"
  ]
}
```

### 4.2. 💥 BREAK

Si el permiso no está, `listen()` en frontend lanza error de permisos.
El error es silencioso (promesa rechazada) y el widget funciona con polling
nomás — no es catastrófico.

### 4.3. 😱 PANIC

```javascript
// En consola del DevTools:
listen('dashboard-updated', () => {}).catch(e => console.error(e));
// → "permission denied" si falta el permiso
```

### 4.4. 🔧 FIX

Agregar `"core:event:default"` al array de permissions.

### 4.5. 🔄 LOOP

- [ ] `cargo build` compila
- [ ] En DevTools: `listen('dashboard-updated', () => {})` no lanza error

---

## 5. Fase 3: Frontend Event-Driven

> **TRY**: Transformar el frontend de **polling puro** a **event-driven con
> heartbeat de respaldo**. Cada widget escucha `dashboard-updated` y solo
> hace fetch propio si no recibe eventos por 30 segundos.

### Archivos a Modificar

- `src/main.ts`

### 5.1. TRY — Código a implementar

#### Nueva constante

```typescript
// Reducido de 10s a 30s porque ahora los eventos son el canal principal
const HEARTBEAT_INTERVAL_MS = 30_000;
```

#### Nueva variable global

```typescript
let unlistenDashboard: (() => void) | undefined;
```

#### Listener de eventos en `bootstrap()`

```typescript
import { listen } from '@tauri-apps/api/event';

// Dentro de bootstrap(), después de render() y antes de scheduleNextRefresh():
unlistenDashboard = await listen<DashboardData>('dashboard-updated', (event) => {
  const data = event.payload;
  console.log('[event] dashboard-updated received — balance:', data.balance);

  lastUpdatedAt = new Date();
  state = { status: 'ready', data };
  consecutiveFailures = 0;

  try {
    render();
  } catch (error) {
    console.error('[event] render failed:', error);
  }
});
```

#### Heartbeat extendido a 30s

```typescript
// En scheduleNextRefresh(), usar HEARTBEAT_INTERVAL_MS en vez de REFRESH_INTERVAL_MS
function getNextDelay(): number {
  if (consecutiveFailures === 0) return HEARTBEAT_INTERVAL_MS;
  // Backoff más suave: 30s → 60s → 120s (capped)
  const factor = Math.min(consecutiveFailures, 3);
  const backoff = Math.min(
    HEARTBEAT_INTERVAL_MS * (2 ** factor),
    MAX_BACKOFF_MS,
  );
  return backoff;
}
```

#### Cleanup en `beforeunload`

```typescript
window.addEventListener('beforeunload', () => {
  if (unlistenDashboard) {
    unlistenDashboard();
    unlistenDashboard = undefined;
  }

  if (refreshHandle !== undefined) {
    window.clearTimeout(refreshHandle);
    refreshHandle = undefined;
  }
  // ... resto del cleanup existente
});
```

### 5.2. 💥 BREAK — Qué puede romperse

| Síntoma | Causa | Detección |
|---------|-------|-----------|
| Widget no se actualiza nunca | Evento no llega + heartbeat no funciona | `lastUpdatedAt` estancado, console muestra solo `[event]` |
| Widget se actualiza 2 veces seguidas | Evento + heartbeat coinciden | Console muestra `[event]` y `[poll]` casi al mismo tiempo |
| Widget muestra datos viejos tras despertar de suspensión | Staleness watchdog dispara heartbeat pero events siguen funcionando | Data no coincide con OpenRouter web |
| Error "already listening" al recargar | Tauri no limpia listeners viejos | Multiples `listen()` acumulados |
| `consecutiveFailures` nunca se resetea | Evento no actualiza la variable | Backoff llega a 120s y nunca baja |

### 5.3. 😱 PANIC — Cómo diagnosticar

```typescript
// En DevTools:
console.log({ state, lastUpdatedAt, isRefreshing, consecutiveFailures });
// Verificar que state.data tiene valores esperados

// Forzar ciclo:
loadDashboard(true);
```

### 5.4. 🔧 FIX — Cómo arreglar

| Problema | Fix |
|----------|-----|
| Dual update (evento + heartbeat) | En heartbeat, solo hacer fetch si `lastUpdatedAt` tiene más de 25s |
| Listener duplicado | Llamar `unlistenDashboard()` antes de registrar nuevo listener |
| `consecutiveFailures` no resetea | El listener de evento DEBE resetear `consecutiveFailures = 0` |
| Widget no responde | Asegurar que `listen()` se llama UNA vez en bootstrap |

#### Guard contra dual update

```typescript
// En el heartbeat:
async function heartbeatFetch() {
  if (lastUpdatedAt && Date.now() - lastUpdatedAt.getTime() < 25_000) {
    // Ya recibimos un evento reciente, no necesitamos fetch
    return;
  }
  await loadDashboard();
}
```

### 5.5. 🔄 LOOP — Criterio para avanzar

- [ ] `npm run build` compila sin errores
- [ ] Widget muestra "Loading" → recibe evento → muestra datos
- [ ] Al abrir segundo widget, ambos muestran mismos datos
- [ ] Al cerrar y reabrir widget, el listener se registra de nuevo
- [ ] Si no hay eventos por 30s, el heartbeat hace fetch
- [ ] Console no muestra errores de evento

---

## 6. Fase 4: Singleton Fetcher (Post-MVP)

> **TRY**: Cuando un nuevo widget se abre, evita el fetch inicial usando
> un comando `get_cached_dashboard` que solo lee cache sin gatillar fetch.

### Archivos a Modificar

- `src-tauri/src/lib.rs`
- `src/api.ts`
- `src/main.ts`

### 6.1. TRY

#### Nuevo comando Rust

```rust
#[tauri::command]
async fn get_cached_dashboard(
    state: tauri::State<'_, AppState>,
) -> Result<Option<DashboardData>, String> {
    let cache = state.data_cache.read().await;
    Ok(cache.as_ref().map(|c| c.data.clone()))
}
```

#### Nuevo wrapper frontend

```typescript
export async function getCachedDashboard(): Promise<DashboardData | null> {
  return invoke<DashboardData | null>('get_cached_dashboard');
}
```

#### En `bootstrap()`, intentar cache primero

```typescript
// Al inicio de bootstrap, antes del primer render:
const cached = await getCachedDashboard();
if (cached) {
  state = { status: 'ready', data: cached };
  render();
} else {
  render(); // muestra loading
}
```

#### Registrar comando

```rust
.invoke_handler(tauri::generate_handler![
    get_dashboard_data,
    get_cached_dashboard,
    save_window_position,
    open_openrouter_activity,
])
```

### 6.2. 💥 BREAK

Posible duplicación de estados: el cache devuelve datos, luego el evento
o heartbeat traen datos nuevos. El widget podría hacer un pequeño "jump"
visual.

### 6.3. 🔧 FIX

El evento siempre sobreescribe el estado. El cache solo es para el estado
inicial. Si el cache es muy reciente (< 2s), ni siquiera mostrar loading.

---

## 7. Estrategia de Pruebas

### 7.1. Pruebas Unitarias (Rust)

| Prueba | Archivo | Descripción |
|--------|---------|-------------|
| `cache_returns_same_data_within_ttl` | `lib.rs` tests | Dos llamadas seguidas devuelven el mismo `DashboardData` (misma referencia) |
| `cache_is_replaced_after_ttl` | `lib.rs` tests | Después de TTL + fetch, los valores cambian |
| `cache_preserves_data_on_fetch_error` | `lib.rs` tests | Si fetch falla, el cache no se invalida |

### 7.2. Pruebas Manuales

```
Caso 1: Widget único
  1. Iniciar app → Widget-0 aparece
  2. Balance y top 3 se muestran correctamente
  3. Esperar 30s → datos se actualizan (por evento o heartbeat)

Caso 2: Múltiples widgets
  1. Tray → "Open New Widget" → Widget-1 aparece
  2. Ambos widgets muestran exactamente el mismo balance y top 3
  3. Esperar 10s → ambos se actualizan simultáneamente

Caso 3: Cierre y reapertura
  1. Cerrar Widget-1 (Alt+F4)
  2. Widget-0 sigue funcionando y actualizándose
  3. Tray → "Open New Widget" → Widget-1 reaparece sincronizado

Caso 4: Recuperación de errores
  1. Cortar internet
  2. Widgets muestran "Refresh failed — showing last data"
  3. Restaurar internet → widgets se recuperan con datos frescos

Caso 5: Sin eventos
  1. (Simular) Bloquear evento en Rust
  2. Widgets se actualizan vía heartbeat cada 30s
```

### 7.3. Pruebas de Integración (Tauri)

```bash
# Compilar y ejecutar
cargo build && cargo test

# Verificar que el cacheo funciona (ejecutar y monitorear logs)
cargo run 2>&1 | grep -E 'cache (HIT|MISS)|dashboard-updated'
```

---

## 8. Riesgos y Mitigaciones

| Riesgo | Prob. | Impacto | Mitigación |
|--------|-------|---------|------------|
| Evento no llega a una ventana específica | Media | Alto — widget desactualizado | Heartbeat 30s como fallback |
| Race condition escritura/lectura de cache | Baja | Medio — datos inconsistentes | `tokio::sync::RwLock` con escritura exclusiva |
| TTL muy corto → muchas llamadas | Media | Medio — rate limiting | TTL configurable, monitorear logs |
| TTL muy largo → datos obsoletos | Media | Bajo — widget aceptable | Máximo 10s TTL |
| Memory leak por listeners | Baja | Medio — rendimiento | `unlistenDashboard()` en `beforeunload` |
| Event payload demasiado grande | Baja | Bajo — lag en render | `otherModels` puede ser grande, pero no se renderiza |
| Widget nuevo sin cache disponible | Alta | Bajo — muestra loading | Caso normal, primer fetch de todas formas |

---

## 9. Checklist de Liberación

### Pre-Release

- [ ] `cargo build` — 0 errores, 0 warnings
- [ ] `cargo test` — todas las pruebas pasan
- [ ] `npm run build` — TypeScript compila
- [ ] Logs de Rust no muestran `[warn]` ni `[error]` en operación normal
- [ ] 2+ widgets abiertos muestran datos consistentes por >5 minutos
- [ ] Heartbeat funciona cuando no hay eventos (simular desconectando)
- [ ] `beforeunload` limpia listeners correctamente
- [ ] No hay fugas de memoria detectables (abrir/cerrar widget 10 veces)

### Post-Release (24h)

- [ ] Monitorear `[warn] failed to emit dashboard-updated event` en logs
- [ ] Verificar que `consecutiveFailures` no se acumula sin motivo
- [ ] Confirmar reducción en llamadas a OpenRouter API

---

## Apéndice A: Código Existente Relevante

### `src-tauri/src/lib.rs` — Fragmentos clave

```rust
// Línea 44-47: AppState actual
struct AppState {
  client: reqwest::Client,
}

// Línea 49-139: get_dashboard_data actual
async fn get_dashboard_data(...) -> Result<DashboardData, String>

// Línea 693-831: run() — setup, tray, window management
pub fn run() { ... }
```

### `src/main.ts` — Fragmentos clave

```typescript
// Línea 16-28: Estado global
let state: DashboardState = { status: 'loading' };
let refreshHandle: number | undefined;
let isRefreshing = false;
let consecutiveFailures = 0;
let lastUpdatedAt: Date | null = null;

// Línea 306-352: loadDashboard()
async function loadDashboard(isInitialLoad = false)

// Línea 389-494: bootstrap()
async function bootstrap()

// Línea 514-546: beforeunload
window.addEventListener('beforeunload', () => { ... })
```

---

## Apéndice B: Diagrama de Secuencia Completo

```
                  ┌─────────────┐          ┌──────────────┐
                  │  Widget-0   │          │  Rust Backend│
                  │  (main.ts)  │          │  (lib.rs)    │
                  └──────┬──────┘          └──────┬───────┘
                         │                        │
    bootstrap():         │                        │
      ──────────────────│────────────────────────│
      render(loading)    │                        │
      listen(event) ────│────────────────────────│
                         │                        │
    Primer fetch:        │                        │
      loadDashboard() ───│─── get_dashboard_data ─→│
                         │                        ├── cache MISS
                         │                        ├── fetch OpenRouter
                         │                        ├── guardar cache
                         │                        ├── emit(event) ──┐
                         │                        │                  │
                         │←── DashboardData ──────┤                  │
                         │                        │                  │
      render(data)       │                        │                  │
                         │                        │                  │
                         │                        │          ┌───────▼────────┐
                         │                        │          │   Widget-1     │
                         │                        │          │   (main.ts)    │
                         │                        │          └───────┬────────┘
                         │                        │                  │
                         │                        │◄─── event ───────┘
                         │                        │                  │
                         │                        │       listen callback:
                         │                        │         state = data
                         │                        │         render()
                         │                        │
    Heartbeat (30s):      │                        │
      ─── timer ─────────│────────────────────────│
      loadDashboard() ───│─── get_dashboard_data ─→│
                         │                        ├── cache HIT
                         │←── DashboardData ──────┤
                         │                        │
      render(data)       │                        │
      (mismos datos)     │                        │
```

---

> **Fin del plan.** Cada fase es independiente y retrocompatible.
> El orden recomendado es secuencial: Fase 1 → Fase 2 → Fase 3 → Fase 4 (opcional).
> Rollback de cualquier fase: revertir el archivo modificado.
