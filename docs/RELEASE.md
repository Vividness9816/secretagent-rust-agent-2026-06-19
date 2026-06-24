# Releasing SecretAgent (Phase 6b)

Per **ADR-20260623-secretagent-phase6-milestone §6b**. A release is a signed, checksummed set of
self-contained binaries + a distroless container, installed by a verify-before-place script. Cutting
a tagged release is the **operator's** step; the pipeline (`.github/workflows/release.yml`) does the
rest.

## The honest trust story (read this)

This is a **single-maintainer, self-signed** project. The signatures prove **authenticity from one
key**, not a CA chain:

- **minisign** (cross-platform): a detached signature over `SHA256SUMS` proves every artifact came
  from the holder of the project's minisign key. Trust = you pinned the right public key.
- **Windows Authenticode** (Dylan-N cert, self-signed): turns "Unknown publisher" into "Verified
  publisher: Dylan N" **only on machines that have imported the cert** (`~/.codesign/trust-codesign.ps1`).
  Elsewhere it's still self-signed.
- **macOS notarization: DEFERRED** — no Apple Developer account. macOS users see a Gatekeeper warning;
  the binary is checksummed + minisign-signed like the others. (Revisit when an Apple account exists.)

Nothing here claims a public CA / notary chain. That's the point of stating it plainly (Pillar C).

## One-time setup (before the first release)

1. **Generate the minisign keypair (password-less, CI-usable):**
   ```sh
   minisign -G -W -p secretagent.pub -s secretagent.key
   ```
   - `secretagent.key` → add as the GitHub Actions secret **`MINISIGN_SECRET_KEY`** (paste the whole
     file). Keep the file offline; never commit it.
   - `secretagent.pub` → it's one line (`RWQ…`). **Pin it** in two places: `install.sh`
     (`MINISIGN_PUBKEY=`) and publish it in the repo/README so installers can verify. (The current
     value in `install.sh` is a `TODO` placeholder — replace it.)
2. **(Optional) Windows Authenticode in CI:** export the Dylan-N PFX and add it as secrets
   **`WINDOWS_PFX_BASE64`** (base64 of the `.pfx`) + **`WINDOWS_PFX_PASSWORD`**. If absent, the
   Windows artifact ships unsigned from CI — sign it locally instead:
   `& "$env:USERPROFILE\.codesign\sign-codesign.ps1" -Target secretagent-…-msvc.exe`.
3. **GHCR**: no setup — the container job uses the built-in `GITHUB_TOKEN` to push to
   `ghcr.io/vividness9816/secretagent`.

## Cutting a release

```sh
# bump the version in secretagent/Cargo.toml (it's 0.0.0 today), commit, then:
git tag v0.1.0 && git push origin v0.1.0
```
`release.yml` then: builds the 4-target matrix → `SHA256SUMS` → `minisign -S` (signs the manifest) →
optional Authenticode → a GitHub release with every asset + `SHA256SUMS` + `SHA256SUMS.minisig` →
a multi-arch (`amd64`+`arm64`) distroless image pushed to GHCR. `ci.yml` runs on the same commit, so
fmt/clippy/test + the self-contained-binary asserts gate the tag too.

## Installing (what users run)

- **Linux/macOS:** `curl -fsSL https://github.com/<repo>/releases/latest/download/install.sh | sh`
  — requires `minisign`; verifies the signature **and** the checksum **before** placing the binary,
  fails closed, and only **prints** PATH guidance (never edits your shell rc). Logic smoke-tested by
  `scripts/test-install-verify.sh`.
- **Windows:** `irm https://github.com/<repo>/releases/latest/download/install.ps1 | iex` — verifies
  the sha256 + the Authenticode signature before placing.
- **Container:** `docker compose up -d` (see `compose.yaml`) — distroless, **non-root**, read-only
  rootfs, `cap_drop: ALL`, `no-new-privileges`; only the `sa-data` volume is writable.
- **Verify any time:** `secretagent doctor` prints the running binary's SHA-256 — compare it to the
  published `<target>.sha256` line in `SHA256SUMS`.

## Deferred (with triggers)
- **macOS notarization** — when an Apple Developer account exists / a real macOS user hits Gatekeeper.
- **A convenience `install` that also configures a service** — `service install` already does that
  separately (Phase 4b); the installer deliberately only fetches+verifies+places (no rc/service edits).
