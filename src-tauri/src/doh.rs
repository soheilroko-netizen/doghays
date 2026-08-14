// doh.rs - Private DoH DNS toggle (independent of VPN connections)
// Mirrors set_doh_for_wow.bat: registers DoH templates + sets static DNS on
// the active adapter, or resets to DHCP + flushes when toggled off.
use anyhow::{bail, Result};
#[cfg(target_os = "windows")]
use anyhow::Context;

/// Primary DoH server (from set_doh_for_wow.bat)
const PRIMARY_IP: &str = "62.238.60.136";
const PRIMARY_DOH: &str = "https://fn.baft.uk/dns-query";
/// Secondary DoH server (from set_doh_for_wow.bat)
const SECONDARY_IP: &str = "5.255.116.43";
const SECONDARY_DOH: &str = "https://ns.reddeernook123.org/dns-query";

/// Run a netsh/ipconfig command without showing a console window.
fn run_hidden(args: &[&str]) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let out = std::process::Command::new(args[0])
            .args(&args[1..])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .with_context(|| format!("failed to run {}", args[0]))?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = args;
        bail!("DoH DNS only supported on Windows")
    }
}

/// Detect the active network adapter name ("Enabled Connected" line).
fn active_adapter() -> Result<String> {
    let out = run_hidden(&["netsh", "interface", "show", "interface"])?;
    for line in out.lines().skip(3) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 && parts[0].eq_ignore_ascii_case("Enabled") && parts[1].eq_ignore_ascii_case("Connected") {
            let name = parts[3..].join(" ");
            return Ok(name);
        }
    }
    bail!("No active network adapter found")
}

/// Register a DoH template for a server IP.
fn register_doh(server_ip: &str, template: &str) -> Result<()> {
    run_hidden(&[
        "netsh", "dns", "add", "encryption",
        "server=", server_ip,
        "dohtemplate=", template,
        "autoupgrade=yes",
        "udpfallback=no",
    ])?;
    Ok(())
}

/// Remove a DoH template registration for a server IP.
fn unregister_doh(server_ip: &str) -> Result<()> {
    run_hidden(&["netsh", "dns", "delete", "encryption", "server=", server_ip])?;
    Ok(())
}

/// Set DoH DNS on the active adapter (on).
pub fn set_doh_dns() -> Result<String> {
    let iface = active_adapter()?;
    register_doh(PRIMARY_IP, PRIMARY_DOH)?;
    register_doh(SECONDARY_IP, SECONDARY_DOH)?;

    run_hidden(&["netsh", "interface", "ipv4", "set", "dnsservers", "name=", &iface, "static", PRIMARY_IP, "primary"])?;
    run_hidden(&["netsh", "interface", "ipv4", "add", "dnsservers", "name=", &iface, SECONDARY_IP, "index=2"])?;
    run_hidden(&["ipconfig", "/flushdns"])?;
    Ok(iface)
}

/// Reset DNS to DHCP and flush (off).
pub fn clear_doh_dns() -> Result<String> {
    let iface = active_adapter()?;
    run_hidden(&["netsh", "interface", "ipv4", "set", "dnsservers", "name=", &iface, "dhcp"])?;
    // Clean up DoH template registrations (unused once DNS is DHCP)
    let _ = unregister_doh(PRIMARY_IP);
    let _ = unregister_doh(SECONDARY_IP);
    run_hidden(&["ipconfig", "/flushdns"])?;
    Ok(iface)
}

/// Whether the active adapter currently uses static (DoH) DNS.
pub fn doh_active() -> Result<bool> {
    let iface = active_adapter()?;
    let out = run_hidden(&["netsh", "interface", "ipv4", "show", "dnsservers", "name=", &iface])?;
    // Static config shows "Configured DNS servers" with a listed address;
    // DHCP shows "DHCP-configured DNS servers".
    Ok(out.contains("Configured DNS servers"))
}
