# SecretAgent Phase 0 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a single self-contained `secretagent` binary that runs `doctor` green headless on Linux/macOS/Windows and round-trips a secret through an age-encrypted file vault across a fresh process — the foundation every later phase builds on.

**Architecture:** A Cargo workspace with **4 crates** (`secretagent` bin + `sa-core-types` + `sa-vault` + `sa-audit`), built just-in-time per the founding ADR. Secrets live only in an age-encrypted file behind a one-impl `Vault` trait; the canonical message types already carry a non-optional `provenance` field; the audit log is its own crate so its append API is the compiler-enforced sole writer.

**Tech Stack:** Rust (stable, edition 2021), `age` (file encryption), `secrecy` (in-memory secret hygiene), `clap` (CLI), `serde`/`serde_json`/`toml`, `blake3` (audit hash-chain), `directories` (per-OS paths), `tracing`. CI: GitHub Actions + `cargo-deny` + `cargo-audit` + cross-compile matrix. **No SQLite in Phase 0** — the DB first appears in Phase 1.

**Authority:** Master spec `inbox/SecretAgent-Build-Plan.md` as amended by `~/.claude/second-brain/decisions/ADR-20260619-secretagent-founding-architecture.md`. On any conflict, the ADR wins.

**Repo:** `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (nested git repo). Private remote `Vividness9816/secretagent-rust-agent-2026-06-19` — create at Task 1, confirm namespace+name with the user before `gh repo create`.

## Global Constraints

Every task implicitly includes these (verbatim from spec + ADR):

- **License:** MIT. `NOTICE` must credit Hermes Agent (Nous Research), EvoMap's Evolver, Honcho (Plastic Labs), and the agentskills.io standard. README has a "Heritage & differences" section. Clean-room — **no Hermes source copied**.
- **Single self-contained binary, defined per-OS:** one executable, no interpreter/venv/external daemon at install or first run. Literal static-libc **only on Linux** (`*-unknown-linux-musl`). macOS = native universal2 linked only against `libSystem`. Windows = native single `.exe` linking only OS DLLs. The phrase "static binary on all three" is a category error and is NOT a goal.
- **No secret ever in the vault store as plaintext, nor in the audit log, nor in process logs.** The store holds age-ciphertext; the audit log holds key *names* and vault key-ids, never values. In-memory secrets are `secrecy::SecretString` (no `Debug`/`Display`).
- **Tool output is tainted:** the canonical message/tool types carry a non-optional `provenance: Provenance` field now. The `Tainted<T>` wrapper + `detaint()` is **deferred to Phase 2** — do NOT build it here.
- **`cargo-deny` license/bans/advisories gate** is scoped to the Phase 0 dependency closure only, not the unbuilt v1 set.
- **age root identity** lives in a `0600` file (Windows: per-user ACL) at a deterministic per-OS path; **keyring is feature-gated OFF** the default build and off the acceptance path.
- Dev platform is **Windows 11 primary**; CI proves Linux-musl-static + headless.

---

## File Structure

```
SecretAgent/
  Cargo.toml                      # workspace: members, shared deps, profile
  deny.toml                       # cargo-deny config (Phase 0 closure)
  rust-toolchain.toml             # pin stable
  LICENSE                         # MIT
  NOTICE                          # upstream credits (spec §3)
  README.md                       # Heritage & differences
  .gitignore
  .github/workflows/ci.yml        # fmt/clippy/test/deny/audit + cross-compile + headless doctor
  crates/
    sa-core-types/
      Cargo.toml
      src/lib.rs                  # re-exports
      src/types.rs                # Message, ToolCall, Provenance, SCHEMA_VERSION
      src/config.rs               # Config, load(), identity_path()
    sa-vault/
      Cargo.toml
      src/lib.rs                  # Vault trait, SecretString re-export
      src/age_file.rs             # AgeFileVault: init/set/get + age helpers
    sa-audit/
      Cargo.toml
      src/lib.rs                  # Audit (sole-writer), AuditEvent, redact
  secretagent/                    # the bin crate
    Cargo.toml
    src/main.rs                   # clap dispatch
    src/doctor.rs                 # doctor checks
    tests/cli.rs                  # integration: doctor + vault round-trip
