//! systemd backend (Linux). Writes a single unit to /etc/systemd/system and enables it.
//! Writes ONLY the unit (no shell-rc mutation). Requires root.

use super::{systemd_unit_text, SERVICE_NAME};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

fn unit_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system").join(format!("{SERVICE_NAME}.service"))
}

fn require_root() -> Result<()> {
    // EUID 0 check without a libc dep: shell `id -u`.
    let out = Command::new("id")
        .arg("-u")
        .output()
        .context("running `id -u`")?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if uid != "0" {
        bail!(
            "`service install` must run as root (writes {}). Re-run with sudo.",
            unit_path().display()
        );
    }
    Ok(())
}

pub fn install() -> Result<()> {
    require_root()?;
    let exe = std::env::current_exe().context("resolving the running binary path")?;
    let unit = systemd_unit_text(&exe);
    std::fs::write(unit_path(), unit)
        .with_context(|| format!("writing {}", unit_path().display()))?;
    run("systemctl", &["daemon-reload"])?;
    run("systemctl", &["enable", "--now", SERVICE_NAME])?;
    println!("installed + enabled {SERVICE_NAME} (systemd). It will start on boot.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    require_root()?;
    // Best-effort stop/disable; then remove the unit.
    let _ = run("systemctl", &["disable", "--now", SERVICE_NAME]);
    let _ = std::fs::remove_file(unit_path());
    let _ = run("systemctl", &["daemon-reload"]);
    println!("uninstalled {SERVICE_NAME} (systemd).");
    Ok(())
}

/// Never fails — reports the unit's enable state for doctor.
pub fn status() -> String {
    match Command::new("systemctl")
        .args(["is-enabled", SERVICE_NAME])
        .output()
    {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "not installed".to_string()
            } else {
                s
            }
        }
        Err(_) => "systemctl unavailable".to_string(),
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("running {cmd} {args:?}"))?;
    if !status.success() {
        bail!("{cmd} {args:?} failed with {status}");
    }
    Ok(())
}
