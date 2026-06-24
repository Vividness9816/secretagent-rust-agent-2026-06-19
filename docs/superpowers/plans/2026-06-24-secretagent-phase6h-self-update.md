# SecretAgent Phase 6h — Self-update (the full pinned-verify contract)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. RCE-grade slice — every step is fail-closed.

**Goal:** `secretagent self-update [--check]` that swaps the running binary for a newer one **only** after a complete, fail-closed verification chain — the contract the ADR demands or the slice is deferred. The operator chose to BUILD it (2026-06-24).

**The contract (ADR-20260623 slice 6h):** download-to-temp → **verify a detached signature against a public key PINNED in the binary** (never fetched) → **no-downgrade** (version read from the *signed* payload) → **verify the downloaded binary's sha256 against the signed manifest** → **atomic rename** → audit event. **Negative-control tests** prove a tampered manifest, a tampered binary, a downgrade, and a wrong-key signature are ALL rejected.

**Crypto (grounded, not hand-rolled):** runtime verify = **`minisign-verify` v0.2** (zero transitive deps → musl-clean by construction; the SAME minisign scheme as the 6b release pipeline, so ONE operator key covers install-verify AND self-update). Test signing = **`minisign` v0.9 as a DEV-dependency** (dev-deps never enter the musl-static binary graph), so the negative-control tests are fully self-contained — no external tooling, no committed fixtures. The signer prehashes; `verify(..., allow_legacy=false)` accepts prehashed → compatible.

**Trust anchor:** the pinned minisign pubkey is a `const` compiled into the binary. It ships **EMPTY** → self-update is **inert (fail-closed) until the operator pins their 6b key** (the safest default for an RCE primitive; the 6b `install.sh`-pubkey precedent). The pure `verify_manifest(bytes, sig, pubkey_b64)` takes the key as a param so tests inject a test key; the CLI path reads the const.

**Outside the egress seam:** the update client is operator-frozen (a `[update] base_url`, not model-reachable) — like the provider/connector clients. It MAY follow redirects (GitHub release → S3) because security comes from the signature+hash, not host-pinning. The model can never invoke `self-update` (a CLI subcommand, never a registry tool).

## Global Constraints
- **Fail-closed at every step**; any verification error aborts BEFORE the swap. Nothing executes the downloaded bytes — they are replaced into place, then the operator restarts.
- **Pinned key never fetched** (`const` in-binary); **version from the signed manifest only** (never from the filename/tag/the untrusted binary's `--version`).
- **musl-static / rustls-only unchanged:** `minisign-verify` is zero-dep pure-Rust; `reqwest` is already in-tree (rustls); `minisign` is DEV-only. **Commit `Cargo.lock`.**
- **TDD**; commit per task; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-6h`; ` # self-audit-ok`; push separately.
- **Gates:** fmt/clippy(all-features) 0; `cargo test --all` BOTH venues; binary purity grep empty; CI green on all 5 jobs. **A dedicated RCE-grade adversarial-review Workflow before push.**

## File Structure
- `secretagent/Cargo.toml` — `minisign-verify` (dep), `reqwest.workspace` (dep, already-in-tree), `minisign` (dev-dep).
- `secretagent/build.rs` — **NEW.** emit `SA_TARGET` from cargo's `TARGET` (the running binary's target triple, for artifact selection). Zero-dep.
- `secretagent/src/self_update.rs` — **NEW.** the pure verify core + the IO flow + tests.
- `crates/sa-core-types/src/config.rs` — `UpdateConfig { base_url: Option<String> }` + `Config.update`.
- `secretagent/src/main.rs` — `Cmd::SelfUpdate { check: bool }` + dispatch + `mod self_update;`.
- `secretagent/src/doctor.rs` — a self-update line (configured? pinned key present?).
- `.github/workflows/release.yml` — emit + minisign-sign `latest.json` (version + per-target {url, sha256}).
- `docs/RELEASE.md` — the operator-gated finish (pin the pubkey const + the manifest-signing step).

---

### Task 1: the pure verify core + negative controls (THE CONTRACT)

**Files:** `secretagent/src/self_update.rs` (pure fns + tests), `secretagent/Cargo.toml` (deps), `secretagent/src/main.rs` (`mod self_update;`).

**Produces:** `Manifest { version, artifacts: BTreeMap<String, Artifact{url, sha256}> }`; `verify_manifest(bytes, minisig, pubkey_b64) -> Result<Manifest>`; `ensure_upgrade(current, candidate) -> Result<()>`; `ensure_sha256(bytes, expected_hex) -> Result<()>`; `select_artifact(&Manifest, target) -> Result<&Artifact>`.