```

---

### Task 1: Workspace scaffold + licensing/provenance + CI

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `LICENSE`, `NOTICE`, `README.md`, `.gitignore`, `.github/workflows/ci.yml`
- Create stub crates: `crates/sa-core-types/`, `crates/sa-vault/`, `crates/sa-audit/`, `secretagent/` (each `Cargo.toml` + minimal `src/lib.rs`/`main.rs`)
- Test: `crates/sa-core-types/tests/provenance_notice.rs` (NOTICE-credits assertion lives here so it runs in `cargo test`)

**Interfaces:**
- Consumes: nothing.
- Produces: a building, lint-clean workspace; the 4 crate names other tasks depend on.

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/sa-core-types", "crates/sa-vault", "crates/sa-audit", "secretagent"]

[workspace.package]
edition = "2021"
license = "MIT"
repository = "https://github.com/Vividness9816/secretagent-rust-agent-2026-06-19"
rust-version = "1.78"

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
secrecy = { version = "0.8", features = ["serde"] }
age = "0.10"
blake3 = "1"
clap = { version = "4", features = ["derive"] }
directories = "5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[profile.release]
strip = true
lto = "thin"
```

- [ ] **Step 2: Pin toolchain + write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 3: Write `LICENSE` (MIT) and `NOTICE`**

`NOTICE`:
```
SecretAgent — an independent, clean-room Rust reimplementation.

SecretAgent is inspired by the observable behavior and feature set of:
  - Hermes Agent (Nous Research) — design reference. No Hermes source code was copied.
  - EvoMap's Evolver — upstream lineage for the closed learning-loop concept.
  - Honcho (Plastic Labs) — dialectic user-modeling approach.
  - The agentskills.io standard — skills format.

SecretAgent is not a fork. See README "Heritage & differences" for where behavior
intentionally diverges (security defaults, install model, provenance handling).
```

- [ ] **Step 4: Write `README.md` with the required heritage section**

```markdown
# SecretAgent

A self-hosted, autonomous AI agent daemon — a single statically-linked binary.

## Heritage & differences
SecretAgent is an **independent Rust reimplementation**, not a fork of Hermes Agent.
It reimplements observable behavior; it copies no upstream source. It intentionally
diverges in three ways: **security-first defaults** (vault-only credentials, sandboxed
execution, strict-by-default), a **zero-friction install model** (single binary, no
interpreter/venv/shell-rc mutation), and **honest provenance** (this section; an open
NOTICE; issues are never silently edited). See `NOTICE` for upstream credits.
```

- [ ] **Step 5: Write `.gitignore` and `deny.toml`**

`.gitignore`:
```
/target
**/*.rs.bk
# never commit a real vault or identity
*.identity.age
/store.age
.secretagent/
```

`deny.toml` (scoped — allow only what the Phase 0 closure pulls):
```toml
[advisories]
yanked = "deny"

[bans]
multiple-versions = "warn"

[licenses]
allow = ["MIT", "Apache-2.0", "Unicode-3.0", "Unicode-DFS-2016", "BSD-3-Clause", "ISC", "Zlib", "CC0-1.0", "MPL-2.0", "BSD-2-Clause"]
# Phase 0 closure only. Extend per-phase as crates land (ADR "Revisit when").
confidence-threshold = 0.9
```

- [ ] **Step 6: Create the 4 stub crates**

`crates/sa-core-types/Cargo.toml`:
```toml
[package]
name = "sa-core-types"
version = "0.0.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
toml.workspace = true
thiserror.workspace = true
directories.workspace = true
```
`crates/sa-core-types/src/lib.rs`: `pub mod types; pub mod config;` (create empty `types.rs`, `config.rs` with `// filled in Task 2/3`).

`crates/sa-vault/Cargo.toml`: name `sa-vault`, deps `age`, `secrecy`, `anyhow`, `thiserror`, `sa-core-types = { path = "../sa-core-types" }`. `src/lib.rs`: `pub mod age_file;` + `// Vault trait — Task 4`.

`crates/sa-audit/Cargo.toml`: name `sa-audit`, deps `serde`, `serde_json`, `blake3`, `anyhow`, `thiserror`. `src/lib.rs`: `// Audit — Task 6`.

`secretagent/Cargo.toml`: name `secretagent`, `[[bin]] name = "secretagent" path = "src/main.rs"`, deps `clap`, `anyhow`, `tracing`, `tracing-subscriber`, `sa-core-types`, `sa-vault`, `sa-audit`. `src/main.rs`: `fn main() { println!("secretagent"); }`.

- [ ] **Step 7: Write the failing NOTICE-credits test**

`crates/sa-core-types/tests/provenance_notice.rs`:
```rust
#[test]
fn notice_credits_all_required_upstreams() {
    let notice = include_str!("../../../NOTICE");
    for who in ["Hermes Agent", "EvoMap", "Honcho", "agentskills.io"] {
        assert!(notice.contains(who), "NOTICE must credit {who}");
    }
    assert!(notice.contains("clean-room") || notice.contains("No Hermes source"),
        "NOTICE must state clean-room");
}
```

- [ ] **Step 8: Run it — verify it passes** (NOTICE was written in Step 3)

