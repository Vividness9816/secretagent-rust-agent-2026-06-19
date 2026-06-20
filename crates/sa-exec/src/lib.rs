//! The tool-execution sandbox (ADR-20260620, Tier D: landlock-only). A single
//! `Sandbox` seam with two impls — `LandlockSandbox` (Linux kernel tier) and
//! `RefuseSandbox` (all platforms, fail-closed default). The injection guard is NOT
//! here: it lives in sa-core-types/sa-core. This crate is sandbox-only.
use anyhow::Result;
use sa_core_types::policy::Policy;

#[derive(Debug, Clone)]
pub enum LandlockStatus {
    /// Landlock is present AND a ruleset enforces; `abi` is the kernel ABI level.
    Enforced { abi: i32 },
    /// No enforced sandbox (wrong OS, old kernel, disabled). `execute_code` fail-closes.
    Unavailable { reason: String },
}

/// The one containment seam. `run_confined` runs `code` (a `sh -c` snippet) restricted
/// to the policy's file roots; it MUST return Err rather than run unconfined.
pub trait Sandbox: Send + Sync {
    fn run_confined(&self, code: &str, policy: &Policy) -> Result<String>;
    fn status(&self) -> LandlockStatus;
}

/// Refuses everything. The all-platforms fallback when no kernel sandbox exists.
pub struct RefuseSandbox;

impl Sandbox for RefuseSandbox {
    fn run_confined(&self, _code: &str, _policy: &Policy) -> Result<String> {
        anyhow::bail!("execute_code refused: no enforced sandbox on this platform")
    }
    fn status(&self) -> LandlockStatus {
        LandlockStatus::Unavailable {
            reason: "no sandbox backend on this platform".into(),
        }
    }
}

#[cfg(target_os = "linux")]
mod landlock_linux;
#[cfg(target_os = "linux")]
pub use landlock_linux::LandlockSandbox;

/// The platform's best sandbox: Landlock on Linux, Refuse everywhere else.
pub fn default_sandbox() -> Box<dyn Sandbox> {
    #[cfg(target_os = "linux")]
    {
        Box::new(LandlockSandbox::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(RefuseSandbox)
    }
}

/// The capability the `doctor` line reports + the dispatch gate consults.
pub fn landlock_status() -> LandlockStatus {
    default_sandbox().status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sa_core_types::policy::Policy;

    #[test]
    fn refuse_sandbox_always_denies_and_reports_unavailable() {
        let sb = RefuseSandbox;
        let err = sb.run_confined("echo hi", &Policy::default()).unwrap_err();
        assert!(err.to_string().contains("refused"), "got: {err}");
        assert!(matches!(sb.status(), LandlockStatus::Unavailable { .. }));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn default_sandbox_refuses_on_non_linux() {
        // On a box with no landlock, execute_code MUST be fail-closed by default.
        let err = default_sandbox()
            .run_confined("echo hi", &Policy::default())
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "got: {err}");
        assert!(matches!(
            landlock_status(),
            LandlockStatus::Unavailable { .. }
        ));
    }
}
