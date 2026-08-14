// config.rs - 8 hardcoded profiles (no management UI)
use anyhow::Result;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_PROFILE: &str = "netherlands-1-stls";
pub const GEOFILES_COOLDOWN_SECS: u64 = 7 * 24 * 3600; // 7 days

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_address: String,
    pub ss_port: u16,
    pub ss_password: String,
    pub stls_port: u16,
    pub stls_password: String,
    pub stls_sni: String,
    pub socks5_port: u16,
    pub mtu: Option<u32>,
    #[serde(default)]
    pub split_mode: String, // "full", "wow"
    #[serde(default)]
    pub split_rules: Vec<SplitRule>,

    pub mode: String,

    // Hysteria2 fields
    pub h2_port: u16,
    pub h2_password: String,
    pub h2_sni: String,
    pub h2_insecure: bool,
    pub h2_obfs: String,
    pub h2_obfs_password: String,
    pub h2_mport: String,
    #[serde(default = "h2_mbps_up_default")]
    pub h2_up_mbps: u32,
    #[serde(default = "h2_mbps_down_default")]
    pub h2_down_mbps: u32,
    #[serde(default = "h2_auto_default")]
    pub h2_auto: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SplitRule {
    pub pattern: String,
    #[serde(default)]
    pub process_names: Vec<String>,
    #[serde(default)]
    pub folder_paths: Vec<String>,
}

fn config_path() -> Result<PathBuf> {
    let proj_dirs = ProjectDirs::from("com", "stls", "stls")
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
    let config_dir = proj_dirs.config_dir();
    fs::create_dir_all(config_dir)?;
    Ok(config_dir.join("config.json"))
}

/// Read the whole config.json as a JSON object (empty object if missing/unreadable)
fn load_config_json() -> serde_json::Value {
    match config_path() {
        Ok(path) if path.exists() => {
            fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        }
        _ => serde_json::json!({}),
    }
}

