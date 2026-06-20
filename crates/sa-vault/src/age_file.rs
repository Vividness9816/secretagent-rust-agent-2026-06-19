use crate::{SecretString, Vault};
use anyhow::Context;
use secrecy::ExposeSecret;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// An age-encrypted file vault. The identity (X25519 private key) lives in a
/// `0600` file; the store is an age-encrypted JSON map of key -> secret. Secrets
/// exist in plaintext only in memory (this struct's `map`), never on disk.
pub struct AgeFileVault {
    identity: age::x25519::Identity,
    store_path: PathBuf,
    map: BTreeMap<String, String>,
}

impl AgeFileVault {
    pub fn open_or_init(identity_path: &Path, store_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = identity_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let identity = if identity_path.exists() {
            let s = std::fs::read_to_string(identity_path)?;
            s.trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("bad identity file: {e}"))?
        } else {
            let id = age::x25519::Identity::generate();
            write_0600(identity_path, id.to_string().expose_secret().as_bytes())
                .context("writing identity")?;
            id
        };
        let map = if store_path.exists() {
            let ct = std::fs::read(store_path)?;
            let pt = decrypt(&identity, &ct)?;
            serde_json::from_slice(&pt).unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            identity,
            store_path: store_path.to_path_buf(),
            map,
        })
    }

    fn flush(&self) -> anyhow::Result<()> {
        let pt = serde_json::to_vec(&self.map)?;
        let recipient = self.identity.to_public();
        let ct = encrypt(&recipient, &pt)?;
        let tmp = self.store_path.with_extension("age.tmp");
        std::fs::write(&tmp, &ct)?;
        std::fs::rename(&tmp, &self.store_path)?;
        Ok(())
    }
}

impl Vault for AgeFileVault {
    fn set(&mut self, key: &str, value: SecretString) -> anyhow::Result<()> {
        self.map
            .insert(key.to_string(), value.expose_secret().to_string());
        self.flush()
    }

    fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        Ok(self.map.get(key).map(|v| SecretString::new(v.clone())))
    }
}

fn encrypt(recipient: &age::x25519::Recipient, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let enc = age::Encryptor::with_recipients(vec![Box::new(recipient.clone())])
        .ok_or_else(|| anyhow::anyhow!("no recipients"))?;
    let mut out = Vec::new();
    let mut w = enc.wrap_output(&mut out)?;
    w.write_all(plaintext)?;
    w.finish()?;
    Ok(out)
}

fn decrypt(identity: &age::x25519::Identity, ct: &[u8]) -> anyhow::Result<Vec<u8>> {
    let dec = match age::Decryptor::new(ct)? {
        age::Decryptor::Recipients(d) => d,
        _ => anyhow::bail!("expected recipients-encrypted store"),
    };
    let mut out = Vec::new();
    let mut r = dec.decrypt(std::iter::once(identity as &dyn age::Identity))?;
    r.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(unix)]
fn write_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(windows)]
fn write_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // ponytail: rely on per-user %APPDATA% ACL inheritance for Phase 0; tighten with
    // an explicit icacls/SetNamedSecurityInfo pass when a shared-box threat is real.
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{ExposeSecret, SecretString};

    #[test]
    fn fresh_process_round_trip_and_no_plaintext_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let id = dir.path().join("identity.age");
        let store = dir.path().join("store.age");

        {
            let mut v = AgeFileVault::open_or_init(&id, &store).unwrap();
            v.set("API_KEY", SecretString::new("s3cr3t-sentinel".into()))
                .unwrap();
        }
        {
            let v = AgeFileVault::open_or_init(&id, &store).unwrap();
            let got = v.get("API_KEY").unwrap().unwrap();
            assert_eq!(got.expose_secret(), "s3cr3t-sentinel");
            assert!(v.get("MISSING").unwrap().is_none());
        }
        let bytes = std::fs::read(&store).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("s3cr3t-sentinel"),
            "plaintext secret found in store file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let id = dir.path().join("identity.age");
        let store = dir.path().join("store.age");
        AgeFileVault::open_or_init(&id, &store).unwrap();
        let mode = std::fs::metadata(&id).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "identity must be 0600, got {:o}", mode & 0o777);
    }
}
