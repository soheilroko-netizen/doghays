// main.rs - Tauri app entry with commands
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod proxy;
mod doh;
mod job;

use config::{Config};
#[cfg(target_os = "windows")]
fn check_single_instance() {
    use std::ffi::CString;
    use std::ptr;
    extern "system" {
        fn CreateMutexA(
            lpMutexAttributes: *mut std::ffi::c_void,
            bInitialOwner: i32,
            lpName: *const i8,
        ) -> *mut std::ffi::c_void;
        fn GetLastError() -> u32;
    }
    let name = CString::new("Local\\stls-single-instance-mutex").unwrap();
    let handle = unsafe { CreateMutexA(ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        eprintln!("[stls] CreateMutexA failed");
        return;
    }
    const ERROR_ALREADY_EXISTS: u32 = 183;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        println!("[stls] Another instance is already running — exiting.");
        std::process::exit(0);
    }
}

#[cfg(not(target_os = "windows"))]
fn check_single_instance() {}

use std::sync::Mutex;
use std::time::Instant;

static SING_BOX_CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();

fn sing_box_client() -> &'static reqwest::blocking::Client {
    SING_BOX_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .pool_max_idle_per_host(2)
            .build()
            .expect("reqwest client")
    })
}

/// Clash API (sing-box experimental) endpoint serving traffic stats
const CLASH_API_BASE: &str = "http://127.0.0.1:9097";
/// Clash API secret, must match the `secret` set in build_vpn_config()
const CLASH_API_SECRET: &str = "dakal";
/// Max log file lines returned to the UI
const LOG_LINE_LIMIT: usize = 100;
/// Minimum seconds between traffic samples before rate is reported
const TRAFFIC_SAMPLE_MIN_SECS: f64 = 0.5;
/// Ping warmup + measurement target (HTTP 204, no body)
const PING_TARGET: &str = "http://www.gstatic.com/generate_204";

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

use proxy::ProxyManager;

struct TrafficSample {
    total: (u64, u64),
    time: Instant,
}

struct AppState {
    proxy: Mutex<ProxyManager>,
    started_at: Mutex<Option<Instant>>,
    prev_sample: Mutex<Option<TrafficSample>>,
    http_client: reqwest::blocking::Client,
    cached_log: Mutex<(std::time::SystemTime, Vec<String>)>,
    is_running_cache: Mutex<bool>,
}

// ── Tray menu rebuild helper ───────────────────────────────────

fn update_tray_state(app: &tauri::AppHandle) {
    let state: tauri::State<AppState> = app.state::<AppState>();
    let running = state.proxy.lock().unwrap().is_running();
    drop(state);

    let profile = config::load_profile();
    let info = config::parse_profile(&profile);
    let server_name = info.server_name;
    let protocol_name = if info.protocol == "hysteria2" { "Hysteria2" } else { "ShadowTLS" };

    let tooltip = if running {
        format!("dakal-tls — {} | {} (connected)", server_name, protocol_name)
    } else {
        "dakal-tls VPN".to_string()
    };

    let show = MenuItemBuilder::with_id("show", "Show").build(app).unwrap();
    let hide = MenuItemBuilder::with_id("hide", "Hide").build(app).unwrap();
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app).unwrap();
    let server_item = MenuItemBuilder::with_id("server", server_name)
        .enabled(false)
        .build(app)
        .unwrap();
    let protocol_item = MenuItemBuilder::with_id("protocol", protocol_name)
        .enabled(false)
        .build(app)
        .unwrap();

    let menu = if running {
        let disc = MenuItemBuilder::with_id("disconnect", "Disconnect")
            .build(app)
            .unwrap();
        MenuBuilder::new(app)
            .item(&server_item)
            .item(&protocol_item)
            .item(&disc)
            .separator()
            .item(&show)
            .item(&hide)
            .separator()
            .item(&quit)
            .build()
            .unwrap()
    } else {
        let conn = MenuItemBuilder::with_id("connect", "Connect")
            .build(app)
            .unwrap();
        MenuBuilder::new(app)
            .item(&server_item)
            .item(&protocol_item)
            .item(&conn)
            .separator()
            .item(&show)
            .item(&hide)
            .separator()
            .item(&quit)
            .build()
            .unwrap()
    };

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tooltip));
        let _ = tray.set_menu(Some(menu));
    }
}

// ── Tauri commands ──────────────────────────────────────────────

#[tauri::command]
fn start_proxy(app: tauri::AppHandle, state: State<AppState>) -> Result<String, String> {
    start_proxy_inner(&app, &state)
}