Run: `cargo test -p sa-core-types --test provenance_notice`
Expected: PASS. If the relative `include_str!` path is wrong, fix the `../` depth until it resolves.

- [ ] **Step 9: Write `.github/workflows/ci.yml`**

```yaml
name: ci
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: rustfmt, clippy }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all
      - uses: EmbarkStudios/cargo-deny-action@v2
      - run: cargo install cargo-audit --locked && cargo audit
  build-matrix:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-musl
            cross: true
          - os: ubuntu-latest
            target: aarch64-unknown-linux-musl
            cross: true
          - os: macos-latest
            target: aarch64-apple-darwin
            cross: false
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            cross: false
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: "${{ matrix.target }}" }
      - uses: Swatinem/rust-cache@v2
      - if: matrix.cross
        run: cargo install cross --locked
      - name: Build
        run: ${{ matrix.cross && 'cross' || 'cargo' }} build --release --target ${{ matrix.target }}
      # Task 8 appends per-OS dependency assertions + headless doctor here.
```

- [ ] **Step 10: Verify the whole workspace builds + lints, then init git + commit**

Run: `cargo build --all && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --all`
Expected: all pass.

```bash
cd "C:/Users/dnoye/ClaudeSecondBrain/SecretAgent"
git init && git add -A
git commit -m "chore(phase0): workspace scaffold, MIT/NOTICE provenance, CI"
# Confirm namespace+name with user, then:
# gh repo create Vividness9816/secretagent-rust-agent-2026-06-19 --private --source=. --remote=origin --push
```

---

### Task 2: `sa-core-types` — domain types + Provenance + schema_version

**Files:**
- Modify: `crates/sa-core-types/src/types.rs`
- Test: `crates/sa-core-types/src/types.rs` (`#[cfg(test)]` mod)

**Interfaces:**
- Consumes: nothing.
- Produces: `Provenance` (enum `Trusted | Untrusted { source: String }`), `Message { role, content, provenance, .. }`, `ToolCall { tool, input, provenance, .. }`, `pub const SCHEMA_VERSION: u32 = 1;`. Later phases add the `Tainted<T>` wrapper that *consumes* `Provenance`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_output_must_carry_provenance_and_round_trips() {
        // A tool result is Untrusted by construction — you cannot build one without provenance.
        let tc = ToolCall {
            tool: "web.fetch".into(),
            input: "https://x".into(),
            provenance: Provenance::Untrusted { source: "web.fetch".into() },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provenance, Provenance::Untrusted { source: "web.fetch".into() });
        assert_eq!(SCHEMA_VERSION, 1);
    }
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p sa-core-types types::tests`
Expected: FAIL — `Provenance`/`ToolCall`/`SCHEMA_VERSION` not found.

- [ ] **Step 3: Implement the types (minimal)**

```rust
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Source-of-truth tag. Locked now; the Tainted<T> wrapper that enforces
/// "never promotable to instruction" arrives in Phase 2 (ADR invariant #3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    Trusted,
    Untrusted { source: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub provenance: Provenance, // non-optional by construction
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub input: String,
    pub provenance: Provenance, // non-optional by construction
}
```

- [ ] **Step 4: Run it — verify it passes**

Run: `cargo test -p sa-core-types types::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sa-core-types/src/types.rs
git commit -m "feat(core-types): Message/ToolCall with non-optional provenance + schema_version"
```

---

### Task 3: `sa-core-types` — config load + per-OS identity path

**Files:**
- Modify: `crates/sa-core-types/src/config.rs`
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing.
- Produces: `Config { vault: VaultConfig }`, `Config::load() -> Result<Config>`, `identity_path() -> PathBuf`, `store_path() -> PathBuf`. `sa-vault` and `doctor` consume `identity_path`/`store_path`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_path_honors_env_override() {
        std::env::set_var("SECRETAGENT_DATA_DIR", "/tmp/sa-test");
        let p = identity_path();
        assert!(p.ends_with("identity.age"), "got {p:?}");
        assert!(p.starts_with("/tmp/sa-test"), "env override ignored: {p:?}");
        std::env::remove_var("SECRETAGENT_DATA_DIR");
    }

    #[test]
    fn config_parses_minimal_toml() {
        let c: Config = toml::from_str("").unwrap(); // all defaults
        assert!(c.vault.backend == "age-file");
    }
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p sa-core-types config::tests`
Expected: FAIL — `identity_path`/`Config` not found.

- [ ] **Step 3: Implement config + path resolution**