- [ ] **Step 1: Failing tests** (sign with a freshly-generated test `minisign::KeyPair` over a manifest JSON):
  - happy path: a correctly-signed manifest verifies + parses; the right artifact selects for a target.
  - **negative control — tampered manifest:** flip a byte of the signed bytes → `verify_manifest` ERRORS.
  - **negative control — wrong key:** sign with key A, verify against key B's pubkey → ERRORS.
  - **negative control — downgrade:** `ensure_upgrade("0.2.0", "0.1.0")` and `("0.2.0","0.2.0")` ERROR; `("0.1.0","0.2.0")` Ok.
  - **negative control — tampered binary:** `ensure_sha256(bytes, sha256_of_OTHER_bytes)` ERRORS; matching Ok (case-insensitive).
  - missing-target artifact → `select_artifact` ERRORS.
- [ ] **Step 2: FAIL. Step 3: Implement** the pure fns (minisign-verify; `serde_json` parse AFTER verify; a tiny `parse_semver("x.y.z")` stripping any `-pre` suffix, documented; `sha2`). **Step 4: PASS. Step 5: Commit** `feat(6h): self-update verify core — pinned minisign sig + no-downgrade + sha256 (phase 6h)`.

### Task 2: the IO flow + CLI + audit + doctor + build.rs

**Files:** `secretagent/build.rs`, `crates/sa-core-types/src/config.rs`, `secretagent/src/self_update.rs` (flow), `secretagent/src/main.rs`, `secretagent/src/doctor.rs`, `secretagent/Cargo.toml` (reqwest).

**Produces:** `pinned_pubkey_b64() -> Result<&'static str>` (empty const → clear fail-closed error); `atomic_replace(target, temp) -> Result<()>` (unix rename; windows rename-self-aside); `async run(check_only) -> Result<()>`.

- [ ] **Step 1: Failing tests** — `atomic_replace` swaps file contents on real temp files; `pinned_pubkey_b64` errors on the empty const; `parse_semver` edges. (The full network round-trip is operator-gated, like the live Slack/SSH/voice tests.)
- [ ] **Step 2: FAIL. Step 3: Implement** — `build.rs` emits `SA_TARGET`; `[update] base_url`; flow: read base_url (None → "not configured") → frozen reqwest GET `latest.json`+`.minisig` (redirects OK) → `verify_manifest` (pinned key) → `ensure_upgrade(env!("CARGO_PKG_VERSION"), manifest.version)` → `--check` prints + returns → `select_artifact(SA_TARGET)` → GET artifact.url → temp in `current_exe()` dir → `ensure_sha256` → chmod 755 (unix) → `atomic_replace` → `Audit::append_synced("self_update", version)` → "restart to apply". `Cmd::SelfUpdate { check }` + the doctor line. **Step 4: PASS. Step 5: Commit** `feat(6h): self-update flow — temp-download, sha256-verify, atomic self-replace, audit (phase 6h)`.

### Task 3: release.yml signed manifest + docs

- [ ] `release.yml`: after the sha256/minisign steps, emit `latest.json` (`version` from the tag + per-target `{url, sha256}` from the matrix artifacts) and `minisign -S` it → upload `latest.json` + `latest.json.minisig`. `docs/RELEASE.md`: the operator-gated finish — paste the minisign pubkey base64 into the `PINNED_MINISIGN_PUBKEY_B64` const + set `[update] base_url`. **Commit** `feat(6h): release.yml signed latest.json manifest + RELEASE.md self-update finish (phase 6h)`.

### Task 4: review + gate + ship
- [ ] **Dedicated RCE-grade adversarial-review Workflow** (verify-chain bypass, downgrade/rollback, TOCTOU on the swap, key-pinning escape, redirect/SSRF on the frozen client, audit-evasion) → fix findings.
- [ ] Both-venue gate; binary purity empty; push; watch CI; update ledger + memory.

---

## Acceptance (ADR slice 6h)
- A **tampered** update (bad signature OR bad binary hash) is **refused**; a **downgrade** is **refused** — proven by negative-control unit tests. ✓ Task 1.
- A **genuine** update verifies (pinned key), no-downgrades, sha256-matches, and swaps **atomically** + audits. ✓ Task 1 (verify) + Task 2 (atomic swap + audit); the live network swap is operator-gated (key + `[update] base_url` + a cut release — the 6b precedent).
- **Operator-gated finish:** pin the 6b minisign pubkey in the const + set `[update] base_url` + cut a release whose `latest.json` is signed. Until then self-update is **inert/fail-closed** (the safe default).
- **Deferred (honest, → 6i):** delta/partial updates; staged rollout; auto-restart after swap (operator restarts); pre-release version ordering.