fn start_proxy_inner(app: &tauri::AppHandle, state: &State<AppState>) -> Result<String, String> {
    let mut proxy = state.proxy.lock().unwrap();
    let result = proxy.start();
    match result {
        Ok(msg) => {
            *state.started_at.lock().unwrap() = Some(Instant::now());
            *state.is_running_cache.lock().unwrap() = proxy.is_running();
            drop(proxy);
            update_tray_state(app);
            Ok(msg)
        }
        Err(e) => {
            // Classify the technical error into a friendly user message.
            // The full error remains in the backend debug log.
            let friendly = {
                let p = state.proxy.lock().unwrap();
                p.classify_error(&e)
            };
            *state.is_running_cache.lock().unwrap() = false;
            *state.started_at.lock().unwrap() = None;
            drop(proxy);
            update_tray_state(app);
            Err(friendly)
        }
    }
}

fn stop_proxy_inner(state: &State<AppState>) -> Result<String, String> {
    let mut proxy = state.proxy.lock().unwrap();
    let result = proxy.stop();
    *state.started_at.lock().unwrap() = None;
    *state.prev_sample.lock().unwrap() = None;
    *state.is_running_cache.lock().unwrap() = false;
    drop(proxy);
    // stop() is idempotent; map its Ok("Already stopped") to a clean message.
    match result {
        Ok(_) => Ok("Stopped".into()),
        Err(e) => {
            let friendly = {
                let p = state.proxy.lock().unwrap();
                p.classify_error(&e)
            };
            Err(friendly)
        }
    }
}

#[tauri::command]
fn stop_proxy(app: tauri::AppHandle, state: State<AppState>) -> Result<String, String> {
    let result = stop_proxy_inner(&state)?;
    update_tray_state(&app);
    Ok(result)
}

#[tauri::command]
fn get_status(state: State<AppState>) -> Result<bool, String> {
    Ok(state.proxy.lock().unwrap().is_running())
}

/// Return the authoritative lifecycle state so the UI can render transitional
/// states (starting/stopping) accurately instead of guessing from a bool.
#[tauri::command]
fn get_vpn_state(state: State<AppState>) -> Result<String, String> {
    use proxy::VpnState;
    Ok(match state.proxy.lock().unwrap().state() {
        VpnState::Stopped => "stopped",
        VpnState::Starting => "starting",
        VpnState::Running => "running",
        VpnState::Stopping => "stopping",
    }
    .to_string())
}

#[tauri::command]
fn get_config() -> Result<Config, String> {
    Ok(config::get_active_config())
}

#[tauri::command]
fn set_mode(mode: String, state: State<AppState>) -> Result<String, String> {
    // Stop proxy if running (mode change requires restart)
    if state.proxy.lock().unwrap().is_running() {
        let _ = stop_proxy_inner(&state);
    }
    // Keep current server, swap protocol suffix
    let current = config::load_profile();
    let server = current
        .strip_suffix("-h2")
        .or_else(|| current.strip_suffix("-stls"))
        .unwrap_or("netherlands-1");
    let new_profile = match mode.as_str() {
        "shadowtls" => format!("{server}-stls"),
        "hysteria2" => format!("{server}-h2"),
        _ => return Err("Invalid mode".into()),
    };
    config::save_profile(&new_profile).map_err(|e| e.to_string())?;
    Ok(format!("Mode set to '{}'", mode))
}

#[tauri::command]
fn get_mode() -> Result<String, String> {
    let profile = config::load_profile();
    Ok(config::parse_profile(&profile).protocol.to_string())
}

#[tauri::command]
fn get_uptime(state: State<AppState>) -> Result<u64, String> {
    let guard = state.started_at.lock().unwrap();
    match *guard {
        Some(start) => Ok(start.elapsed().as_secs()),
        None => Ok(0),
    }
}

