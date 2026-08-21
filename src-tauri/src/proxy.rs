// proxy.rs - sing-box proxy manager (VPN-only)
use anyhow::{bail, Context, Result};
use crate::config::Config;
use crate::job::WinJob;
use directories::ProjectDirs;
use std::fs;
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use tauri::Emitter;

// ── Windows helper: spawn without console window ──────────────────
#[cfg(target_os = "windows")]
fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}
#[cfg(not(target_os = "windows"))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

// ── sing-box config builder ────────────────────────────────────
// Uses serde_json::json! instead of 30+ struct definitions

/// Lifecycle state of the VPN. A single enum (not scattered booleans) so the
/// UI and backend can never disagree about what the VPN is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnState {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl VpnState {
    /// Textual label for logs/debug.
    fn label(self) -> &'static str {
        match self {
            VpnState::Stopped => "stopped",
            VpnState::Starting => "starting",
            VpnState::Running => "running",
            VpnState::Stopping => "stopping",
        }
    }
}

pub struct ProxyManager {
    /// The live sing-box child process, if any.
    child: Arc<Mutex<Option<Child>>>,
    /// Authoritative lifecycle state.
    state: Arc<Mutex<VpnState>>,
    /// Handle to the monitor thread that watches for unexpected exit.
    monitor: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    /// Epoch token for the active monitor. Each spawn bumps it so a stale
    /// monitor (from a previous start) detects it has been superseded and
    /// exits promptly without touching current state.
    monitor_epoch: Arc<AtomicU64>,
    /// Epoch token for auto-reconnect. Bumped on manual stop() or a fresh
    /// start(); an in-flight reconnect loop checks this and aborts if it no
    /// longer matches (so manual Stop / manual Start wins over auto-retry).
    reconnect_epoch: Arc<AtomicU64>,
    /// Tray/app handle used by the monitor to emit state-change events.
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    /// Windows Job Object (kill-on-close). When dropped, the OS kills sing-box.
    job: Arc<Mutex<Option<WinJob>>>,
    config_dir: PathBuf,
    config: Config,
    active_mode: Arc<Mutex<Option<String>>>,
    dns_cache: Arc<Mutex<Option<Vec<String>>>>,
    pub debug_log_path: PathBuf,
}

// Auto-reconnect tuning: retry up to this many times after an unexpected
// drop, waiting this many seconds between attempts.
const RECONNECT_TRIES: u32 = 3;
const RECONNECT_GAP: u64 = 5;