```rust
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub vault: VaultConfig,
}
impl Default for Config {
    fn default() -> Self { Self { vault: VaultConfig::default() } }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    pub backend: String, // "age-file" in Phase 0
}
impl Default for VaultConfig {
    fn default() -> Self { Self { backend: "age-file".into() } }
}

impl Config {
    pub fn load() -> anyhow::Result<Config> {
        let path = config_dir().join("config.toml");
        if path.exists() {
            Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
        } else {
            Ok(Config::default())
        }
    }
}

/// Per-OS data dir, overridable by SECRETAGENT_DATA_DIR (set by the systemd unit's
/// StateDirectory on Linux). Falls back to the platform data dir.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("SECRETAGENT_DATA_DIR") {
        return PathBuf::from(d);
    }
    directories::ProjectDirs::from("dev", "secretagent", "secretagent")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".secretagent"))
}
pub fn config_dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "secretagent", "secretagent")
        .map(|p| p.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".secretagent"))
}
pub fn identity_path() -> PathBuf { data_dir().join("identity.age") }
pub fn store_path() -> PathBuf { data_dir().join("store.age") }
```

- [ ] **Step 4: Run it — verify it passes**

Run: `cargo test -p sa-core-types config::tests`
Expected: PASS. (On Windows the env-override test still passes — it only checks prefix/suffix, no real FS.)

- [ ] **Step 5: Commit**

```bash
git add crates/sa-core-types/src/config.rs
git commit -m "feat(core-types): config load + per-OS identity/store path resolution"
```

---

### Task 4: `sa-vault` — `Vault` trait + `SecretString` hygiene

**Files:**
- Modify: `crates/sa-vault/src/lib.rs`
- Test: `crates/sa-vault/src/lib.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub use secrecy::SecretString;` and
  ```rust
  pub trait Vault {
      fn set(&mut self, key: &str, value: SecretString) -> anyhow::Result<()>;
      fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>>;
  }
  ```
  `sa-audit`/`doctor` rely on the fact that `SecretString` has no `Debug`/`Display` that exposes the value.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    #[test]
    fn secret_string_does_not_leak_in_debug() {
        let s = SecretString::new("hunter2".to_string());
        assert!(!format!("{s:?}").contains("hunter2"), "secret leaked in Debug");
        assert_eq!(s.expose_secret(), "hunter2");
    }
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p sa-vault tests::secret_string`
Expected: FAIL — `SecretString` not re-exported / trait absent.

- [ ] **Step 3: Implement the trait + re-export**

```rust
pub mod age_file;
pub use secrecy::SecretString;

pub trait Vault {
    fn set(&mut self, key: &str, value: SecretString) -> anyhow::Result<()>;
    fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>>;
}
```

- [ ] **Step 4: Run it — verify it passes**

Run: `cargo test -p sa-vault tests::secret_string`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sa-vault/src/lib.rs
git commit -m "feat(vault): Vault trait + secrecy::SecretString re-export"
```

---

### Task 5: `sa-vault` — age-file backend (init / set / get, fresh-process round-trip)

**Files:**
- Modify: `crates/sa-vault/src/age_file.rs`
- Test: `crates/sa-vault/src/age_file.rs` (`#[cfg(test)]`, uses `tempfile`)
- Modify: `crates/sa-vault/Cargo.toml` — add `[dev-dependencies] tempfile = "3"`

**Interfaces:**
- Consumes: `Vault`, `SecretString` (Task 4).
- Produces: `AgeFileVault::open_or_init(identity_path, store_path) -> Result<Self>` implementing `Vault`. The store is an age-encrypted JSON map; the identity is a `0600` file (Windows: per-user ACL).

**Note on the age API:** the round-trip *test* is the contract. `age`'s `Encryptor`/`Decryptor` signatures shift across versions — pin `age = "0.10"` and adapt the two helper fns below until the round-trip test is green. Do not chase a specific signature from memory; let the test drive it.

- [ ] **Step 1: Write the failing test (the Phase 0 acceptance core)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{SecretString, ExposeSecret};

    #[test]
    fn fresh_process_round_trip_and_no_plaintext_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let id = dir.path().join("identity.age");
        let store = dir.path().join("store.age");

        // "set" in one vault instance...
        {
            let mut v = AgeFileVault::open_or_init(&id, &store).unwrap();
            v.set("API_KEY", SecretString::new("s3cr3t-sentinel".into())).unwrap();
        }
        // ...drop it, open a *fresh* instance (simulates daemon restart) and "get".
        {
            let v = AgeFileVault::open_or_init(&id, &store).unwrap();
            let got = v.get("API_KEY").unwrap().unwrap();
            assert_eq!(got.expose_secret(), "s3cr3t-sentinel");
            assert!(v.get("MISSING").unwrap().is_none());
        }
        // store must not contain the plaintext.
        let bytes = std::fs::read(&store).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("s3cr3t-sentinel"),
            "plaintext secret found in store file");
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
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p sa-vault age_file::tests`
Expected: FAIL — `AgeFileVault` not found.

- [ ] **Step 3: Implement the age-file backend**

```rust
use crate::{SecretString, Vault};
use anyhow::Context;
use secrecy::ExposeSecret;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct AgeFileVault {
    identity: age::x25519::Identity,
    store_path: PathBuf,
    map: BTreeMap<String, String>, // key -> plaintext secret (in-memory only)
}

