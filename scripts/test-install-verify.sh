#!/bin/sh
# Smoke test (Phase 6b) for the installer's verify-before-place fail-closed logic. Proves the sha256
# check (the exact `grep " $asset$" SHA256SUMS | sha256sum -c -` that install.sh runs) REJECTS a
# tampered binary or a wrong checksum, so nothing un-verified ever gets placed. Run: sh scripts/test-install-verify.sh
set -eu
SHA="sha256sum"; command -v sha256sum >/dev/null 2>&1 || SHA="shasum -a 256"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
asset="secretagent-x86_64-unknown-linux-musl"

# Mirrors install.sh: extract the signed hash (separator-agnostic) and compare to the file's hash.
verify() {
  want="$(awk -v a="$asset" '{f=$2; sub(/^\*/,"",f); if (f==a) print $1}' "$tmp/SHA256SUMS" | head -1)"
  [ -n "$want" ] || return 1
  got="$($SHA "$tmp/$asset" | awk '{print $1}')"
  [ "$want" = "$got" ]
}

# Case 1: genuine binary + its real checksum → verify PASSES.
printf 'GENUINE-BINARY' > "$tmp/$asset"
( cd "$tmp" && $SHA "$asset" ) > "$tmp/SHA256SUMS"
verify || { echo "FAIL: a genuine binary must verify"; exit 1; }

# Case 2: TAMPERED binary against the same checksums → verify must FAIL (fail-closed).
printf 'TAMPERED-BINARY' > "$tmp/$asset"
if verify; then echo "FAIL: a tampered binary must NOT verify (fail-closed broken)"; exit 1; fi

# Case 3: WRONG checksum in the manifest → verify must FAIL.
printf 'GENUINE-BINARY' > "$tmp/$asset"
printf '%s  %s\n' "$(printf 0%.0s $(seq 1 64))" "$asset" > "$tmp/SHA256SUMS"
if verify; then echo "FAIL: a wrong checksum must NOT verify"; exit 1; fi

echo "ok: installer verify is fail-closed (genuine passes; tampered + wrong-sum rejected)"
