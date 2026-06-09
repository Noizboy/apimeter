mod openrouter;

use chrono::{Datelike, Duration, Utc};
use openrouter::{ActivityItem, CreditsEnvelope};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, fs, path::{Path, PathBuf}};
use std::process::Command;
use std::time::Duration as StdDuration;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::Manager;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{LogicalPosition, Position, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModelCost {
  name: String,
  cost: f64,
  share: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DashboardData {
  balance: f64,
  top_models: Vec<ModelCost>,
  other_models: Vec<ModelCost>,
}

#[derive(Debug)]
struct AggregatedModel {
  name: String,
  cost: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WindowState {
  x: f64,
  y: f64,
}

#[tauri::command]
async fn get_dashboard_data(app: tauri::AppHandle) -> Result<DashboardData, String> {
  let key = load_management_key(&app)?;
  let client = reqwest::Client::builder()
    .user_agent("openrouter-widget/0.1.0")
    .connect_timeout(StdDuration::from_secs(5))
    .timeout(StdDuration::from_secs(10))
    .build()
    .map_err(|error| error.to_string())?;

  let activity_url = "https://openrouter.ai/api/v1/activity".to_string();
  eprintln!("[debug] activity_url: {activity_url}");
  eprintln!("[debug] key (first 8 chars): {}…", &key[..key.len().min(8)]);

  let (credits, activity) = tokio::try_join!(
    fetch_json::<CreditsEnvelope>(&client, "https://openrouter.ai/api/v1/credits", &key, "credits"),
    fetch_json::<openrouter::ActivityEnvelope>(&client, &activity_url, &key, "activity"),
  )?;

  eprintln!("[debug] credits: total_credits={}, total_usage={}", credits.data.total_credits, credits.data.total_usage);
  eprintln!("[debug] raw activity items count: {}", activity.data.len());

  // Filter to month-to-date: from the 1st of current month through yesterday
  let now = Utc::now().date_naive();
  let month_start = now.with_day(1).unwrap();
  let yesterday = now - Duration::days(1);
  let start_str = month_start.to_string();
  let end_str = yesterday.to_string();
  let raw_count = activity.data.len();
  let filtered: Vec<ActivityItem> = activity.data
    .into_iter()
    .filter(|item| {
      // date field format: "2026-06-08 00:00:00" → compare first 10 chars (YYYY-MM-DD)
      item.date.len() >= 10
        && &item.date[..10] >= start_str.as_str()
        && &item.date[..10] <= end_str.as_str()
    })
    .collect();
  eprintln!("[debug] filtered items (month-to-date): {} (excluded {})",
    filtered.len(),
    raw_count - filtered.len(),
  );
  for item in &filtered {
    eprintln!("[debug]   item: date={}, model={:?}, usage={}", item.date, item.model, item.usage);
  }

  let mut aggregated = aggregate_models(filtered);
  aggregated.sort_by(|left, right| right.cost.total_cmp(&left.cost));

  eprintln!("[debug] aggregated models (all):");
  for m in &aggregated {
    eprintln!("[debug]   {} → ${:.4}", m.name, m.cost);
  }

  let total_model_cost: f64 = aggregated.iter().map(|item| item.cost).sum();
  eprintln!("[debug] total_model_cost: {total_model_cost}");

  let model_costs = aggregated
    .into_iter()
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

  let top_models = model_costs.iter().take(3).cloned().collect::<Vec<_>>();
  let other_models = model_costs.iter().skip(3).cloned().collect::<Vec<_>>();

  eprintln!("[debug] --- TOP 3 MODELS ---");
  for (i, m) in top_models.iter().enumerate() {
    eprintln!("[debug]   #{i}: {} — ${} ({:.1}%)", m.name, m.cost, m.share);
  }
  eprintln!("[debug] other models count: {}", other_models.len());

  Ok(DashboardData {
    balance: round_money(credits.data.total_credits - credits.data.total_usage),
    top_models,
    other_models,
  })
}

#[tauri::command]
async fn save_window_position(app: tauri::AppHandle, x: f64, y: f64) -> Result<(), String> {
  if !x.is_finite() || !y.is_finite() {
    return Err("Window position must be finite numbers".to_string());
  }

  let window_state = WindowState { x, y };
  let state_path = resolve_window_state_path(&app)?;

  if let Some(parent) = state_path.parent() {
    fs::create_dir_all(parent).map_err(|error| format!("Failed to create config dir: {error}"))?;
  }

  let content = serde_json::to_string(&window_state).map_err(|error| error.to_string())?;
  fs::write(&state_path, content).map_err(|error| format!("Failed to persist window position: {error}"))
}

#[tauri::command]
async fn open_openrouter_activity() -> Result<(), String> {
  Command::new("xdg-open")
    .arg("https://openrouter.ai/activity")
    .spawn()
    .map_err(|error| format!("Failed to open OpenRouter activity: {error}"))?;

  Ok(())
}

fn aggregate_models(items: Vec<ActivityItem>) -> Vec<AggregatedModel> {
  let mut grouped: HashMap<String, AggregatedModel> = HashMap::new();

  for item in items {
    let key = item.model;
    let entry = grouped.entry(key.clone()).or_insert(AggregatedModel {
      name: key,
      cost: 0.0,
    });

    entry.cost += item.usage;
  }

  grouped.into_values().collect()
}

fn round_money(value: f64) -> f64 {
  (value * 1000.0).round() / 1000.0
}

async fn fetch_json<T>(client: &reqwest::Client, url: &str, key: &str, label: &str) -> Result<T, String>
where
  T: serde::de::DeserializeOwned,
{
  client
    .get(url)
    .bearer_auth(key)
    .send()
    .await
    .map_err(|error| format!("Failed to fetch {label}: {error}"))?
    .error_for_status()
    .map_err(|error| format!("{label} request failed: {error}"))?
    .json::<T>()
    .await
    .map_err(|error| format!("Invalid {label} response: {error}"))
}

fn resolve_window_state_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
  app.path()
    .app_config_dir()
    .map(|dir| dir.join("window-state.json"))
    .map_err(|error| error.to_string())
}

fn load_window_state(app: &tauri::AppHandle) -> Option<WindowState> {
  let path = resolve_window_state_path(app).ok()?;
  let content = fs::read_to_string(path).ok()?;
  serde_json::from_str(&content).ok()
}

fn load_management_key(app: &tauri::AppHandle) -> Result<String, String> {
  if let Ok(value) = env::var("OPENROUTER_MANAGEMENT_KEY") {
    let trimmed = value.trim();
    if !trimmed.is_empty() {
      return Ok(trimmed.to_string());
    }
  }

  let candidate_paths = resolve_env_paths(app);
  let mut checked: Vec<String> = Vec::new();

  for path in &candidate_paths {
    checked.push(path.display().to_string());
    if let Some(value) = read_key_from_env_file(path)? {
      return Ok(value);
    }
  }

  Err(format!(
    "Missing OPENROUTER_MANAGEMENT_KEY. Searched: [{}]. Export the env var or create a .env file in one of those locations.",
    checked.join(", ")
  ))
}

fn resolve_env_paths(app: &tauri::AppHandle) -> Vec<PathBuf> {
  build_env_search_paths(
    env::current_dir().ok().as_deref(),
    app.path().app_config_dir().ok().as_deref(),
    xdg_config_home().as_deref(),
    app.path().resource_dir().ok().as_deref(),
  )
}

/// Build the ordered list of `.env` candidates. Pure (testable).
///
/// Order:
/// 1. `<cwd>/.env.local`, `<cwd>/.env`
/// 2. If `cwd` is `src-tauri/`, also check the parent (project root)
///    — `cargo run` is invoked from there, so the user's project-level
///    `.env.local` lives one level up.
/// 3. Tauri identifier-based app config dir (e.g.
///    `~/.config/com.apimeter.openrouterwidget/`)
/// 4. Human-friendly fallback (e.g. `~/.config/openrouter-widget/`)
///    — matches the path users typically create manually.
/// 5. Tauri resource dir (production bundled resources).
fn build_env_search_paths(
  cwd: Option<&Path>,
  tauri_config_dir: Option<&Path>,
  xdg_config_home: Option<&Path>,
  resource_dir: Option<&Path>,
) -> Vec<PathBuf> {
  let mut paths: Vec<PathBuf> = Vec::new();
  let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

  let push = |path: PathBuf, paths: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<PathBuf>| {
    if seen.insert(path.clone()) {
      paths.push(path);
    }
  };

  if let Some(cwd) = cwd {
    push(cwd.join(".env.local"), &mut paths, &mut seen);
    push(cwd.join(".env"), &mut paths, &mut seen);

    // `cargo run` is launched from src-tauri/, but users keep .env.local
    // at the project root. Walk up one level when we're in src-tauri/.
    if cwd.file_name().map(|name| name == "src-tauri").unwrap_or(false) {
      if let Some(parent) = cwd.parent() {
        push(parent.join(".env.local"), &mut paths, &mut seen);
        push(parent.join(".env"), &mut paths, &mut seen);
      }
    }
  }

  if let Some(cfg) = tauri_config_dir {
    push(cfg.join(".env"), &mut paths, &mut seen);
    push(cfg.join(".env.local"), &mut paths, &mut seen);
  }

  if let Some(xdg) = xdg_config_home {
    let friendly = xdg.join("openrouter-widget");
    push(friendly.join(".env"), &mut paths, &mut seen);
    push(friendly.join(".env.local"), &mut paths, &mut seen);
  }

  if let Some(res) = resource_dir {
    push(res.join(".env.local"), &mut paths, &mut seen);
    push(res.join(".env"), &mut paths, &mut seen);
  }

  paths
}

/// Resolves the XDG config home: `$XDG_CONFIG_HOME` if set and non-empty,
/// otherwise `$HOME/.config`. Returns `None` if neither is set.
fn xdg_config_home() -> Option<PathBuf> {
  if let Ok(value) = env::var("XDG_CONFIG_HOME") {
    let trimmed = value.trim();
    if !trimmed.is_empty() {
      return Some(PathBuf::from(trimmed));
    }
  }
  env::var("HOME")
    .ok()
    .filter(|h| !h.trim().is_empty())
    .map(|home| PathBuf::from(home.trim()).join(".config"))
}

fn read_key_from_env_file(path: &Path) -> Result<Option<String>, String> {
  if !path.exists() {
    return Ok(None);
  }

  let env_content = fs::read_to_string(path)
    .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;

  for raw_line in env_content.lines() {
    let line = raw_line.trim();

    if line.is_empty() || line.starts_with('#') {
      continue;
    }

    if let Some(value) = line.strip_prefix("OPENROUTER_MANAGEMENT_KEY=") {
      let trimmed = value.trim().trim_matches('"').trim_matches('\'');
      if trimmed.is_empty() {
        return Err(format!("OPENROUTER_MANAGEMENT_KEY is empty in {}", path.display()));
      }

      return Ok(Some(trimmed.to_string()));
    }
  }

  Ok(None)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn aggregate_models_merges_duplicate_models() {
    let items = vec![
      ActivityItem {
        model: "openai/gpt-4o".to_string(),
        usage: 1.25,
        date: "2026-06-08 00:00:00".to_string(),
      },
      ActivityItem {
        model: "openai/gpt-4o".to_string(),
        usage: 0.75,
        date: "2026-06-08 00:00:00".to_string(),
      },
      ActivityItem {
        model: "anthropic/claude-sonnet-4".to_string(),
        usage: 3.0,
        date: "2026-06-08 00:00:00".to_string(),
      },
    ];

    let mut aggregated = aggregate_models(items);
    aggregated.sort_by(|left, right| left.name.cmp(&right.name));

    assert_eq!(aggregated.len(), 2);
    assert_eq!(aggregated[0].name, "anthropic/claude-sonnet-4");
    assert_eq!(aggregated[0].cost, 3.0);
    assert_eq!(aggregated[1].name, "openai/gpt-4o");
    assert_eq!(aggregated[1].cost, 2.0);
  }

  #[test]
  fn round_money_keeps_three_decimals() {
    assert_eq!(round_money(12.3454), 12.345);
    assert_eq!(round_money(12.3456), 12.346);
  }

  #[test]
  fn env_search_paths_includes_src_tauri_parent() {
    // When cwd is the Rust crate directory, the project's .env.local
    // sits one level up and must be on the candidate list.
    let cwd = Path::new("/home/me/proj/src-tauri");
    let tauri_cfg = Path::new("/home/me/.config/com.apimeter.openrouterwidget");
    let xdg = Path::new("/home/me/.config");
    let res = Path::new("/opt/app/resources");

    let paths = build_env_search_paths(
      Some(cwd),
      Some(tauri_cfg),
      Some(xdg),
      Some(res),
    );

    let as_strings: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();

    // CWD-relative entries come first.
    assert!(as_strings[0].ends_with("/src-tauri/.env.local"));
    assert!(as_strings[1].ends_with("/src-tauri/.env"));

    // Project root fallback must be present.
    assert!(
      as_strings.iter().any(|p| p == "/home/me/proj/.env.local"),
      "expected project root .env.local, got {as_strings:?}"
    );
    assert!(
      as_strings.iter().any(|p| p == "/home/me/proj/.env"),
      "expected project root .env, got {as_strings:?}"
    );

    // Tauri identifier-based config dir.
    assert!(as_strings.iter().any(|p| p == "/home/me/.config/com.apimeter.openrouterwidget/.env"));
    assert!(as_strings.iter().any(|p| p == "/home/me/.config/com.apimeter.openrouterwidget/.env.local"));

    // Human-friendly fallback.
    assert!(as_strings.iter().any(|p| p == "/home/me/.config/openrouter-widget/.env"));
    assert!(as_strings.iter().any(|p| p == "/home/me/.config/openrouter-widget/.env.local"));

    // Resource dir last.
    assert!(as_strings.iter().any(|p| p == "/opt/app/resources/.env.local"));
  }

  #[test]
  fn env_search_paths_skips_parent_when_not_src_tauri() {
    // When cwd is anything other than src-tauri, no parent fallback.
    let cwd = Path::new("/home/me/proj/scripts");
    let paths = build_env_search_paths(Some(cwd), None, None, None);

    let as_strings: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    assert_eq!(as_strings, vec![
      "/home/me/proj/scripts/.env.local".to_string(),
      "/home/me/proj/scripts/.env".to_string(),
    ]);
  }

  #[test]
  fn env_search_paths_dedupes_overlapping_inputs() {
    // If tauri_config_dir and the human-friendly dir collapse to the
    // same path, the result must still contain each unique path only once.
    let cwd = Path::new("/x");
    let shared = Path::new("/home/me/.config/openrouter-widget");
    let paths = build_env_search_paths(Some(cwd), Some(shared), Some(shared), None);

    let occurrences = paths
      .iter()
      .filter(|p| p.display().to_string() == "/home/me/.config/openrouter-widget/.env")
      .count();
    assert_eq!(occurrences, 1, "expected deduplicated .env, got {paths:?}");
  }

  #[test]
  fn xdg_config_home_prefers_env_var() {
    // The function reads from process env, so this test only validates
    // the value plumbing in real environments; the assertion is just
    // that a non-empty HOME produces a non-None result on Unix.
    if env::var("HOME").ok().filter(|h| !h.is_empty()).is_some() {
      assert!(xdg_config_home().is_some());
    }
  }

  #[test]
  fn position_within_monitors_accepts_top_left_of_primary() {
    // Single 1920x1080 monitor at origin, scale 1.0.
    let monitors = [(0, 0, 1920u32, 1080u32)];
    assert!(is_position_within_monitors(0.0, 0.0, 1.0, &monitors));
    assert!(is_position_within_monitors(100.0, 100.0, 1.0, &monitors));
    assert!(is_position_within_monitors(1919.0, 1079.0, 1.0, &monitors));
  }

  #[test]
  fn position_within_monitors_rejects_negative() {
    let monitors = [(0, 0, 1920u32, 1080u32)];
    assert!(!is_position_within_monitors(-1.0, 0.0, 1.0, &monitors));
    assert!(!is_position_within_monitors(0.0, -1.0, 1.0, &monitors));
  }

  #[test]
  fn position_within_monitors_rejects_beyond_bounds() {
    let monitors = [(0, 0, 1920u32, 1080u32)];
    assert!(!is_position_within_monitors(1920.0, 0.0, 1.0, &monitors));
    assert!(!is_position_within_monitors(0.0, 1080.0, 1.0, &monitors));
    assert!(!is_position_within_monitors(5000.0, 5000.0, 1.0, &monitors));
  }

  #[test]
  fn position_within_monitors_handles_offset_second_monitor() {
    // HDMI-1 above primary: 1920x1080 at (0, -1080) — vertical stacked.
    let monitors = [(0, -1080, 1920u32, 1080u32), (0, 0, 1920u32, 1080u32)];
    // Top-left at (100, -1000) → inside the upper monitor.
    assert!(is_position_within_monitors(100.0, -1000.0, 1.0, &monitors));
    // Top-left at (100, 100) → inside the lower monitor.
    assert!(is_position_within_monitors(100.0, 100.0, 1.0, &monitors));
    // x is outside both monitors' horizontal range.
    assert!(!is_position_within_monitors(2000.0, 100.0, 1.0, &monitors));
  }

  #[test]
  fn position_within_monitors_rejects_in_actual_gap() {
    // Two monitors with a 50px vertical gap between them.
    let monitors = [(0, 0, 1920u32, 1080u32), (0, 1130, 1920u32, 1080u32)];
    assert!(!is_position_within_monitors(100.0, 1100.0, 1.0, &monitors));
  }

  #[test]
  fn position_within_monitors_applies_scale_factor() {
    // 4K monitor with 2x scaling → 3840x2160 physical, 1920x1080 logical.
    let monitors = [(0, 0, 3840u32, 2160u32)];
    // Logical (1919, 1079) at scale 2.0 = physical (3838, 2158) → inside.
    assert!(is_position_within_monitors(1919.0, 1079.0, 2.0, &monitors));
    // Logical (1920, 1080) at scale 2.0 = physical (3840, 2160) → out (one past edge).
    assert!(!is_position_within_monitors(1920.0, 1080.0, 2.0, &monitors));
  }

  #[test]
  fn safe_fallback_uses_first_monitor_with_padding() {
    // Single monitor at origin.
    let monitors = [(0, 0, 1920u32, 1080u32)];
    let (x, y) = safe_fallback_position(1.0, &monitors).unwrap();
    assert_eq!((x, y), (24.0, 24.0));
  }

  #[test]
  fn safe_fallback_offsets_for_monitor_above_origin() {
    // Monitor at (0, -1080) — typical vertical-stacked secondary above primary.
    let monitors = [(0, -1080, 1920u32, 1080u32), (0, 0, 1920u32, 1080u32)];
    // Uses the FIRST monitor (the one the user is likely working from).
    let (x, y) = safe_fallback_position(1.0, &monitors).unwrap();
    assert_eq!((x, y), (24.0, -1056.0));
  }

  #[test]
  fn safe_fallback_offsets_for_offset_second_monitor() {
    // DP-1 below at (0, 1080), HDMI-1 above at (1064, 0) — the user's actual
    // setup. The clamp should land inside the FIRST monitor returned.
    let monitors = [(0, 1080, 3840u32, 1080u32), (1064, 0, 1920u32, 1080u32)];
    let (x, y) = safe_fallback_position(1.0, &monitors).unwrap();
    // First monitor is DP-1 at y=1080, so fallback is (24, 1104).
    assert_eq!((x, y), (24.0, 1104.0));
  }

  #[test]
  fn safe_fallback_divides_by_scale_factor() {
    // 4K monitor at origin, scale 2.0 → logical (12, 12) at physical (24, 24).
    let monitors = [(0, 0, 3840u32, 2160u32)];
    let (x, y) = safe_fallback_position(2.0, &monitors).unwrap();
    assert_eq!((x, y), (12.0, 12.0));
  }

  #[test]
  fn safe_fallback_returns_none_with_no_monitors() {
    let monitors: [(i32, i32, u32, u32); 0] = [];
    assert!(safe_fallback_position(1.0, &monitors).is_none());
  }
}

/// Validates a saved window position against the available monitors.
/// Pure (testable): takes a slice of `(x, y, w, h)` tuples in physical
/// pixels and the window's scale factor. Returns `true` if the saved
/// logical position's top-left corner falls inside any monitor's bounds.
fn is_position_within_monitors(
  logical_x: f64,
  logical_y: f64,
  scale: f64,
  monitors: &[(i32, i32, u32, u32)],
) -> bool {
  let scale = scale.max(0.01);
  let phys_x = logical_x * scale;
  let phys_y = logical_y * scale;
  monitors.iter().any(|&(mx, my, mw, mh)| {
    let mxf = mx as f64;
    let myf = my as f64;
    let mwf = mw as f64;
    let mhf = mh as f64;
    phys_x >= mxf && phys_x < mxf + mwf && phys_y >= myf && phys_y < myf + mhf
  })
}

/// Computes a safe fallback window position (logical pixels) inside the
/// first available monitor, with 24 physical pixels of padding from its
/// top-left (so the padding looks consistent across scale factors).
/// Pure (testable). Returns `None` if no monitor info is available.
fn safe_fallback_position(
  scale: f64,
  monitors: &[(i32, i32, u32, u32)],
) -> Option<(f64, f64)> {
  let scale = scale.max(0.01);
  let &(mx, my, _, _) = monitors.first()?;
  let padding_physical = 24.0_f64;
  Some((
    (mx as f64 + padding_physical) / scale,
    (my as f64 + padding_physical) / scale,
  ))
}

/// Creates a new widget window at the bottom-left of the primary monitor.
fn create_widget_window(app: &tauri::AppHandle, label: &str) -> Result<(), String> {
  let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
    .title("Apimeter")
    .inner_size(404.0, 132.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .build()
    .map_err(|e| format!("failed to build window: {e}"))?;

  // Position at bottom-left of the primary monitor
  if let Some(monitor) = app.available_monitors().ok().and_then(|list| list.into_iter().next()) {
    let scale = monitor.scale_factor();
    let mon_pos = monitor.position();
    let mon_size = monitor.size();
    let window_h = 132.0;
    let padding_phys = 24.0;

    let x_phys = mon_pos.x as f64 + padding_phys;
    let y_phys = mon_pos.y as f64 + mon_size.height as f64 - (window_h * scale) - padding_phys;

    window
      .set_position(Position::Logical(LogicalPosition::new(
        x_phys / scale,
        y_phys / scale,
      )))
      .map_err(|e| format!("failed to set position: {e}"))?;
  }

  Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_autostart::init(
      tauri_plugin_autostart::MacosLauncher::LaunchAgent,
      None,
    ))
    .setup(|app| {
      if let Some(window) = app.get_webview_window("main") {
        if let Some(saved_position) = load_window_state(app.handle()) {
          let scale = window.scale_factor().unwrap_or(1.0);
          let monitors: Vec<(i32, i32, u32, u32)> = app
            .available_monitors()
            .map(|list| {
              list
                .iter()
                .map(|m| {
                  let p = m.position();
                  let s = m.size();
                  (p.x, p.y, s.width, s.height)
                })
                .collect()
            })
            .unwrap_or_default();

          let is_visible = is_position_within_monitors(
            saved_position.x,
            saved_position.y,
            scale,
            &monitors,
          );

          if is_visible {
            let _ = window.set_position(Position::Logical(LogicalPosition::new(
              saved_position.x,
              saved_position.y,
            )));
          } else {
            match safe_fallback_position(scale, &monitors) {
              Some((x, y)) => {
                eprintln!(
                  "[diag] saved window position ({}, {}) is off-screen; \
                   falling back to ({}, {}) inside the primary monitor",
                  saved_position.x, saved_position.y, x, y
                );
                let _ = window.set_position(Position::Logical(LogicalPosition::new(x, y)));
              }
              None => {
                eprintln!(
                  "[diag] saved window position ({}, {}) is off-screen and no \
                   monitors detected; leaving window at its default position",
                  saved_position.x, saved_position.y
                );
              }
            }
          }
        }

        let _ = window.set_always_on_top(true);
        let _ = window.set_skip_taskbar(true);
      }

      // ── Window counter for multi-window labels (starts at 1 because widget-0 is the initial window) ──
      app.manage(AtomicU32::new(1));

      // ── System tray icon with context menu ──
      let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);

      let open_new = MenuItem::with_id(app, "open_new", "Open New Widget", true, None::<&str>)?;
      let close_all = MenuItem::with_id(app, "close_all", "Close All Widgets", true, None::<&str>)?;
      let separator = PredefinedMenuItem::separator(app)?;
      let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Auto-startup",
        true,
        autostart_enabled,
        None::<&str>,
      )?;

      let menu = Menu::with_items(app, &[&open_new, &close_all, &separator, &autostart])?;

      TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Apimeter — OpenRouter Widget")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
          "open_new" => {
            let state = app.state::<AtomicU32>();
            let counter = state.fetch_add(1, Ordering::SeqCst);
            let label = format!("widget-{}", counter);

            if let Err(e) = create_widget_window(app, &label) {
              eprintln!("[tray] failed to create window: {e}");
            }
          }
          "close_all" => {
            let labels: Vec<String> = app.webview_windows().keys().cloned().collect();
            for label in &labels {
              if let Some(window) = app.get_webview_window(label) {
                let _ = window.close();
              }
            }
          }
          "autostart" => {
            let is_checked = autostart.is_checked().unwrap_or(false);
            let result = if is_checked {
              app.autolaunch().enable()
            } else {
              app.autolaunch().disable()
            };
            if let Err(e) = result {
              eprintln!("[tray] autostart toggle failed: {e}");
            }
          }
          _ => {}
        })
        .build(app)?;

      // ── Open the first visible widget at bottom-left ──
      if let Err(e) = create_widget_window(app.handle(), "widget-0") {
        eprintln!("[setup] failed to create initial widget: {e}");
      }

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![get_dashboard_data, save_window_position, open_openrouter_activity])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
