# SecretAgent installer (Phase 6b, Windows) — fetch, VERIFY (sha256 + Authenticode), THEN place.
# Verifies BEFORE placing and fails closed. PRINTS PATH guidance; never edits your PATH for you.
# Usage:  irm <url>/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$Repo = 'Vividness9816/secretagent-rust-agent-2026-06-19'
$Asset = 'secretagent-x86_64-pc-windows-msvc.exe'
$InstallDir = if ($env:SECRETAGENT_INSTALL_DIR) { $env:SECRETAGENT_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\secretagent" }
$Base = "https://github.com/$Repo/releases/latest/download"

$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ("sa-install-" + [Guid]::NewGuid()))
try {
    Write-Host "Downloading $Asset + checksums…"
    Invoke-WebRequest "$Base/$Asset"    -OutFile "$tmp\$Asset"
    Invoke-WebRequest "$Base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS"

    # 1) Verify the sha256 against the published checksums file.
    Write-Host "Verifying $Asset checksum…"
    $want = (Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match [Regex]::Escape($Asset) + '$' } |
             ForEach-Object { ($_ -split '\s+')[0] }) | Select-Object -First 1
    if (-not $want) { throw "no checksum line for $Asset in SHA256SUMS" }
    $got = (Get-FileHash "$tmp\$Asset" -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $want.ToLower()) { throw "CHECKSUM MISMATCH for $Asset — refusing to install" }

    # 2) Verify the Authenticode signature (the native Windows trust: "Verified publisher: Dylan N").
    #    Self-signed → Valid only where the cert is trusted; UnknownError if not yet imported.
    $sig = Get-AuthenticodeSignature "$tmp\$Asset"
    if ($sig.Status -eq 'Valid') {
        Write-Host "Authenticode: Valid — $($sig.SignerCertificate.Subject)"
    } else {
        Write-Warning "Authenticode status: $($sig.Status). The checksum verified; if you trust the publisher cert, import it (trust-codesign.ps1)."
    }

    # 3) Verified — NOW place it (verify-before-place).
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Move-Item "$tmp\$Asset" "$InstallDir\secretagent.exe" -Force
    Write-Host "Installed: $InstallDir\secretagent.exe"

    # 4) PRINT PATH guidance — never edits PATH for you.
    if (($env:PATH -split ';') -notcontains $InstallDir) {
        Write-Host ""
        Write-Host "Add it to your PATH (persistent, current user):"
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$InstallDir;`" + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
    }
    Write-Host "Then verify: secretagent doctor"
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