/// Persist a JSON object to config.json (pretty-printed)
fn save_config_json(value: &serde_json::Value) -> Result<()> {
    let path = config_path()?;
    fs::write(&path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

/// Load active profile name from config.json
pub fn load_profile() -> String {
    load_config_json()["profile"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
}

/// Save profile name to config.json
pub fn save_profile(profile: &str) -> Result<()> {
    let mut existing = load_config_json();
    existing["profile"] = serde_json::Value::String(profile.to_string());
    save_config_json(&existing)
}

/// Save Hysteria2 speed test results to config.json
pub fn save_h2_speeds(up_mbps: u32, down_mbps: u32) -> Result<()> {
    let mut existing = load_config_json();
    existing["h2_up_mbps"] = serde_json::json!(up_mbps);
    existing["h2_down_mbps"] = serde_json::json!(down_mbps);
    save_config_json(&existing)
}

/// Load Hysteria2 speed test results from config.json
pub fn load_h2_speeds() -> (u32, u32) {
    let v = load_config_json();
    let up = v["h2_up_mbps"].as_u64().unwrap_or(h2_mbps_up_default() as u64) as u32;
    let down = v["h2_down_mbps"].as_u64().unwrap_or(h2_mbps_down_default() as u64) as u32;
    (up, down)
}

/// Default Hysteria2 upload bandwidth in Mbps
pub fn h2_mbps_up_default() -> u32 { 40 }

/// Default Hysteria2 download bandwidth in Mbps
pub fn h2_mbps_down_default() -> u32 { 80 }

/// Auto-tune flag default
pub fn h2_auto_default() -> bool { false }

/// Save split tunnel settings to config.json
pub fn save_split_settings(split_mode: &str, split_rules: Vec<SplitRule>) -> Result<()> {
    let mut existing = load_config_json();
    existing["split_mode"] = serde_json::Value::String(split_mode.to_string());
    existing["split_rules"] = serde_json::to_value(split_rules)?;
    save_config_json(&existing)
}

/// Load split tunnel settings from config.json
pub fn load_split_settings() -> (String, Vec<SplitRule>) {
    let v = load_config_json();
    let mode = v["split_mode"].as_str().unwrap_or("full").to_string();
    let rules = v["split_rules"].as_array().map(|arr| {
        arr.iter().filter_map(|r| {
            Some(SplitRule {
                pattern: r["pattern"].as_str()?.to_string(),
                process_names: r["process_names"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
                folder_paths: r["folder_paths"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
            })
        }).collect()
    }).unwrap_or_default();
    (mode, rules)
}

/// Profile descriptor returned by [`parse_profile`]
pub struct ProfileInfo {
    /// Display name, e.g. "Netherlands #1"
    pub server_name: &'static str,
    /// Mode: "hysteria2" or "shadowtls"
    pub protocol: &'static str,
}

/// Parse a profile string like "germany-3-h2" into display name + protocol.
/// Unknown/unparseable profiles map to the Netherlands #1 ShadowTLS default.
pub fn parse_profile(profile: &str) -> ProfileInfo {
    let (server, protocol) = match profile.strip_suffix("-h2") {
        Some(base) => (base, "hysteria2"),
        None => (profile.strip_suffix("-stls").unwrap_or(profile), "shadowtls"),
    };
    match server {
        "netherlands-1" => ProfileInfo { server_name: "Netherlands #1", protocol },
        "germany-1" => ProfileInfo { server_name: "Germany #1", protocol },
        "germany-3" => ProfileInfo { server_name: "Germany #3", protocol },
        "finland-1" => ProfileInfo { server_name: "Finland #1", protocol },
        _ => ProfileInfo { server_name: "Unknown", protocol },
    }
}

/// Get config for a specific profile
pub fn get_profile_config(profile: &str) -> Config {
    let (up, down) = load_h2_speeds();
    let stls_ss_password = "tE+3/qlN/orCZRVUutWouysZ8BQs4RWzq46WK6CDGG4=".to_string();
    let stls_password = "y2lachetore".to_string();
    let stls_sni = "dl.google.com".to_string();
    let h2_password = "testpass1".to_string();
    let h2_obfs_password = "testobfspass".to_string();
    let h2_mport = "40001-45000".to_string();

    match profile {
        "netherlands-1-h2" => Config {
            server_address: "ns.baft.uk".to_string(),
            h2_sni: "ns.baft.uk".to_string(),
            h2_obfs: "salamander".to_string(),
            mode: "hysteria2".to_string(),
            h2_password, h2_obfs_password, h2_mport,
            h2_up_mbps: up, h2_down_mbps: down,
            ..default_config()
        },
        "germany-1-stls" => Config {
            server_address: "ns.baft.uk".to_string(),
            mode: "shadowtls".to_string(),
            ss_password: stls_ss_password, stls_password, stls_sni,
            ..default_config()
        },
        "germany-1-h2" => Config {
            server_address: "ns.baft.uk".to_string(),
            h2_sni: "ns.baft.uk".to_string(),
            h2_obfs: "salamander".to_string(),
            mode: "hysteria2".to_string(),
            h2_password, h2_obfs_password, h2_mport,
            h2_up_mbps: up, h2_down_mbps: down,
            ..default_config()
        },
        "germany-3-stls" => Config {
            server_address: "de3.baft.uk".to_string(),
            mode: "shadowtls".to_string(),
            ss_password: stls_ss_password, stls_password, stls_sni,
            ..default_config()
        },
        "germany-3-h2" => Config {
            server_address: "de3.baft.uk".to_string(),
            h2_sni: "de3.baft.uk".to_string(),
            h2_obfs: "salamander".to_string(),
            mode: "hysteria2".to_string(),
            h2_password, h2_obfs_password, h2_mport,
            h2_up_mbps: up, h2_down_mbps: down,
            ..default_config()
        },
        "finland-1-stls" => Config {
            server_address: "fn.baft.uk".to_string(),
            mode: "shadowtls".to_string(),
            ss_password: stls_ss_password, stls_password, stls_sni,
            ..default_config()
        },
        "finland-1-h2" => Config {
            server_address: "fn.baft.uk".to_string(),
            h2_sni: "fn.baft.uk".to_string(),
            h2_obfs: "salamander".to_string(),
            mode: "hysteria2".to_string(),
            h2_password, h2_obfs_password, h2_mport,
            h2_up_mbps: up, h2_down_mbps: down,
            ..default_config()
        },
        // netherlands-1-stls (default) and any unknown profile
        _ => Config {
            server_address: "ns.baft.uk".to_string(),
            mode: "shadowtls".to_string(),
            ss_password: stls_ss_password, stls_password, stls_sni,
            ..default_config()
        },
    }
}

/// Base profile with common defaults (all 8 profiles share these)
fn default_config() -> Config {
    let (up, down) = load_h2_speeds();
    Config {
        server_address: String::new(),
        ss_port: 8380,
        ss_password: String::new(),
        stls_port: 8553,
        stls_password: String::new(),
        stls_sni: String::new(),
        socks5_port: 1080,
        mtu: None,
        split_mode: "full".to_string(),
        split_rules: vec![],
        mode: "shadowtls".to_string(),
        h2_port: 40001,
        h2_password: String::new(),
        h2_sni: String::new(),
        h2_insecure: false,
        h2_obfs: String::new(),
        h2_obfs_password: String::new(),
        h2_mport: String::new(),
        h2_up_mbps: up,
        h2_down_mbps: down,
        h2_auto: false,
    }
}

/// Get config for active profile, with split settings overlaid
pub fn get_active_config() -> Config {
    let profile = load_profile();
    let mut cfg = get_profile_config(&profile);

    // Split tunnel settings from config.json override profile defaults
    let (split_mode, split_rules) = load_split_settings();
    cfg.split_mode = split_mode;
    cfg.split_rules = split_rules;

    cfg
}

/// Save geofiles update timestamp to config.json
pub fn save_geofiles_timestamp() -> Result<()> {
    let mut existing = load_config_json();
    existing["geofiles_last_update"] = serde_json::Value::String(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string()
    );
    save_config_json(&existing)
}

/// Check if geofiles cooldown period has passed (returns true if can download)
pub fn can_download_geofiles() -> bool {
    let v = load_config_json();
    if let Some(ts) = v["geofiles_last_update"].as_str() {
        if let Ok(last_secs) = ts.parse::<u64>() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            return now.saturating_sub(last_secs) >= GEOFILES_COOLDOWN_SECS;
        }
    }
    true
}
