// proxy.rs - sing-box proxy manager (VPN-only)
use anyhow::{bail, Context, Result};
use crate::config::Config;
use directories::ProjectDirs;
use std::fs;
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

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

/// Fallback bypass range when the server hostname cannot be resolved
const TUN_FALLBACK_CIDR: &str = "198.18.0.0/15";
/// Static placeholder IP used when no server IP was resolved
const TUN_FALLBACK_IP: &str = "198.18.0.0";

pub struct ProxyManager {
    child: Arc<Mutex<Option<Child>>>,
    config_dir: PathBuf,
    config: Config,
    active_mode: Arc<Mutex<Option<String>>>,
    dns_cache: Arc<Mutex<Option<Vec<String>>>>,
    pub debug_log_path: PathBuf,
}

impl ProxyManager {
    pub fn new() -> Result<Self> {
        let config = crate::config::get_active_config();
        let config_dir = ProjectDirs::from("com", "dakal-tls", "dakal-tls")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        fs::create_dir_all(&config_dir)?;

        Ok(ProxyManager {
            child: Arc::new(Mutex::new(None)),
            config_dir: config_dir.clone(),
            config,
            active_mode: Arc::new(Mutex::new(None)),
            dns_cache: Arc::new(Mutex::new(None)),
            debug_log_path: config_dir.join("dakal-tls-debug.log"),
        })
    }

    pub fn is_running(&self) -> bool {
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    *guard = None;
                    false
                }
                Ok(None) => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(|c| c.id())
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

    pub fn start(&mut self) -> Result<String> {
        if self.is_running() {
            bail!("Proxy already running");
        }

        // Check admin on Windows (needed for TUN)
        #[cfg(target_os = "windows")]
        {
            extern "system" {
                fn IsUserAnAdmin() -> i32;
            }
            // SAFETY: IsUserAnAdmin() from shell32.dll
            let is_admin = unsafe { IsUserAnAdmin() != 0 };
            if !is_admin {
                bail!("Admin required. Right-click stls.exe → 'Run as administrator'.");
            }
        }

        // Re-read config from active mode
        self.config = crate::config::get_active_config();
        self.debug_log("config loaded");
        
        // Clear DNS cache on profile change to prevent IP reuse
        *self.dns_cache.lock().unwrap() = None;

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

        // Validate config before launch (no window)
        self.debug_log("running sing-box check...");
        let mut cmd = Command::new(&exe);
        let check_output = no_window(&mut cmd)
            .arg("check")
            .arg("-c")
            .arg(&cfg_path)
            .output()
            .context("failed to run sing-box check")?;
        if !check_output.status.success() {
            let err_text = String::from_utf8_lossy(&check_output.stderr);
            let out_text = String::from_utf8_lossy(&check_output.stdout);
            self.debug_log(format!("config check FAILED: {err_text}{out_text}"));
            bail!(
                "Config validation failed:\n{}{}\nConfig: {}",
                err_text.trim(),
                out_text.trim(),
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

        Ok("VPN mode started".to_string())
    }

    pub fn stop(&mut self) -> Result<String> {
        let _mode = self.active_mode.lock().unwrap().take();

        let mut guard = self.child.lock().unwrap();
        let was_running = guard.is_some();
        if let Some(mut child) = guard.take() {
            child.kill()?;
            child.wait()?;
        }
        drop(guard);

        if !was_running {
            bail!("Not running");
        }

        Ok("Stopped".into())
    }

    // ── VPN / TUN mode config ─────────────────────────────────────

    fn build_vpn_config(&self) -> Result<serde_json::Value> {
        let c = &self.config;

        // Resolve STLS server IP to bypass from TUN (cached)
        let stls_ips: Vec<String> = {
            let mut cache = self.dns_cache.lock().unwrap();
            if let Some(ips) = cache.as_ref() {
                ips.clone()
            } else {
                let ips = resolve_hostname(&c.server_address).unwrap_or_else(|_| {
                    eprintln!("[stls] DNS resolution failed for {} — using hostname directly", c.server_address);
                    vec![]
                });
                *cache = Some(ips.clone());
                ips
            }
        };

        let bypass_cidrs: Vec<String> = if stls_ips.is_empty() {
            vec![TUN_FALLBACK_CIDR.into()]
        } else {
            stls_ips.iter().map(|ip| format!("{ip}/32")).collect()
        };

        let stls_ip = stls_ips.first().cloned().unwrap_or_else(|| TUN_FALLBACK_IP.into());
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
            {"ip_cidr": bypass_cidrs, "outbound": "direct"}
        ]);

        // App-rule flag (set by WoW block below)
        let mut has_app_rules = false;

        // For WoW mode, add hardcoded WoW domains + tunnel the target apps
        if is_wow_mode {
            let arr = route_rules.as_array_mut().unwrap();
            let wow_domains = [
                "battle.net",
                "blizzard.com",
                "worldofwarcraft.com",
                "akamaized.net",
            ];
            for domain in wow_domains {
                arr.insert(3, serde_json::json!({"domain_suffix": [domain], "outbound": default_direct}));
            }
            // Target apps route through the VPN (tunnel-list semantics in WoW)
            arr.insert(3, serde_json::json!({
                "process_name": ["Discord.exe", "Update.exe", "Telegram.exe", "chrome.exe"],
                "outbound": default_direct
            }));
            has_app_rules = true; // ensure route.find_process is enabled so process rules match
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
                    {"type": "https", "tag": "remote-doh", "server": "1.1.1.1", "detour": final_outbound}
                ],
                "final": "remote-doh",
                "reverse_mapping": true
            },
            "inbounds": [{
                "type": "tun", "tag": "tun-in",
                "address": ["172.19.0.1/30"],
                "mtu": c.mtu,
                "auto_route": true, "strict_route": true, "stack": "mixed"
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
                "server_ports": [format!("{}-{}", c.h2_port, c.h2_port + 4999)],
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
        if exe.exists() {
            return Ok(exe);
        }

        println!("[stls] resolving latest sing-box release...");
        let client = reqwest::blocking::Client::builder()
            .user_agent("stls")
            .build()?;

        let rel: serde_json::Value = client
            .get("https://api.github.com/repos/SagerNet/sing-box/releases/latest")
            .send()?
            .json()?;

        let tag = rel["tag_name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no tag"))?;
        let version = tag.trim_start_matches('v');

        println!("[stls] downloading sing-box {version}...");
        let zip_name = format!("sing-box-{version}-windows-amd64.zip");
        let url =
            format!("https://github.com/SagerNet/sing-box/releases/download/{tag}/{zip_name}");

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
                return Ok(exe);
            }
        }

        bail!("sing-box.exe not found in release")
    }
}

// ── DNS resolver for STLS server IP (used to build TUN bypass) ────

fn resolve_hostname(host: &str) -> Result<Vec<String>> {
    let addr_str = format!("{host}:0");
    let addrs = addr_str
        .to_socket_addrs()
        .context("DNS resolution failed")?;
    let mut ips: Vec<String> = Vec::new();
    for addr in addrs {
        let ip = addr.ip().to_string();
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    if ips.is_empty() {
        bail!("no IPs resolved for {host}");
    }
    println!("[stls] resolved {host} -> {:?}", ips);
    Ok(ips)
}

// ── tests ───────────────────────────────────────────────────────────