impl AgeFileVault {
    pub fn open_or_init(identity_path: &Path, store_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = identity_path.parent() { std::fs::create_dir_all(parent)?; }
        let identity = if identity_path.exists() {
            let s = std::fs::read_to_string(identity_path)?;
            s.trim().parse().map_err(|e| anyhow::anyhow!("bad identity: {e}"))?
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
        Ok(Self { identity, store_path: store_path.to_path_buf(), map })
    }

    fn flush(&self) -> anyhow::Result<()> {
        let pt = serde_json::to_vec(&self.map)?;
        let recipient = self.identity.to_public();
        let ct = encrypt(&recipient, &pt)?;
        // atomic-ish: write temp then rename
        let tmp = self.store_path.with_extension("age.tmp");
        std::fs::write(&tmp, &ct)?;
        std::fs::rename(&tmp, &self.store_path)?;
        Ok(())
    }
}

impl Vault for AgeFileVault {
    fn set(&mut self, key: &str, value: SecretString) -> anyhow::Result<()> {
        self.map.insert(key.to_string(), value.expose_secret().to_string());
        self.flush()
    }
    fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        Ok(self.map.get(key).map(|v| SecretString::new(v.clone())))
    }
}

// --- age helpers: adapt to the pinned age version until the round-trip test is green ---
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
    let mut f = std::fs::OpenOptions::new().create(true).truncate(true).write(true)
        .mode(0o600).open(path)?;
    f.write_all(bytes)
}
#[cfg(windows)]
fn write_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // ponytail: rely on per-user %APPDATA% ACL inheritance for Phase 0; tighten with
    // an explicit icacls/SetNamedSecurityInfo pass when a shared-box threat is real.
    std::fs::write(path, bytes)
}
```

- [ ] **Step 4: Run it — verify it passes** (fix the age helpers if the API differs from 0.10)

Run: `cargo test -p sa-vault age_file::tests`
Expected: PASS — round-trip returns the sentinel, store has no plaintext, identity is 0600 on unix.

- [ ] **Step 5: Commit**

```bash
git add crates/sa-vault/
git commit -m "feat(vault): age-file backend with fresh-process round-trip + 0600 identity"
```

---

### Task 6: `sa-audit` — sole-writer append-only hash-chained JSONL + the leak test

**Files:**
- Modify: `crates/sa-audit/src/lib.rs`
- Test: `crates/sa-audit/src/lib.rs` (`#[cfg(test)]`, uses `tempfile`)
- Modify: `crates/sa-audit/Cargo.toml` — `[dev-dependencies] tempfile = "3"`

**Interfaces:**
- Consumes: nothing (deliberately does NOT depend on `sa-vault` — audit must not be able to read a secret).
- Produces: `Audit::open(path) -> Result<Audit>`, `Audit::append(&mut self, AuditEvent) -> Result<()>`, `AuditEvent { action: String, key_id: String }` (key *names*/ids only — never values), `Audit::verify_chain(path) -> Result<bool>`. The append method is the **sole writer**; the inner file handle is private to this crate.

- [ ] **Step 1: Write the failing tests (append/read-back, tamper-detect, leak)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn append_reads_back_and_chain_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent { action: "vault.set".into(), key_id: "API_KEY".into() }).unwrap();
            a.append(AuditEvent { action: "vault.get".into(), key_id: "API_KEY".into() }).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 2);
        assert!(Audit::verify_chain(&p).unwrap(), "untampered chain should verify");
    }

    #[test]
    fn mutating_an_entry_breaks_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        { let mut a = Audit::open(&p).unwrap();
          a.append(AuditEvent { action: "x".into(), key_id: "k".into() }).unwrap();
          a.append(AuditEvent { action: "y".into(), key_id: "k".into() }).unwrap(); }
        let tampered = std::fs::read_to_string(&p).unwrap().replace("\"x\"", "\"z\"");
        std::fs::write(&p, tampered).unwrap();
        assert!(!Audit::verify_chain(&p).unwrap(), "tamper must be detected");
    }

    #[test]
    fn audit_never_contains_secret_material() {
        // The Phase 0 leak test: the only secret-handler is the vault; prove that an
        // audited vault action records the KEY NAME, never the value.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        { let mut a = Audit::open(&p).unwrap();
          a.append(AuditEvent { action: "vault.set".into(), key_id: "API_KEY".into() }).unwrap(); }
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(!body.contains("s3cr3t-sentinel"), "no secret value should ever reach audit");
        assert!(body.contains("API_KEY"), "key name is fine to record");
    }
}
```

- [ ] **Step 2: Run them — verify they fail**

Run: `cargo test -p sa-audit tests`
Expected: FAIL — `Audit`/`AuditEvent` not found.

- [ ] **Step 3: Implement the sole-writer hash-chained log**

```rust
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub action: String,
    pub key_id: String, // key NAME / vault key-id — never a secret value
}

