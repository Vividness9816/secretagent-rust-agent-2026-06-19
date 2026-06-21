//! OS service install (Phase 4b). `install()/uninstall()/status()` dispatch by `cfg` to the
//! systemd (Linux) backend; the Windows SCM backend lands in Task 2; macOS is a compile-only
//! stub (launchd deferred, ADR-20260621). The unit-text generator is pure + CI-tested on every OS.

use std::path::Path;

pub const SERVICE_NAME: &str = "secretagent";
/// Used only by the Windows SCM backend (the systemd unit hardcodes its Description).
#[cfg(windows)]
pub const SERVICE_DISPLAY: &str = "SecretAgent Gateway";

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
pub mod windows;

/// The systemd unit text. PURE — no IO; tested on every OS. Runs `<exe> gateway` as a system
/// service, wires the existing SECRETAGENT_DATA_DIR seam to StateDirectory, restarts on failure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // consumed by linux.rs + the unit test
pub fn systemd_unit_text(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=SecretAgent autonomous agent gateway\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} gateway\n\
         StateDirectory=secretagent\n\
         Environment=SECRETAGENT_DATA_DIR=/var/lib/secretagent\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        exe = exe.display()
    )
}

/// Install the service so it starts on boot. Requires privilege (root / admin).
pub fn install() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::install()
    }
    #[cfg(windows)]
    {
        windows::install()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        anyhow::bail!("service install is not supported on this OS yet (launchd deferred)")
    }
}

pub fn uninstall() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::uninstall()
    }
    #[cfg(windows)]
    {
        windows::uninstall()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        anyhow::bail!("service uninstall is not supported on this OS yet")
    }
}

/// A never-failing health string for `doctor` (and `service status`).
pub fn status() -> String {
    #[cfg(target_os = "linux")]
    {
        linux::status()
    }
    #[cfg(windows)]
    {
        windows::status()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        "unsupported on this OS yet".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn systemd_unit_has_the_load_bearing_directives() {
        let unit = systemd_unit_text(Path::new("/usr/local/bin/secretagent"));
        assert!(unit.contains("ExecStart=/usr/local/bin/secretagent gateway"));
        assert!(unit.contains("StateDirectory=secretagent"));
        assert!(unit.contains("Environment=SECRETAGENT_DATA_DIR=/var/lib/secretagent"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert!(
            unit.contains("[Unit]") && unit.contains("[Service]") && unit.contains("[Install]")
        );
    }

    #[test]
    fn service_name_is_stable() {
        // The install binPath args + the (Windows) service-run dispatcher must agree on this.
        assert_eq!(SERVICE_NAME, "secretagent");
    }
}