impl ProxyManager {
    pub fn new() -> Result<Self> {
        let config = crate::config::get_active_config();
        let config_dir = ProjectDirs::from("com", "dakal-tls", "dakal-tls")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        fs::create_dir_all(&config_dir)?;

        Ok(ProxyManager {
            child: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(VpnState::Stopped)),
            monitor: Arc::new(Mutex::new(None)),
            monitor_epoch: Arc::new(AtomicU64::new(0)),
            reconnect_epoch: Arc::new(AtomicU64::new(0)),
            app_handle: Arc::new(Mutex::new(None)),
            job: Arc::new(Mutex::new(None)),
            config_dir: config_dir.clone(),
            config,
            active_mode: Arc::new(Mutex::new(None)),
            dns_cache: Arc::new(Mutex::new(None)),
            debug_log_path: config_dir.join("dakal-tls-debug.log"),
        })
    }

    /// Attach the app handle so the monitor thread can emit state events.
    /// Must be called once during setup before any start().
    pub fn init_app_handle(&self, handle: tauri::AppHandle) {
        *self.app_handle.lock().unwrap() = Some(handle);
    }

    pub fn state(&self) -> VpnState {
        *self.state.lock().unwrap()
    }

    /// Reflect reality: if our state says the process should exist but it has
    /// already exited, reset to Stopped. Returns true if a process is alive
    /// (or expected to be alive and not yet known dead).
    pub fn is_running(&self) -> bool {
        let mut guard = self.child.lock().unwrap();
        let alive = if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    // Process exited; clear it and fall through to state reset.
                    *guard = None;
                    false
                }
                Ok(None) => true,
                Err(_) => false,
            }
        } else {
            false
        };
        drop(guard);

        if !alive && self.state() != VpnState::Stopped {
            // Process died but state still thinks it's up — reset.
            self.reset_to_stopped_internal(None);
        }
        alive
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(|c| c.id())
    }

    /// Classify a technical error into a short, non-technical user message.
    /// The original error is ALWAYS preserved in the logs (debug_log + stderr),
    /// so this never throws information away.
    pub fn classify_error(&self, err: &anyhow::Error) -> String {
        let msg = err.to_string().to_lowercase();
        let raw = format!("{err:#}");

        // Permission failures (Windows TUN needs admin)
        if msg.contains("admin") || msg.contains("permission") || msg.contains("denied") {
            self.debug_log(format!("[error-classify] permission/access: {raw}"));
            return "The VPN could not start because Windows permission was denied.\nPlease restart the application with administrator permissions.".to_string();
        }
        // Server / connection failures (sing-box could not reach the server)
        if msg.contains("connection refused")
            || msg.contains("connect:")
            || msg.contains("no route")
            || msg.contains("timeout")
            || msg.contains("deadline")
            || msg.contains("handshake")
            || msg.contains("closed by peer")
        {
            self.debug_log(format!("[error-classify] connection/server: {raw}"));
            return "Could not connect to the VPN server.\nPlease check your internet connection or try again.".to_string();
        }
        // DNS / server resolution failures
        if msg.contains("resolve") || msg.contains("dns") || msg.contains("hostname") || msg.contains("no ips resolved") {
            self.debug_log(format!("[error-classify] dns/resolution: {raw}"));
            return "Could not resolve the VPN server.\nPlease check your internet connection and try again.".to_string();
        }
        // TUN / network interface failures
        if msg.contains("tun") || msg.contains("interface") || msg.contains("bind") || msg.contains("network") {
            self.debug_log(format!("[error-classify] tun/network: {raw}"));
            return "Could not start the VPN network interface.\nPlease try restarting the application.".to_string();
        }
        // Config validation failures
        if msg.contains("config") || msg.contains("validation") || msg.contains("check failed") || msg.contains("invalid") {
            self.debug_log(format!("[error-classify] config: {raw}"));
            return "VPN configuration is invalid.\nPlease check the selected profile.".to_string();
        }
        // sing-box binary / startup failures
        if msg.contains("sing-box") || msg.contains("spawn") || msg.contains("process") || msg.contains("exe") {
            self.debug_log(format!("[error-classify] startup/binary: {raw}"));
            return "The VPN could not start.\nPlease check the logs for more details.".to_string();
        }
        // Generic fallback — still logs the full error.
        self.debug_log(format!("[error-classify] generic: {raw}"));
        "The VPN could not start.\nCheck the logs for more details.".to_string()
    }

    fn debug_log(&self, msg: impl AsRef<str>) {
        use std::io::Write;
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!("[{secs}] {}\n", msg.as_ref());
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.debug_log_path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// Begin a VPN start on a background thread so the (blocking) resolve +
    /// sing-box check never freeze the Tauri main thread / UI.
    ///
    /// Returns immediately. The real outcome is delivered to the frontend via
    /// the `vpn-state` event on success, or the `vpn-error` event on failure
    /// (the UI un-freezes with a message instead of hanging).
    ///
    /// `started_at` / `is_running_cache` belong to `AppState` and are set here
    /// (on the worker thread, when the tunnel actually comes up) — the async
    /// `start()` cannot set them synchronously like the old blocking version.
    pub fn start(&mut self, started_at: Arc<Mutex<Option<Instant>>>, is_running_cache: Arc<Mutex<bool>>) {
        // A manual Start supersedes any in-flight auto-reconnect loop.
        self.reconnect_epoch.fetch_add(1, Ordering::SeqCst);

        // Prevent concurrent starts. If we are already starting/running/
        // stopping, do not spawn another sing-box instance (Test 3).
        let cur = self.state();
        if cur != VpnState::Stopped {
            self.debug_log(format!("[start] ignored: already in state {}", cur.label()));
            let msg = match cur {
                VpnState::Starting => "VPN is already starting.",
                VpnState::Running => "VPN is already running.",
                VpnState::Stopping => "VPN is stopping, please wait.",
                VpnState::Stopped => unreachable!(),
            };
            if let Some(a) = self.app_handle.lock().unwrap().as_ref() {
                let _ = a.emit("vpn-error", serde_json::json!({ "message": msg }));
            }
            return;
        }

        // Mark STARTING before any blocking work so a second call is rejected
        // even while the first is downloading / resolving.
        *self.state.lock().unwrap() = VpnState::Starting;
        self.debug_log("[start] state -> Starting");
        self.emit_state();

        // Capture the shared bits needed on the worker thread. The state
        // machine, app handle, and child/monitor live behind Arc<Mutex>, so
        // the worker can drive them without touching the main thread.
        let state = Arc::clone(&self.state);
        let app_handle = Arc::clone(&self.app_handle);
        let child = Arc::clone(&self.child);
        let monitor = Arc::clone(&self.monitor);
        let monitor_epoch = Arc::clone(&self.monitor_epoch);
        let job = Arc::clone(&self.job);
        let config_dir = self.config_dir.clone();
        let dns_cache = Arc::clone(&self.dns_cache);
        let reconnect_epoch = Arc::clone(&self.reconnect_epoch);
        let started_at = Arc::clone(&started_at);
        let is_running_cache = Arc::clone(&is_running_cache);

        thread::spawn(move || {
            // Re-read config from active mode (inside thread; config.rs reads disk)
            let cfg = crate::config::get_active_config();
            // Build a temporary manager pointing at the same shared state so we
            // can reuse start_inner's logic without holding the caller's &mut.
            let mut tmp = ProxyManager {
                child,
                state,
                monitor,
                monitor_epoch,
                app_handle,
                job,
                config_dir: config_dir.clone(),
                config: cfg,
                active_mode: Arc::new(Mutex::new(None)),
                dns_cache,
                reconnect_epoch,
                debug_log_path: config_dir.join("dakal-tls-debug.log"),
            };
            // Check admin on Windows (needed for TUN)
            #[cfg(target_os = "windows")]
            {
                extern "system" {
                    fn IsUserAnAdmin() -> i32;
                }
                // SAFETY: IsUserAnAdmin() from shell32.dll
                let is_admin = unsafe { IsUserAnAdmin() != 0 };
                if !is_admin {
                    tmp.reset_to_stopped_internal(Some("Admin required — Run as administrator."));
                    if let Some(a) = tmp.app_handle.lock().unwrap().as_ref() {
                        let _ = a.emit(
                            "vpn-error",
                            serde_json::json!({ "message": "Admin required. Right-click the app → 'Run as administrator'." }),
                        );
                    }
                    return;
                }
            }

            // Clear DNS cache on profile change to prevent IP reuse
            *tmp.dns_cache.lock().unwrap() = None;

            match tmp.start_inner(started_at.clone(), is_running_cache.clone()) {
                Ok(_) => {
                    // start_inner already set state -> Running, emitted it,
                    // and recorded the uptime start + running cache.
                }
                Err(e) => {
                    // ANY failure during startup must return to a consistent
                    // STOPPED state and leave no orphaned process (Test 4).
                    tmp.debug_log(format!("[start] failed: {e:#}"));
                    let friendly = tmp.classify_error(&e);
                    tmp.reset_to_stopped_internal(Some(&friendly));
                    if let Some(a) = tmp.app_handle.lock().unwrap().as_ref() {
                        let _ = a.emit(
                            "vpn-error",
                            serde_json::json!({ "message": friendly }),
                        );
                    }
                }
            }
        });
    }

    /// Auto-reconnect after an *unexpected* drop. Retries `RECONNECT_TRIES`
    /// times with `RECONNECT_GAP` seconds between attempts, reusing the same
    /// startup path as `start()`. Aborts immediately if `reconnect_epoch`
    /// changes (manual Stop or a fresh manual Start supersedes the loop).
    /// On final failure, emits a "stopped" event so the UI shows disconnect.
    pub fn schedule_reconnect(
        child: Arc<Mutex<Option<Child>>>,
        state: Arc<Mutex<VpnState>>,
        monitor: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
        monitor_epoch: Arc<AtomicU64>,
        app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
        job: Arc<Mutex<Option<WinJob>>>,
        config_dir: PathBuf,
        dns_cache: Arc<Mutex<Option<Vec<String>>>>,
        started_at: Arc<Mutex<Option<Instant>>>,
        is_running_cache: Arc<Mutex<bool>>,
        reconnect_epoch: Arc<AtomicU64>,
    ) {
        let my_epoch = reconnect_epoch.fetch_add(1, Ordering::SeqCst) + 1;

        thread::spawn(move || {
            // Small settle so the just-died process/TUN is fully gone.
            std::thread::sleep(std::time::Duration::from_millis(800));

            for attempt in 1..=RECONNECT_TRIES {
                // Abort if superseded (manual stop / manual start) or already up.
                if reconnect_epoch.load(Ordering::SeqCst) != my_epoch {
                    return;
                }
                if *state.lock().unwrap() == VpnState::Running {
                    return;
                }

                if let Some(app) = app_handle.lock().unwrap().as_ref() {
                    let _ = app.emit(
                        "vpn-state",
                        serde_json::json!({
                            "state": "reconnecting",
                            "message": format!("Connection lost — reconnecting… ({attempt}/{RECONNECT_TRIES})")
                        }),
                    );
                }

                // Build a temporary manager sharing the live state/child so we
                // can reuse start_inner() exactly like start().
                let mut tmp = ProxyManager {
                    child: Arc::clone(&child),
                    state: Arc::clone(&state),
                    monitor: Arc::clone(&monitor),
                    monitor_epoch: Arc::clone(&monitor_epoch),
                    app_handle: Arc::clone(&app_handle),
                    job: Arc::clone(&job),
                    config_dir: config_dir.clone(),
                    config: crate::config::get_active_config(),
                    active_mode: Arc::new(Mutex::new(None)),
                    dns_cache: Arc::clone(&dns_cache),
                    reconnect_epoch: Arc::clone(&reconnect_epoch),
                    debug_log_path: config_dir.join("dakal-tls-debug.log"),
                };

                *tmp.dns_cache.lock().unwrap() = None;
                match tmp.start_inner(started_at.clone(), is_running_cache.clone()) {
                    Ok(_) => {
                        return; // success — start_inner set clocks; leave tunnel up
                    }
                    Err(e) => {
                        tmp.debug_log(format!("[reconnect] attempt {attempt} failed: {e:#}"));
                        tmp.reset_to_stopped_internal(None);
                        // Wait the gap before the next try (not after the last).
                        if attempt < RECONNECT_TRIES {
                            std::thread::sleep(std::time::Duration::from_secs(RECONNECT_GAP));
                        }
                    }
                }
            }

            // Exhausted retries.
            if reconnect_epoch.load(Ordering::SeqCst) == my_epoch {
                *state.lock().unwrap() = VpnState::Stopped;
                if let Some(app) = app_handle.lock().unwrap().as_ref() {
                    let _ = app.emit(
                        "vpn-state",
                        serde_json::json!({
                            "state": "stopped",
                            "message": "Connection lost and auto-reconnect failed after 3 attempts.\nClick Start to try again."
                        }),
                    );
                }
            }
        });
    }

    /// Actual startup work. `start()` wraps this to guarantee state cleanup.
    fn start_inner(
        &mut self,
        started_at: Arc<Mutex<Option<Instant>>>,
        is_running_cache: Arc<Mutex<bool>>,
    ) -> Result<String> {
        let exe = self.get_bundled_or_download()?;
        self.debug_log(format!("sing-box exe: {}", exe.display()));

        let cfg = self.build_vpn_config()?;

        let cfg_json = serde_json::to_string_pretty(&cfg)?;
        let cfg_path = self.config_dir.join("config.json");

        let current_raw = fs::read_to_string(&cfg_path).ok();
        let current = current_raw.as_deref();
        if current != Some(&cfg_json) {
            fs::write(&cfg_path, &cfg_json)?;
            self.debug_log(format!("config written to {}", cfg_path.display()));
        } else {
            self.debug_log("config unchanged, skipping write");
        }

        // Validate config before launch (no window). Bounded with a timeout so
        // a config that hangs validation (e.g. Hysteria2 on sing-box 1.13.x)
        // surfaces an error instead of blocking the worker thread forever.
        self.debug_log("running sing-box check...");
        let mut cmd = Command::new(&exe);
        let mut child_check = no_window(&mut cmd)
            .arg("check")
            .arg("-c")
            .arg(&cfg_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to run sing-box check")?;
        let check_timeout = std::time::Duration::from_secs(8);
        let check_deadline = std::time::Instant::now() + check_timeout;
        let mut check_ok = false;
        loop {
            match child_check.try_wait() {
                Ok(Some(status)) => {
                    check_ok = status.success();
                    break;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= check_deadline {
                        let _ = child_check.kill();
                        let _ = child_check.wait();
                        self.debug_log("sing-box check timed out after 8s (hung validation?)");
                        bail!("Configuration validation timed out.\nThe server profile could not be verified. Try ShadowTLS, or reconnect on a different network.");
                    }
                    thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    self.debug_log(format!("sing-box check wait error: {e}"));
                    bail!("Configuration validation failed to run.");
                }
            }
        }
        if !check_ok {
            let err_text = {
                let mut buf = String::new();
                if let Some(mut o) = child_check.stderr.take() {
                    let _ = o.read_to_string(&mut buf);
                }
                let mut out = String::new();
                if let Some(mut o) = child_check.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                format!("{buf}{out}")
            };
            self.debug_log(format!("config check FAILED: {err_text}"));
            bail!(
                "Config validation failed:\n{}{}\nConfig: {}",
                err_text.trim(),
                "",
                cfg_path.display()
            );
        }
        self.debug_log("config check passed");

        #[cfg(target_os = "windows")]
        let _log_file = fs::File::create(self.config_dir.join("sing-box.log"))?;

        self.debug_log("starting sing-box run...");
        // Start sing-box with hidden window on Windows
        #[cfg(target_os = "windows")]
        let child = {
            no_window(&mut Command::new(&exe))
                .arg("run")
                .arg("-c")
                .arg(&cfg_path)
                .stdout(Stdio::from(_log_file.try_clone()?))
                .stderr(Stdio::from(_log_file))
                .spawn()?
        };
        self.debug_log("sing-box process spawned");

        #[cfg(not(target_os = "windows"))]
        let child = Command::new(&exe)
            .arg("run")
            .arg("-c")
            .arg(&cfg_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        *self.child.lock().unwrap() = Some(child);
        *self.active_mode.lock().unwrap() = Some("vpn".into());
        *self.state.lock().unwrap() = VpnState::Running;
        self.debug_log("[start] state -> Running");

        // Attach sing-box to a kill-on-close Job Object so it is terminated by
        // the OS if THIS process exits unexpectedly (e.g. Task Manager kill).
        if let Some(pid) = self.pid() {
            match WinJob::create() {
                Ok(mut job) => match job.assign_pid(pid) {
                    Ok(()) => {
                        self.debug_log(format!("[job] assigned sing-box pid {pid} to kill-on-close job"));
                        *self.job.lock().unwrap() = Some(job);
                    }
                    Err(e) => self.debug_log(format!("[job] assign failed (non-fatal): {e}")),
                },
                Err(e) => self.debug_log(format!("[job] create failed (non-fatal): {e}")),
            }
        }

        // Spawn the monitor that detects unexpected sing-box exit.
        self.spawn_monitor(started_at.clone(), is_running_cache.clone());

        // Record uptime start + mark running cache (used by ping/traffic).
        *started_at.lock().unwrap() = Some(Instant::now());
        *is_running_cache.lock().unwrap() = true;

        self.emit_state();
        Ok("VPN mode started".to_string())
    }

    /// Reset internal state to STOPPED, ensuring no child process lingers.
    /// `reason` (if any) is recorded in the debug log for troubleshooting.
    fn reset_to_stopped_internal(&self, reason: Option<&str>) {
        if let Some(r) = reason {
            self.debug_log(format!("[state] returning to Stopped: {r}"));
        } else {
            self.debug_log("[state] returning to Stopped");
        }
        // Reap any child process.
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Dropping the WinJob closes the job handle; with KillOnJobClose any
        // still-living assigned process (sing-box) is terminated by the OS.
        *self.job.lock().unwrap() = None;
        // Tell the monitor (if any) to retire cooperatively via the epoch
        // counter; it exits on its next poll. We do NOT join it — that would
        // block this command off the main thread.
        self.monitor_epoch.fetch_add(1, Ordering::SeqCst);
        *self.monitor.lock().unwrap() = None;
        *self.state.lock().unwrap() = VpnState::Stopped;
        self.emit_state();
    }

    /// Retire any running monitor WITHOUT blocking the caller. Used before a
    /// fresh start. The previous monitor detects its epoch is stale (or the
    /// empty child) and exits on its own; we never join it.
    fn stop_monitor(&self) {
        self.monitor_epoch.fetch_add(1, Ordering::SeqCst);
        *self.monitor.lock().unwrap() = None;
    }

    /// Launch a background thread that watches the sing-box child and, if it
    /// exits unexpectedly while we believe we are Running, resets state and
    /// emits a `vpn-state` event with a human-readable message (Test 5).
    ///
    /// CRITICAL lifecycle rules (Batch 2 fix):
    ///  - The monitor does NOT hold `self.child` during a wait. It polls
    ///    `try_wait()` every 400ms and releases the lock between polls, so the
    ///    main-thread status poll (frontend timer) never blocks → no UI freeze.
    ///  - Shutdown is cooperative via an epoch token, NOT a join: `stop()`/
    ///    `reset()` bump `monitor_epoch` and drop the handle; this monitor
    ///    detects the mismatch (or the now-empty child) and returns. No Tauri
    ///    command ever waits on this thread.
    fn spawn_monitor(
        &self,
        started_at: Arc<Mutex<Option<Instant>>>,
        is_running_cache: Arc<Mutex<bool>>,
    ) {
        self.stop_monitor();
        let my_epoch = self.monitor_epoch.fetch_add(1, Ordering::SeqCst) + 1;

        let child_arc = self.child.clone();
        let state_arc = self.state.clone();
        let epoch_arc = self.monitor_epoch.clone();
        let app_arc = self.app_handle.clone();
        let reconnect_epoch = self.reconnect_epoch.clone();
        let started_at = Arc::clone(&started_at);
        let is_running_cache = Arc::clone(&is_running_cache);
        let monitor_arc = self.monitor.clone();
        let monitor_epoch_arc = self.monitor_epoch.clone();
        let job_arc = self.job.clone();
        let config_dir = self.config_dir.clone();
        let dns_cache = self.dns_cache.clone();
        let log_path = self.debug_log_path.clone();

        let handle = thread::spawn(move || {
            // Poll briefly; never hold the shared child lock during a long wait.
            loop {
                // Cooperative shutdown: retire if superseded or cancelled.
                if epoch_arc.load(Ordering::SeqCst) != my_epoch {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
                if epoch_arc.load(Ordering::SeqCst) != my_epoch {
                    return;
                }

                // Acquire the lock only long enough to ask the child's status.
                let exited: Option<std::process::ExitStatus> = {
                    let mut guard = child_arc.lock().unwrap();
                    match guard.as_mut() {
                        Some(child) => match child.try_wait() {
                            Ok(Some(status)) => Some(status),
                            Ok(None) => None, // still running
                            Err(e) => {
                                let _ = fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(&log_path)
                                    .map(|mut f| {
                                        use std::io::Write;
                                        let _ = writeln!(f, "[monitor] try_wait error: {e}");
                                    });
                                Some(std::process::ExitStatus::default())
                            }
                        },
                        None => {
                            // Child was taken over by stop()/reset — nothing to watch.
                            return;
                        }
                    }
                };

                match exited {
                    Some(status) => {
                        // Process exited. Clear the shared slot.
                        *child_arc.lock().unwrap() = None;
                        let _ = fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&log_path)
                            .map(|mut f| {
                                use std::io::Write;
                                let _ = writeln!(f, "[monitor] sing-box exited unexpectedly: {status}");
                            });
                        // Only react if we are still the live monitor AND we were
                        // Running (not intentionally Stopping).
                        let live = epoch_arc.load(Ordering::SeqCst) == my_epoch
                            && *state_arc.lock().unwrap() == VpnState::Running;
                        if live {
                            // Mark stopped immediately so the UI reflects the drop,
                            // then auto-reconnect (3 tries, 5s apart). Manual Stop
                            // or a fresh manual Start bumps reconnect_epoch and
                            // makes the loop abort on its next check.
                            *state_arc.lock().unwrap() = VpnState::Stopped;
                            *is_running_cache.lock().unwrap() = false;
                            ProxyManager::schedule_reconnect(
                                child_arc,
                                state_arc,
                                monitor_arc,
                                monitor_epoch_arc,
                                app_arc,
                                job_arc,
                                config_dir,
                                dns_cache,
                                started_at,
                                is_running_cache,
                                reconnect_epoch,
                            );
                        }
                        return;
                    }
                    None => { /* still running — keep polling */ }
                }
            }
        });

        *self.monitor.lock().unwrap() = Some(handle);
    }

    /// Emit the current state to the frontend (if a handle is available).
    fn emit_state(&self) {
        if let Some(app) = self.app_handle.lock().unwrap().as_ref() {
            let s = match *self.state.lock().unwrap() {
                VpnState::Stopped => "stopped",
                VpnState::Starting => "starting",
                VpnState::Running => "running",
                VpnState::Stopping => "stopping",
            };
            let _ = app.emit("vpn-state", serde_json::json!({ "state": s }));
        }
    }

    pub fn stop(&mut self) -> Result<String> {
        // Idempotent: stopping when already stopped is harmless (Test 6).
        if self.state() == VpnState::Stopped {
            self.debug_log("[stop] ignored: already stopped");
            return Ok("Already stopped".into());
        }

        // Cancel any in-flight auto-reconnect loop: bump its epoch so the loop
        // sees a mismatch on its next check and aborts (manual Stop wins).
        self.reconnect_epoch.fetch_add(1, Ordering::SeqCst);

        // Mark STOPPING so concurrent callers are rejected and the UI can show
        // the transitional state.
        *self.state.lock().unwrap() = VpnState::Stopping;
        self.debug_log("[stop] state -> Stopping");
        self.emit_state();

        let result = self.stop_inner();
        // stop_inner always returns to Stopped (even on error), so no extra
        // cleanup needed here.
        result
    }

    /// Terminate the sing-box process and wait for it, with a bounded timeout
    /// so we never hang. Guarantees no orphaned process remains (Test 2).
    fn stop_inner(&mut self) -> Result<String> {
        let _mode = self.active_mode.lock().unwrap().take();

        // Stop the monitor first so it doesn't race with our kill/wait.
        self.stop_monitor();

        let mut guard = self.child.lock().unwrap();
        let was_running = guard.is_some();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            // Wait up to 5s for a clean exit; then reap via try_wait loop.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if std::time::Instant::now() >= deadline {
                            // Timed out waiting — process will be reaped by OS on
                            // drop; record and move on rather than hang.
                            self.debug_log("[stop] sing-box did not exit within 5s; proceeding");
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => {
                        self.debug_log(format!("[stop] wait error: {e}"));
                        break;
                    }
                }
            }
        }
        drop(guard);

        // Final reap in case anything lingers.
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        *self.state.lock().unwrap() = VpnState::Stopped;
        self.emit_state();

        if !was_running {
            return Ok("Already stopped".into());
        }
        Ok("Stopped".into())
    }

    // ── VPN / TUN mode config ─────────────────────────────────────

    fn build_vpn_config(&self) -> Result<serde_json::Value> {
        let c = &self.config;

        // Resolve the VPN server hostname to build the TUN bypass rules.
        // The hostname stays the AUTHORITATIVE sing-box outbound address; the
        // resolved IP is used ONLY to route around the local TUN interface.
        // If resolution fails we must NOT substitute a placeholder IP as the
        // server address — fail cleanly instead (see below).
        let stls_ips: Vec<String> = {
            let mut cache = self.dns_cache.lock().unwrap();
            if let Some(ips) = cache.as_ref() {
                ips.clone()
            } else {
                let ips = resolve_hostname(&c.server_address)
                    .context("VPN server hostname could not be resolved before startup")?;
                *cache = Some(ips.clone());
                ips
            }
        };

        let bypass_cidrs: Vec<String> =
            stls_ips.iter().map(|ip| format!("{ip}/32")).collect();

        // If no IPs resolved (resolve_hostname already errors above, so this
        // is only defensive), keep the hostname authoritative — do not fall
        // back to a reserved placeholder as the real server address.
        let stls_ip = stls_ips.first().cloned().unwrap_or_else(|| c.server_address.clone());
        let h2_mode = c.mode == "hysteria2";
        let final_outbound = if h2_mode { "h2-out" } else { "ss-out" };

        // WoW Split mode: whitelist (only listed domains go through VPN)
        let is_wow_mode = c.split_mode == "wow";
        
        let (route_final, default_direct) = if is_wow_mode {
            ("direct", final_outbound)  // default = direct, WoW domains → VPN
        } else {
            (final_outbound, "direct")  // default = VPN, listed domains → direct
        };

        // Build outbounds
        let mut outbounds = self.common_outbounds();
        // Patch server IP for VPN loop prevention
        for ob in outbounds.as_array_mut().unwrap() {
            if let Some(tag) = ob.get("tag").and_then(|v| v.as_str()) {
                if tag == "ss-out" || tag == "shadowtls-out" || tag == "h2-out" {
                    ob["server"] = serde_json::json!(stls_ip);
                }
            }
        }

        // Build route rules
        let mut route_rules = serde_json::json!([
            {"action": "sniff"},
            {"type": "logical", "mode": "or", "rules": [{"protocol": "dns"}, {"port": 53}], "action": "hijack-dns"},
            {"ip_cidr": bypass_cidrs, "outbound": "direct"},
            {"ip_is_private": true, "action": "route", "outbound": "direct"},
            {"type": "logical", "mode": "or", "rules": [{"port": 853}, {"protocol": "stun"}], "action": "reject"}
        ]);

        // App-rule flag (set by WoW block below)
        let mut has_app_rules = false;

        // For WoW mode, optionally add hardcoded WoW domains + user-checked apps.
        // "WoW Domains" is now a user toggle (c.wow_domains) — off means only the
        // checked apps are tunneled. Apps remain optional too; caller enforces ≥1.
        if is_wow_mode {
            let arr = route_rules.as_array_mut().unwrap();
            if c.wow_domains {
                let wow_domains = [
                "battle.net",
                "blizzard.com",
                "worldofwarcraft.com",
                "wow.com",
                "battlenet.com",
                "akamaized.net",
                "akamaihd.net",
                "akadns.net",
                "akamai.net",
                "edgecastcdn.net",
                "edgecast.net",
                "llnw.net",
                "llnw.com",
                "limelight.net",
                "cloudfront.net",
                "fastly.net",
                "level3.com",
                "level3.net",
                "blizzardcdn.com",
                "blizzard.112.2o7.net",
                "2o7.net",
                "omtrdc.net",
                "connection.wow",
                "realmlist.wow",
            ];
            for domain in wow_domains {
                arr.insert(3, serde_json::json!({"domain_suffix": [domain], "outbound": default_direct}));
            }
            } // end if c.wow_domains
            // Apps to tunnel, keyed by user-checkbox id
            let app_map: &[(&str, &[&str])] = &[
                ("discord", &["Discord.exe", "Update.exe"]),
                ("chrome", &["chrome.exe"]),
                ("telegram", &["Telegram.exe"]),
            ];
            for (id, exes) in app_map {
                if c.wow_apps.iter().any(|a| a == id) {
                    arr.insert(3, serde_json::json!({
                        "process_name": exes.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        "outbound": default_direct
                    }));
                    has_app_rules = true; // enable route.find_process so process rules match
                }
            }
        }

        Ok(serde_json::json!({
            "log": {"disabled": false, "level": "info", "timestamp": true},
            "experimental": {
                "clash_api": {
                    "external_controller": "127.0.0.1:9097",
                    "secret": "dakal",
                    "default_mode": "rule"
                }
            },
            "dns": {
                "servers": [
                    {"type": "https", "tag": "remote-doh", "server": "dns.google", "server_port": 443, "path": "/dns-query", "detour": final_outbound}
                ],
                "final": "remote-doh",
                "reverse_mapping": true,
                "strategy": "ipv4_only"
            },
            "inbounds": [{
                "type": "tun", "tag": "tun-in",
                "address": ["172.19.0.1/30"],
                // MTU 1360 = safe clamp for PMTU/blackhole on the WoW loading
                // screen (large asset transfers stall when 1400-byte segments
                // exceed the real path MTU and ICMP PMTU is broken through TUN).
                // NOTE: tcp_mss_fix is NOT available in sing-box 1.13.x — lower
                // MTU is the 1.13-compatible MSS mitigation.
                "mtu": c.mtu.unwrap_or(1360),
                "auto_route": true, "strict_route": true, "stack": c.tun_stack.as_str()
            }],
            "outbounds": outbounds,
            "route": {
                "rules": route_rules,
                "final": route_final,
                "auto_detect_interface": true,
                "default_domain_resolver": "remote-doh",
                "find_process": has_app_rules
            }
        }))
    }

    fn common_outbounds(&self) -> serde_json::Value {
        let c = &self.config;

        let mut outbounds = Vec::new();

        if c.mode == "hysteria2" {
            let mut h2 = serde_json::json!({
                "type": "hysteria2", "tag": "h2-out",
                "server": c.server_address,
                "server_ports": [format!("{}:{}", c.h2_port, c.h2_port + 4999)],
                "hop_interval": "30s",
                "up_mbps": c.h2_up_mbps,
                "down_mbps": c.h2_down_mbps,
                "password": format!("testuser1:{}", c.h2_password),
                "tls": {"enabled": true, "server_name": c.h2_sni, "insecure": c.h2_insecure}
            });
            if !c.h2_obfs.is_empty() {
                h2["obfs"] = serde_json::json!({"type": c.h2_obfs, "password": c.h2_obfs_password});
            }
            outbounds.push(h2);
        } else {
            outbounds.push(serde_json::json!({
                "type": "shadowsocks", "tag": "ss-out",
                "server": c.server_address, "server_port": c.ss_port,
                "method": "2022-blake3-chacha20-poly1305", "password": c.ss_password,
                "detour": "shadowtls-out", "udp_over_tcp": {"enabled": true}
            }));
            outbounds.push(serde_json::json!({
                "type": "shadowtls", "tag": "shadowtls-out",
                "server": c.server_address, "server_port": c.stls_port,
                "version": 3, "password": c.stls_password,
                "tls": {"enabled": true, "server_name": c.stls_sni, "insecure": false}
            }));
        }

        outbounds.push(serde_json::json!({"type": "direct", "tag": "direct"}));
        serde_json::json!(outbounds)
    }

    // ── sing-box binary management ─────────────────────────────────

    fn sing_box_exe(&self) -> PathBuf {
        self.config_dir.join("sing-box.exe")
    }

    fn get_bundled_or_download(&self) -> Result<PathBuf> {
        let candidates = [
            // Next to exe
            std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("sing-box.exe"))),
            // Next to exe/resources (Tauri bundle layout)
            std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("resources").join("sing-box.exe"))),
            // Next to exe/bin (Tauri resources with bin/ prefix)
            std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("bin").join("sing-box.exe"))),
            // Relative paths
            Some(PathBuf::from("bin").join("sing-box.exe")),
            Some(PathBuf::from("sing-box.exe")),
            // Cached in config dir
            Some(self.sing_box_exe()),
        ];
        for path in candidates.iter().flatten() {
            if path.exists() {
                println!("[stls] using sing-box: {}", path.display());
                return Ok(path.clone());
            }
        }
        println!("[stls] no bundled sing-box found, downloading...");
        self.download_sing_box()
    }

    fn download_sing_box(&self) -> Result<PathBuf> {
        let exe = self.sing_box_exe();

        // Pinned version. Unpinned "latest" can pull 1.14+ whose config schema
        // changes silently break this hand-built config.
        const PINNED_VERSION: &str = "1.13.19";

        // Stale-binary guard: if a cached exe exists but reports a different
        // version (e.g. an old 1.13.x or a pulled 1.14), drop it and re-fetch.
        if exe.exists() {
            if let Ok(out) = std::process::Command::new(&exe)
                .arg("version")
                .output()
            {
                let txt = String::from_utf8_lossy(&out.stdout);
                if !txt.contains(PINNED_VERSION) {
                    println!("[stls] cached sing-box mismatch, removing");
                    let _ = fs::remove_file(&exe);
                }
            }
        }

        if !exe.exists() {
            println!("[stls] downloading sing-box {PINNED_VERSION}...");
            let client = reqwest::blocking::Client::builder()
                .user_agent("stls")
                .build()?;

            let tag = format!("v{PINNED_VERSION}");
            let zip_name = format!("sing-box-{PINNED_VERSION}-windows-amd64.zip");
            let url = format!(
                "https://github.com/SagerNet/sing-box/releases/download/{tag}/{zip_name}"
            );

            let bytes = client.get(&url).send()?.error_for_status()?.bytes()?;

            println!("[stls] extracting...");
            let reader = std::io::Cursor::new(bytes);
            let mut archive = zip::ZipArchive::new(reader)?;

            for i in 0..archive.len() {
                let mut file = archive.by_index(i)?;
                let name = file.name().to_string();
                if name.ends_with("sing-box.exe") {
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf)?;
                    let mut out = fs::File::create(&exe)?;
                    out.write_all(&buf)?;
                    println!("[stls] sing-box ready");
                    break;
                }
            }

            if !exe.exists() {
                bail!("sing-box.exe not found in release");
            }
        }

        Ok(exe)
    }
}

