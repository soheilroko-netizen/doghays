// geofiles.rs - Download and manage sing-box geoip/geosite databases
use anyhow::Result;
use std::fs;
use std::path::PathBuf;

/// Get geo directory path (%LOCALAPPDATA%\stls\geo\)
pub fn geo_dir() -> Result<PathBuf> {
    let local_data = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| anyhow::anyhow!("Could not determine local data directory"))?;
    let geo_path = PathBuf::from(local_data).join("stls").join("geo");
    fs::create_dir_all(&geo_path)?;
    Ok(geo_path)
}

/// Download geoip.db and geosite.db from SagerNet releases
pub fn download_geofiles() -> Result<()> {
    let geo_path = geo_dir()?;
    let geoip_url = "https://github.com/SagerNet/sing-geoip/releases/latest/download/geoip.db";
    let geosite_url = "https://github.com/SagerNet/sing-geosite/releases/latest/download/geosite.db";

    // Download geoip.db
    let geoip_path = geo_path.join("geoip.db");
    let geoip_bytes = reqwest::blocking::get(geoip_url)?.bytes()?;
    fs::write(&geoip_path, geoip_bytes)?;

    // Download geosite.db
    let geosite_path = geo_path.join("geosite.db");
    let geosite_bytes = reqwest::blocking::get(geosite_url)?.bytes()?;
    fs::write(&geosite_path, geosite_bytes)?;

    // Record timestamp for 1-week cooldown
    let _ = crate::config::save_geofiles_timestamp();

    Ok(())
}

/// Check if geofiles exist
pub fn geofiles_exist() -> bool {
    let Ok(geo_path) = geo_dir() else { return false };
    geo_path.join("geoip.db").exists() && geo_path.join("geosite.db").exists()
}