#[tauri::command]
fn get_full_status(state: State<AppState>) -> Result<FullStatus, String> {
    let proxy = state.proxy.lock().unwrap();
    let running = proxy.is_running();
    let pid = proxy.pid();
    let log_path = proxy.debug_log_path.clone();
    drop(proxy);

    let profile = config::load_profile();
    let mode = config::parse_profile(&profile).protocol;

    if !running {
        return Ok(FullStatus {
            running: false, mode: mode.to_string(), server: None, uptime_secs: 0, pid: None,
            traffic_up: 0, traffic_down: 0, total_up: 0, total_down: 0,
            log_lines: Vec::new(),
        });
    }

    let cfg = config::get_active_config();
    let uptime_secs = state.started_at.lock().unwrap().map(|s| s.elapsed().as_secs()).unwrap_or(0);

    // Read last 100 log lines (cache: only re-read if file changed)
    let log_lines = {
        let modified = std::fs::metadata(&log_path)
            .and_then(|m| m.modified())
            .ok();
        let mut cache = state.cached_log.lock().unwrap();
        let needs_refresh = match modified {
            Some(m) => cache.1.is_empty() || m > cache.0,
            None => false,
        };
        if needs_refresh {
            if let Ok(content) = std::fs::read_to_string(&log_path) {
                let mut lines: Vec<String> = content.lines().rev().take(LOG_LINE_LIMIT).map(String::from).collect();
                lines.reverse();
                cache.0 = std::time::SystemTime::now();
                cache.1 = lines;
            }
        }
        cache.1.clone()
    };

    // Fetch traffic stats from Clash API (only if running)
    let running = *state.is_running_cache.lock().unwrap();
    let (cur_up, cur_down) = if running {
        let client = sing_box_client();
        client
            .get(format!("{CLASH_API_BASE}/connections"))
            .header("Authorization", format!("Bearer {CLASH_API_SECRET}"))
            .timeout(std::time::Duration::from_secs(1))
            .send()
            .ok()
            .and_then(|r| r.json::<serde_json::Value>().ok())
            .map(|v| {
                let up = v["upload_total"].as_u64().unwrap_or(0);
                let down = v["download_total"].as_u64().unwrap_or(0);
                if up == 0 && down == 0 {
                    // Fallback: sum from connections array
                    v["connections"].as_array().map(|arr| {
                        arr.iter().fold((0u64, 0u64), |(u, d), c| (
                            u.saturating_add(c["upload"].as_u64().unwrap_or(0)),
                            d.saturating_add(c["download"].as_u64().unwrap_or(0)),
                        ))
                    }).unwrap_or((up, down))
                } else {
                    (up, down)
                }
            })
            .unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    let now = Instant::now();
    let mut prev_sample = state.prev_sample.lock().unwrap();
    let (traffic_up, traffic_down) = if let Some(prev) = prev_sample.as_ref() {
        let elapsed = now.duration_since(prev.time).as_secs_f64();
        if elapsed > TRAFFIC_SAMPLE_MIN_SECS {
            let up_delta = cur_up.saturating_sub(prev.total.0);
            let down_delta = cur_down.saturating_sub(prev.total.1);
            *prev_sample = Some(TrafficSample { total: (cur_up, cur_down), time: now });
            ((up_delta as f64 / elapsed) as u64, (down_delta as f64 / elapsed) as u64)
        } else {
            (0, 0)
        }
    } else {
        *prev_sample = Some(TrafficSample { total: (cur_up, cur_down), time: now });
        (0, 0)
    };

    Ok(FullStatus {
        running: true, mode: mode.to_string(), server: Some(cfg.server_address), uptime_secs, pid,
        traffic_up, traffic_down, total_up: cur_up, total_down: cur_down, log_lines,
    })
}

#[derive(serde::Serialize)]
struct FullStatus {
    running: bool,
    mode: String,
    server: Option<String>,
    uptime_secs: u64,
    pid: Option<u32>,
    traffic_up: u64,
    traffic_down: u64,
    total_up: u64,
    total_down: u64,
    log_lines: Vec<String>,
}

#[tauri::command]
fn get_log(state: State<AppState>) -> Result<String, String> {
    let proxy = state.proxy.lock().unwrap();
    if let Some(f) = std::fs::read_to_string(&proxy.debug_log_path).ok() {
        Ok(f)
    } else {
        Ok("No log available".to_string())
    }
}

#[tauri::command]
fn real_ping(state: State<AppState>) -> Result<String, String> {
    let running = *state.is_running_cache.lock().unwrap();
    if !running {
        return Err("VPN not connected".into());
    }

    // Single request with a hard 2s timeout so a dead/stalled link fails fast
    // instead of blocking the (shared) command channel for up to 6s (two GETs).
    let start = Instant::now();
    let resp = state
        .http_client
        .get(PING_TARGET)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .map_err(|e| format!("ping failed: {}", e))?;
    if !resp.status().is_success() && resp.status().as_u16() != 204 {
        return Err(format!("bad status: {}", resp.status()));
    }

    let ms = (start.elapsed().as_micros() as f64 / 1000.0) as u64;
    Ok(format!("{}ms", ms))
}

#[tauri::command]
fn apply_h2_preset(name: String) -> Result<serde_json::Value, String> {
    let (up, down) = match name.as_str() {
        "adsl" => (4, 16),
        "4g" => (15, 30),
        "5g" => (40, 80),
        "max" => (80, 120),
        _ => return Err(format!("unknown preset: {}", name)),
    };
    config::save_h2_speeds(up, down).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "up_mbps": up, "down_mbps": down }))
}

