#!/bin/sh
# SecretAgent installer (Phase 6b) — fetch, VERIFY (minisign signature + sha256), THEN place.
# Verifies BEFORE placing and fails closed. PRINTS PATH guidance; never edits your shell rc.
# No interpreter, no venv, no shell-rc mutation. Usage: curl -fsSL <url>/install.sh | sh
set -eu

REPO="Vividness9816/secretagent-rust-agent-2026-06-19"
# Pinned minisign PUBLIC key — the release's trust anchor (authenticity from one maintainer key,
# NOT a CA chain). Generate once: `minisign -G -p secretagent.pub -s secretagent.key`; publish the
# .pub and paste its single key line here; keep the .key secret (CI's MINISIGN_SECRET_KEY).
MINISIGN_PUBKEY="RWQREPLACE_WITH_REAL_MINISIGN_PUBLIC_KEY_BEFORE_FIRST_RELEASE0000000000"  # TODO
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- target detection ---
os="$(uname -s)"; arch="$(uname -m)"
case "$os" in
  Linux)
    case "$arch" in
      x86_64) tgt=x86_64-unknown-linux-musl ;;
      aarch64|arm64) tgt=aarch64-unknown-linux-musl ;;
      *) die "unsupported arch: $arch" ;;
    esac ;;
  Darwin) tgt=aarch64-apple-darwin ;;  # arm64; Intel macs run it via Rosetta or build from source
  *) die "unsupported OS: $os (on Windows use install.ps1)" ;;
esac
asset="secretagent-$tgt"

# --- tooling (fail closed: no signature tool = no install) ---
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v minisign >/dev/null 2>&1 || die "minisign is required (the signature MUST be verified): install via brew/apt/apk minisign"
if command -v sha256sum >/dev/null 2>&1; then SHA="sha256sum"
elif command -v shasum >/dev/null 2>&1; then SHA="shasum -a 256"
else die "no sha256 tool (sha256sum/shasum) found"; fi

base="https://github.com/$REPO/releases/latest/download"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

say "Downloading $asset + signed checksums…"
curl -fsSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
curl -fsSL "$base/SHA256SUMS.minisig" -o "$tmp/SHA256SUMS.minisig"

# 1) Verify the minisign signature over the checksums file (one sig covers every artifact).
say "Verifying minisign signature…"
minisign -Vm "$tmp/SHA256SUMS" -P "$MINISIGN_PUBKEY" \
  || die "SIGNATURE VERIFICATION FAILED — refusing to install"

# 2) Verify the downloaded binary matches its signed checksum. Compare hashes directly so the
#    separator (GNU "hash  file" vs binary "hash *file") never matters.
say "Verifying $asset checksum…"
want="$(awk -v a="$asset" '{f=$2; sub(/^\*/,"",f); if (f==a) print $1}' "$tmp/SHA256SUMS" | head -1)"
[ -n "$want" ] || die "no checksum line for $asset in SHA256SUMS"
got="$($SHA "$tmp/$asset" | awk '{print $1}')"
[ "$want" = "$got" ] || die "CHECKSUM MISMATCH for $asset — refusing to install"

# 3) Verified — NOW place it (verify-before-place).
mkdir -p "$INSTALL_DIR"
chmod +x "$tmp/$asset"
mv "$tmp/$asset" "$INSTALL_DIR/secretagent"
say "Installed: $INSTALL_DIR/secretagent"

# 4) PRINT PATH guidance — never edit the shell rc for you.
case ":${PATH:-}:" in
  *":$INSTALL_DIR:"*) say "On your PATH already. Verify the install: secretagent doctor" ;;
  *) say ""; say "Add it to your PATH (this session):"; say "  export PATH=\"$INSTALL_DIR:\$PATH\"";
     say "Then verify: secretagent doctor" ;;
esac