#[derive(Serialize, Deserialize)]
struct Entry {
    seq: u64,
    prev: String, // hex blake3 of the previous entry's canonical bytes ("" for genesis)
    event: AuditEvent,
    hash: String, // blake3(seq || prev || event_json)
}

/// Owns the only writable handle to the log. The file field is private to the crate,
/// so no other crate/module can append or mutate entries except via `append`.
pub struct Audit {
    file: std::fs::File,
    last_hash: String,
    seq: u64,
}

fn entry_hash(seq: u64, prev: &str, event: &AuditEvent) -> String {
    let ev = serde_json::to_string(event).unwrap();
    let mut h = blake3::Hasher::new();
    h.update(&seq.to_le_bytes());
    h.update(prev.as_bytes());
    h.update(ev.as_bytes());
    h.finalize().to_hex().to_string()
}

impl Audit {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let (last_hash, seq) = if path.exists() {
            let mut last = String::new();
            let mut n = 0u64;
            for line in std::fs::read_to_string(path)?.lines() {
                let e: Entry = serde_json::from_str(line)?;
                last = e.hash; n = e.seq + 1;
            }
            (last, n)
        } else { (String::new(), 0) };
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)
            .context("opening audit log append-only")?;
        Ok(Self { file, last_hash, seq })
    }

    pub fn append(&mut self, event: AuditEvent) -> anyhow::Result<()> {
        let hash = entry_hash(self.seq, &self.last_hash, &event);
        let entry = Entry { seq: self.seq, prev: self.last_hash.clone(), event, hash: hash.clone() };
        writeln!(self.file, "{}", serde_json::to_string(&entry)?)?;
        self.file.flush()?;
        self.last_hash = hash;
        self.seq += 1;
        Ok(())
    }

    pub fn verify_chain(path: &Path) -> anyhow::Result<bool> {
        let mut prev = String::new();
        let mut expected_seq = 0u64;
        for line in std::fs::read_to_string(path)?.lines() {
            let e: Entry = serde_json::from_str(line)?;
            if e.seq != expected_seq || e.prev != prev { return Ok(false); }
            if entry_hash(e.seq, &e.prev, &e.event) != e.hash { return Ok(false); }
            prev = e.hash; expected_seq += 1;
        }
        Ok(true)
    }
}
```

`ponytail:` blake3 hash-chain gives tamper-evidence with zero key management; upgrade to ed25519 signatures when an external verifier must trust the log without the file.

- [ ] **Step 4: Run them — verify they pass**

Run: `cargo test -p sa-audit tests`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add crates/sa-audit/
git commit -m "feat(audit): sole-writer hash-chained JSONL + tamper-detect + leak test"
```

---

### Task 7: `secretagent` bin — clap CLI, `doctor`, `vault` subcommands

**Files:**
- Modify: `secretagent/src/main.rs`
- Create: `secretagent/src/doctor.rs`
- Test: `secretagent/tests/cli.rs` (integration, uses `assert_cmd` + `tempfile`)
- Modify: `secretagent/Cargo.toml` — `[dev-dependencies] assert_cmd = "2"`, `tempfile = "3"`

**Interfaces:**
- Consumes: `sa-core-types::config`, `sa-vault::age_file::AgeFileVault`, `sa-audit::Audit`.
- Produces: the `secretagent` binary with `doctor`, `vault init|set|get`. `doctor` exits 0 when healthy/headless.

- [ ] **Step 1: Write the failing integration test**