#[tauri::command]
fn get_h2_speeds() -> Result<serde_json::Value, String> {
    let (up, down) = config::load_h2_speeds();
    Ok(serde_json::json!({ "up_mbps": up, "down_mbps": down }))
}

#[tauri::command]
fn update_settings(
    mtu: Option<u32>,
    split_mode: String,
    reconnect: bool,
    wow_apps: Option<Vec<String>>,
    wow_domains: Option<bool>,
    tun_stack: Option<String>,
    state: State<AppState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    // Validate MTU
    if let Some(m) = mtu {
        if m < 576 || m > 9000 {
            return Err("MTU must be between 576 and 9000".into());
        }
    }
    
    // Validate split mode (backend supports full/wow)
    if !["full", "wow"].contains(&split_mode.as_str()) {
        return Err("Invalid split mode".into());
    }

    // Validate TUN stack (system / mixed / gvisor)
    if let Some(ref s) = tun_stack {
        if !["system", "mixed", "gvisor"].contains(&s.as_str()) {
            return Err("Invalid TUN stack".into());
        }
    }
    
    // Persist split mode
    config::save_split_mode(&split_mode).map_err(|e| e.to_string())?;

    // Persist TUN stack
    if let Some(ref s) = tun_stack {
        config::save_tun_stack(s).map_err(|e| e.to_string())?;
    }
    
    // Persist WoW checked apps when in wow mode (default all three when none passed)
    if split_mode == "wow" {
        let apps = wow_apps.unwrap_or_else(|| {
            vec!["discord".to_string(), "chrome".to_string(), "telegram".to_string()]
        });
        let use_domains = wow_domains.unwrap_or(true);
        // At least one of: WoW domains, Discord, Chrome, or Telegram must be selected.
        if !use_domains && apps.is_empty() {
            return Err(
                "Select at least one option: WoW Domains, Discord, Chrome, or Telegram".into(),
            );
        }
        config::save_wow_apps(&apps).map_err(|e| e.to_string())?;
        config::save_wow_domains(use_domains).map_err(|e| e.to_string())?;
    } else {
        // Keep persisted values in sync even when leaving wow mode.
        if let Some(d) = wow_domains {
            config::save_wow_domains(d).map_err(|e| e.to_string())?;
        }
    }
    
    // On a settings/profile change while running: stop the tunnel cleanly,
    // then tell the frontend to auto-reconnect through the SAME reliable path
    // the Connect button uses (fresh start_proxy). We deliberately do NOT
    // inline stop+start here: the Windows TUN rebind in one call is racy and
    // leaves a "running but dead" tunnel. The frontend disarms the watchdog
    // during the brief gap and re-arms it on the reconnect's successful ping.
    let reconnect_needed = if reconnect {
        let was_running = state.proxy.lock().unwrap().is_running();
        if was_running {
            let _ = stop_proxy_inner(&state);
            update_tray_state(&app);
            true
        } else {
            false
        }
    } else {
        false
    };
    
    Ok(reconnect_needed)
}

#[tauri::command]
fn get_split_settings() -> Result<serde_json::Value, String> {
    let split_mode = config::load_split_mode();
    let wow_apps = config::load_wow_apps();
    let wow_domains = config::load_wow_domains();
    Ok(serde_json::json!({
        "split_mode": split_mode,
        "wow_apps": wow_apps,
        "wow_domains": wow_domains
    }))
}





#[tauri::command]
fn get_profile() -> Result<String, String> {
    Ok(config::load_profile())
}

#[tauri::command]
fn set_profile(app: tauri::AppHandle, state: State<AppState>, profile: String) -> Result<bool, String> {
    // Save profile
    config::save_profile(&profile).map_err(|e| e.to_string())?;
    
    // On a profile change while running: stop cleanly and signal the frontend
    // to auto-reconnect via the Connect path (fresh start_proxy). The Windows
    // TUN rebind is racy if done inline, so the frontend owns the timed gap.
    let running = state.proxy.lock().unwrap().is_running();
    let reconnect_needed = if running {
        let _ = stop_proxy_inner(&state);
        true
    } else {
        false
    };
    update_tray_state(&app);
    Ok(reconnect_needed)
}