// ── DNS resolver for STLS server IP (used to build TUN bypass) ────

fn resolve_hostname(host: &str) -> Result<Vec<String>> {
    // `to_socket_addrs()` is a BLOCKING DNS lookup. Bound it with a timeout so
    // an unresolvable host (e.g. on a network that can't reach the VPS NS)
    // fails fast with a friendly message instead of hanging the worker thread.
    let (tx, rx) = std::sync::mpsc::channel();
    let host_for_thread = host.to_string();
    let worker = thread::spawn(move || {
        let addr_str = format!("{host_for_thread}:0");
        let res = match addr_str.to_socket_addrs() {
            Ok(addrs) => {
                let mut ips: Vec<String> = Vec::new();
                for addr in addrs {
                    let ip = addr.ip().to_string();
                    if !ips.contains(&ip) {
                        ips.push(ip);
                    }
                }
                if ips.is_empty() {
                    Err(format!("no IPs resolved for {host_for_thread}"))
                } else {
                    Ok(ips)
                }
            }
            Err(e) => Err(format!("bootstrap DNS resolution failed: {e}")),
        };
        let _ = tx.send(res);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(8)) {
        Ok(Ok(ips)) => {
            println!("[stls] bootstrap resolved {host} -> {ips:?} (used for TUN bypass only; outbound keeps hostname {host})");
            Ok(ips)
        }
        Ok(Err(e)) => bail!("{e}"),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Detach the worker so we never join it (which would block). It
            // finishes on its own; we just surface the timeout.
            drop(worker);
            bail!("Could not resolve server '{host}' within 8s.\nCheck your internet connection, or reconnect on a network that can reach the server.")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            drop(worker);
            bail!("Could not resolve server '{host}'.\nCheck your internet connection, or reconnect on a network that can reach the server.")
        }
    }
}

// ── tests ───────────────────────────────────────────────────────────