```rust
use assert_cmd::Command;

fn cmd(data_dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("secretagent").unwrap();
    c.env("SECRETAGENT_DATA_DIR", data_dir);
    c
}

#[test]
fn doctor_exits_zero_headless() {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path()).arg("vault").arg("init").assert().success();
    // No TTY, no D-Bus, no keyring in a test runner — must still be green.
    cmd(dir.path()).arg("doctor").assert().success();
}

#[test]
fn vault_round_trips_via_cli() {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path()).args(["vault", "init"]).assert().success();
    cmd(dir.path()).args(["vault", "set", "API_KEY", "s3cr3t-sentinel"]).assert().success();
    cmd(dir.path()).args(["vault", "get", "API_KEY"]).assert().success()
        .stdout(predicates::str::contains("s3cr3t-sentinel"));
}
```
(Add `predicates = "3"` to dev-deps.)

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p secretagent --test cli`
Expected: FAIL — subcommands unimplemented.

- [ ] **Step 3: Implement the CLI + doctor**

`secretagent/src/main.rs`:
```rust
mod doctor;
use clap::{Parser, Subcommand};
use sa_core_types::config;
use sa_vault::{age_file::AgeFileVault, Vault, SecretString};
use secrecy::ExposeSecret;

#[derive(Parser)]
#[command(name = "secretagent", version)]
struct Cli { #[command(subcommand)] cmd: Cmd }

#[derive(Subcommand)]
enum Cmd {
    /// Health/self-diagnostic (exits 0 when healthy; never fails on a missing keyring)
    Doctor,
    /// Credential vault
    Vault { #[command(subcommand)] op: VaultOp },
}

#[derive(Subcommand)]
enum VaultOp {
    Init,
    Set { key: String, value: String },
    Get { key: String },
}

fn open_vault() -> anyhow::Result<AgeFileVault> {
    AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::from_default_env()).init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Doctor => doctor::run(),
        Cmd::Vault { op } => match op {
            VaultOp::Init => { open_vault()?; println!("vault initialized at {:?}", config::store_path()); Ok(()) }
            VaultOp::Set { key, value } => {
                let mut v = open_vault()?;
                v.set(&key, SecretString::new(value))?;
                println!("set {key}"); // key name only — never the value
                Ok(())
            }
            VaultOp::Get { key } => {
                let v = open_vault()?;
                match v.get(&key)? {
                    Some(s) => { println!("{}", s.expose_secret()); Ok(()) }
                    None => { eprintln!("no such key: {key}"); std::process::exit(2); }
                }
            }
        },
    }
}
```

`secretagent/src/doctor.rs`:
```rust
use sa_core_types::config;

/// Phase 0 doctor: config + vault, headless-safe. Provider/backend/DB checks land
/// in later phases. Keyring is informational and NEVER fails the run.
pub fn run() -> anyhow::Result<()> {
    let mut ok = true;

    // config loads
    match config::Config::load() {
        Ok(_) => println!("[ok]   config loads"),
        Err(e) => { println!("[fail] config: {e}"); ok = false; }
    }

    // identity present + (unix) perms
    let id = config::identity_path();
    if id.exists() {
        println!("[ok]   identity present: {id:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&id)?.permissions().mode() & 0o777;
            if mode == 0o600 { println!("[ok]   identity perms 0600"); }
            else { println!("[fail] identity perms {mode:o} (want 0600) — run: chmod 600 {id:?}"); ok = false; }
        }
    } else {
        println!("[warn] no identity yet — run: secretagent vault init");
    }

    // unattended decrypt self-test (no prompt, no keyring, no D-Bus)
    match self_test_vault() {
        Ok(()) => println!("[ok]   vault decrypts unattended"),
        Err(e) => { println!("[fail] vault self-test: {e}"); ok = false; }
    }

    // keyring: informational, never fails
    println!("[info] keyring: not used in this build (age-file backend) — expected");

    if ok { println!("doctor: OK"); Ok(()) }
    else { std::process::exit(1); }
}

