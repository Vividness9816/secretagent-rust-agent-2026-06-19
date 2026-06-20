pub mod age_file;

pub use secrecy::SecretString;

/// A credential store. Phase 0 ships exactly one implementation (`age_file`);
/// keyring/TPM backends are added later behind this same trait (ADR: keyring
/// deferred, off the default build and off the acceptance path).
pub trait Vault {
    fn set(&mut self, key: &str, value: SecretString) -> anyhow::Result<()>;
    fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn secret_string_does_not_leak_in_debug() {
        let s = SecretString::new("hunter2".to_string());
        assert!(
            !format!("{s:?}").contains("hunter2"),
            "secret leaked in Debug output"
        );
        assert_eq!(s.expose_secret(), "hunter2");
    }
}