/// All selectable server variants (location + instance number)
const PROFILE_SERVERS: [&str; 3] = ["netherlands-1", "germany-3", "finland-1"];
/// Protocol suffixes mapped to config modes
const PROFILE_MODES: [&str; 2] = ["h2", "stls"];

#[tauri::command]
fn list_profiles() -> Result<Vec<String>, String> {
    let mut profiles = Vec::with_capacity(PROFILE_SERVERS.len() * PROFILE_MODES.len());
    for server in PROFILE_SERVERS {
        for mode in PROFILE_MODES {
            profiles.push(format!("{server}-{mode}"));
        }
    }
    Ok(profiles)
}

// ── DoH DNS toggle (independent of VPN) ──────────────────────────

#[tauri::command]
fn doh_set() -> Result<String, String> {
    doh::set_doh_dns().map_err(|e| e.to_string())
}

#[tauri::command]
fn doh_clear() -> Result<String, String> {
    doh::clear_doh_dns().map_err(|e| e.to_string())
}

#[tauri::command]
fn doh_active() -> Result<bool, String> {
    doh::doh_active().map_err(|e| e.to_string())
}

fn create_main_window(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("dakal")
        .inner_size(500.0, 780.0)
        .resizable(false)
        .build()?;
    Ok(())
}

fn main() {
    check_single_instance();

    let panic_log = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("stls-panic.log");
    std::fs::write(&panic_log, "stls starting...\n").ok();
    let pl = panic_log.clone();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("PANIC: {}\n", info);
        std::fs::write(&pl, &msg).ok();
    }));

    let proxy_manager = ProxyManager::new().expect("Failed to init proxy manager");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            proxy: Mutex::new(proxy_manager),
            started_at: Mutex::new(None),
            prev_sample: Mutex::new(None),
            http_client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap(),
            cached_log: Mutex::new((std::time::SystemTime::UNIX_EPOCH, Vec::new())),
            is_running_cache: Mutex::new(false),
        })
        .setup(|app| {
            // Give the proxy manager a handle so its monitor thread can emit
            // state-change events to the frontend (e.g. unexpected exit).
            {
                let state: tauri::State<AppState> = app.state::<AppState>();
                let proxy = state.proxy.lock().unwrap();
                proxy.init_app_handle(app.handle().clone());
            }

            let show_item = MenuItemBuilder::with_id("show", "Show").build(app)?;
            let hide_item = MenuItemBuilder::with_id("hide", "Hide").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let profile_startup = config::load_profile();
            let mode_startup = if profile_startup.ends_with("-h2") { "hysteria2" } else { "shadowtls" };
            let mode_item = MenuItemBuilder::with_id("mode", mode_startup)
                .enabled(false)
                .build(app)?;
            let connect_item = MenuItemBuilder::with_id("connect", "Connect").build(app)?;
            let disconnect_item = MenuItemBuilder::with_id("disconnect", "Disconnect")
                .enabled(false)
                .build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&mode_item)
                .item(&connect_item)
                .item(&disconnect_item)
                .separator()
                .item(&show_item)
                .item(&hide_item)
                .separator()
                .item(&quit_item)
                .build()?;

            let _tray = TrayIconBuilder::with_id("main")
                .tooltip("dakal-tls VPN")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                window.show().ok();
                                window.set_focus().ok();
                            }
                        }
                        "hide" => {
                            if let Some(window) = app.get_webview_window("main") {
                                window.hide().ok();
                            }
                        }
                        "connect" => {
                            let state = app.state::<AppState>();
                            let _ = start_proxy_inner(app, &state);
                            update_tray_state(app);
                        }
                        "disconnect" => {
                            let state = app.state::<AppState>();
                            let _ = stop_proxy_inner(&state);
                            update_tray_state(app);
                        }
                        "quit" => {
                            let state = app.state::<AppState>();
                            let _ = stop_proxy_inner(&state);
                            update_tray_state(app);
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().ok().unwrap_or(false) {
                                window.hide().ok();
                            } else {
                                window.show().ok();
                                window.set_focus().ok();
                            }
                        }
                    }
                })
                .build(app)?;

            update_tray_state(&app.handle());
            create_main_window(&app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    window.hide().ok();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_vpn_state,
            start_proxy,
            stop_proxy,
            get_config,
            set_mode,
            get_mode,
            real_ping,
            get_uptime,
            get_full_status,
            get_log,
            get_profile,
            set_profile,
            list_profiles,
            update_settings,

            get_h2_speeds,
            apply_h2_preset,
            doh_set,
            doh_clear,
            doh_active,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