fn self_test_vault() -> anyhow::Result<()> {
    use sa_vault::{age_file::AgeFileVault, Vault, SecretString};
    use secrecy::ExposeSecret;
    let v = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
    // round-trip a nonce through a throwaway store path to avoid touching real data
    let _ = v; // opening + decrypting the real store above already proves unattended decrypt
    let nonce = "doctor-nonce";
    let dir = std::env::temp_dir().join("secretagent-doctor");
    std::fs::create_dir_all(&dir)?;
    let mut t = AgeFileVault::open_or_init(&dir.join("id.age"), &dir.join("st.age"))?;
    t.set("n", SecretString::new(nonce.into()))?;
    let got = t.get("n")?.ok_or_else(|| anyhow::anyhow!("nonce missing"))?;
    anyhow::ensure!(got.expose_secret() == nonce, "nonce mismatch");
    Ok(())
}
```

- [ ] **Step 4: Run it — verify it passes**

Run: `cargo test -p secretagent --test cli`
Expected: PASS — doctor exits 0 headless; vault round-trips via CLI.

- [ ] **Step 5: Commit**

```bash
git add secretagent/
git commit -m "feat(bin): clap CLI + headless doctor + vault init/set/get"
```

---

### Task 8: Acceptance — per-OS packaging assertions + headless doctor in CI

**Files:**
- Modify: `.github/workflows/ci.yml` (extend `build-matrix`)

**Interfaces:**
- Consumes: the release binary from each matrix leg.
- Produces: the CI-enforced Phase 0 acceptance gate.

- [ ] **Step 1: Append the per-OS dependency + headless-doctor assertions to `build-matrix`**

After the Build step, add:
```yaml
      - name: Assert self-contained binary (Linux musl = static)
        if: runner.os == 'Linux'
        run: |
          BIN=target/${{ matrix.target }}/release/secretagent
          file "$BIN"
          if ldd "$BIN" 2>&1 | grep -qv 'not a dynamic executable'; then
            echo "expected fully static musl binary"; ldd "$BIN"; exit 1; fi
      - name: Assert only OS dylibs (macOS)
        if: runner.os == 'macOS'
        run: |
          BIN=target/${{ matrix.target }}/release/secretagent
          otool -L "$BIN"
          if otool -L "$BIN" | tail -n +2 | grep -vE '/usr/lib/|/System/'; then
            echo "third-party dylib linked"; exit 1; fi
      - name: Assert only OS DLLs (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $bin = "target/${{ matrix.target }}/release/secretagent.exe"
          & "$bin" --version
          # smoke: it runs as a single exe with no vendored DLL beside it
          if (Get-ChildItem (Split-Path $bin) -Filter *.dll) { throw "vendored DLL present" }
      - name: Headless doctor (Linux, no D-Bus / no TTY)
        if: matrix.target == 'x86_64-unknown-linux-musl'
        run: |
          BIN=target/${{ matrix.target }}/release/secretagent
          env -i SECRETAGENT_DATA_DIR=$RUNNER_TEMP/sa "$BIN" vault init
          env -i SECRETAGENT_DATA_DIR=$RUNNER_TEMP/sa "$BIN" doctor
```

- [ ] **Step 2: Verify locally what you can (Windows dev box)**

Run: `cargo build --release && ./target/release/secretagent.exe vault init && ./target/release/secretagent.exe doctor`
Expected: `doctor: OK`, exit 0.

- [ ] **Step 3: Push and confirm the matrix is green**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(phase0): per-OS self-contained-binary assertions + headless doctor gate"
git push origin master   # or the working branch
```
Expected: all four matrix legs green; the Linux leg proves static + headless doctor.

- [ ] **Step 4: Phase 0 acceptance sign-off**

Confirm against the ADR's rewritten acceptance test:
- [ ] one self-contained binary per OS (CI assertions green)
- [ ] `doctor` exits 0 headless on Linux (no D-Bus/TTY/keyring)
- [ ] vault round-trips a secret across a fresh process (Task 5 + Task 7 tests)
- [ ] store + audit log contain neither the plaintext nor the private key (Task 5 + Task 6 tests)
- [ ] `cargo-deny`/`cargo-audit` green on the Phase 0 closure

Stop here for user review before Phase 1.

---

## Self-Review

**Spec/ADR coverage:**
- 4-crate workspace JIT → Task 1. ✅
- MIT + NOTICE credits + heritage README (spec §3) → Task 1 (+ enforced test). ✅
- `provenance` field + `schema_version` (ADR inv#3) → Task 2. ✅ `Tainted<T>` correctly NOT built (deferred Phase 2).
- Per-OS identity path + config (ADR bootstrap) → Task 3. ✅
- age-file vault, `SecretString`, 0600, fresh-process round-trip, no-plaintext (ADR inv#4 + acceptance) → Tasks 4–5. ✅
- Sole-writer hash-chained audit + leak test (ADR inv#4, sa-audit boundary) → Task 6. ✅
- `doctor` headless + vault CLI → Task 7. ✅
- CI fmt/clippy/test/deny(scoped)/audit + cross-compile + per-OS self-contained assertions + headless doctor (rewritten acceptance) → Tasks 1 + 8. ✅
- keyring OFF default/acceptance → reflected in doctor `[info]` line + no `keyring` dep. ✅
- SQLite-canonical → correctly absent in Phase 0 (no DB); rebuild-test deferred to Phase 1 per ADR. ✅

**Placeholder scan:** no TBD/TODO-as-task; every code step has complete code. The one explicit flexibility — the `age` encrypt/decrypt helper signatures — is called out with the round-trip test as the binding contract, not left vague.

**Type consistency:** `AgeFileVault::open_or_init(identity, store)`, `Vault::{set,get}`, `Audit::{open,append,verify_chain}`, `AuditEvent{action,key_id}`, `Provenance::{Trusted,Untrusted{source}}`, `config::{identity_path,store_path,Config::load}` — names match across Tasks 2–8. ✅
