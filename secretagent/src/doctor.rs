use sa_core_types::config;

/// Phase 0 doctor: config + vault, headless-safe. Provider/backend/DB checks land
/// in later phases. Keyring is informational and NEVER fails the run.
pub fn run() -> anyhow::Result<()> {
    let mut ok = true;

    match config::Config::load() {
        Ok(_) => println!("[ok]   config loads"),
        Err(e) => {
            println!("[fail] config: {e}");
            ok = false;
        }
    }

    let id = config::identity_path();
    if id.exists() {
        println!("[ok]   identity present: {id:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&id)?.permissions().mode() & 0o777;
            if mode == 0o600 {
                println!("[ok]   identity perms 0600");
            } else {
                println!("[fail] identity perms {mode:o} (want 0600) — run: chmod 600 {id:?}");
                ok = false;
            }
        }
    } else {
        println!("[warn] no identity yet — run: secretagent vault init");
    }

    // Unattended decrypt self-test: no prompt, no keyring, no D-Bus. Uses a
    // throwaway store so it never mutates real vault state.
    match self_test_vault() {
        Ok(()) => println!("[ok]   vault encrypt/decrypt works unattended"),
        Err(e) => {
            println!("[fail] vault self-test: {e}");
            ok = false;
        }
    }

    // Keyring is informational in Phase 0 — a missing keyring is the expected,
    // healthy state on a headless box (ADR: keyring off the default build).
    println!("[info] keyring: not used in this build (age-file backend) — expected");

    // Provider reachability is informational — a down endpoint is not a doctor
    // failure (you may be offline / configuring).
    let cfg = config::Config::load().unwrap_or_default();
    report_provider(&cfg.provider.base_url);

    // Landlock capability (ADR-20260620). Reported, never gating doctor's exit — the real
    // fail-closed gate is execute_code's dispatch-refuse, proven by the deny-corpus.
    match sa_exec::landlock_status() {
        sa_exec::LandlockStatus::Enforced { abi } => {
            println!("[ok]   landlock: enforced (ABI {abi}) — execute_code available")
        }
        sa_exec::LandlockStatus::Unavailable { reason } => {
            if cfg!(target_os = "linux") {
                println!("[warn] landlock: unavailable ({reason}) — execute_code disabled");
            } else {
                println!(
                    "[info] landlock: not applicable on this OS — execute_code disabled (expected)"
                );
            }
        }
    }

    // Execution backend (Phase 5a). The operator-frozen backend for execute_code; reported with
    // its HONEST confinement + availability, never gates doctor's exit.
    match crate::exec::backend_from_config(&cfg.exec) {
        Ok(b) => {
            let avail = match &b {
                sa_exec::Backend::Local(_) => true, // landlock already reported above
                sa_exec::Backend::Docker { .. } => backend_cli_present("docker"),
                sa_exec::Backend::Ssh { .. } => backend_cli_present("ssh"),
            };
            let note = confinement_note(&b.confinement());
            if avail {
                println!("[ok]   exec backend: {} ({note})", b.label());
            } else {
                println!(
                    "[warn] exec backend: {} ({note}) — CLI not found on PATH",
                    b.label()
                );
            }
        }
        Err(e) => println!("[warn] exec backend: {e}"),
    }

    // Voice (Phase 5d). Probe the configured STT/TTS commands by BINARY NAME only (argv[0]) — never
    // print the full command (a cloud wrapper could carry a key). Never gates doctor's exit.
    #[cfg(feature = "voice")]
    {
        let v = &cfg.voice;
        if v.stt_cmd.is_empty() || v.tts_cmd.is_empty() {
            println!("[info] voice: not configured ([voice] stt_cmd/tts_cmd)");
        } else {
            for (which, argv) in [("stt", &v.stt_cmd), ("tts", &v.tts_cmd)] {
                if backend_cli_present(&argv[0]) {
                    println!("[ok]   voice {which}: {} on PATH", argv[0]);
                } else {
                    println!("[warn] voice {which}: {} not found on PATH", argv[0]);
                }
            }
        }
    }

    // OS service (Phase 4b). Reported, never gates doctor's exit (founding-ADR doctor-exit-0).
    println!("[info] service: {}", crate::service::status());

    // MCP servers (informational). Tools load at run time, namespaced + allow-listed.
    let cfg_for_mcp = config::Config::load().unwrap_or_default();
    if cfg_for_mcp.mcp.is_empty() {
        println!("[info] mcp: no servers configured");
    } else {
        let names: Vec<&str> = cfg_for_mcp.mcp.iter().map(|m| m.name.as_str()).collect();
        println!(
            "[info] mcp: {} server(s) configured: {} (tools load at run time, namespaced + allow-listed)",
            names.len(),
            names.join(", ")
        );
    }

    // Audit-log integrity (6g ops): verify the live hash-chain so the operator knows the log is
    // intact before a backup / after a restore. Reported, never gates exit (a torn tail is a
    // report, not a fatal — the founding-ADR doctor-exit-0 rule; the real signal is restore's check).
    let ap = config::audit_path();
    if ap.exists() {
        match sa_audit::Audit::verify_chain(&ap) {
            Ok(true) => println!("[ok]   audit chain: verified"),
            Ok(false) => println!("[warn] audit chain: UNVERIFIED (torn tail or tamper) — {ap:?}"),
            Err(e) => println!("[warn] audit chain: could not read: {e}"),
        }
    } else {
        println!("[info] audit log: none yet (no audited actions)");
    }

    // Binary integrity (§9): print the running binary's SHA-256 + version so the operator can verify
    // it matches the published `<target>.sha256` (offline provenance check). Never gates exit.
    match self_sha256() {
        Ok(h) => println!(
            "[ok]   binary: secretagent {} — sha256 {h}",
            env!("CARGO_PKG_VERSION")
        ),
        Err(e) => println!("[warn] binary integrity: could not hash self: {e}"),
    }

    if ok {
        println!("doctor: OK");
        Ok(())
    } else {
        std::process::exit(1);
    }
}

/// SHA-256 of the running binary, so `doctor` can print it for the operator to compare against the
/// published `<target>.sha256` (the §9 offline integrity check). Reads the whole exe — cheap + rare.
fn self_sha256() -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let path = std::env::current_exe()?;
    let bytes = std::fs::read(&path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

/// Honest one-line description of a backend's confinement story (never overstates).
fn confinement_note(c: &sa_exec::Confinement) -> String {
    match c {
        sa_exec::Confinement::LocalKernel(sa_exec::LandlockStatus::Enforced { abi }) => {
            format!("local, landlock enforced ABI {abi}")
        }
        sa_exec::Confinement::LocalKernel(sa_exec::LandlockStatus::Unavailable { reason }) => {
            format!("local, landlock unavailable: {reason}")
        }
        sa_exec::Confinement::Container { image } => {
            format!("docker {image}, operator-vouched, --network=none")
        }
        sa_exec::Confinement::RemoteHost { host } => {
            format!("remote {host}, operator-vouched; egress NOT confined by us")
        }
    }
}

/// True if the backend's CLI is on PATH (spawns `<bin> --version`; exit code ignored).
fn backend_cli_present(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok()
}

/// Best-effort, non-gating reachability probe of the configured provider endpoint.
fn report_provider(base_url: &str) {
    use std::net::ToSocketAddrs;
    let after = base_url.split("://").nth(1).unwrap_or(base_url);
    let host = after.split('/').next().unwrap_or(after);
    let hostport = if host.contains(':') {
        host.to_string()
    } else if base_url.starts_with("https") {
        format!("{host}:443")
    } else {
        format!("{host}:80")
    };
    let reachable = hostport
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .map(|addr| {
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
                .is_ok()
        })
        .unwrap_or(false);
    if reachable {
        println!("[ok]   provider endpoint reachable: {base_url}");
    } else {
        println!("[info] provider endpoint not reachable ({base_url}) — expected if offline");
    }
}

fn self_test_vault() -> anyhow::Result<()> {
    use sa_vault::{age_file::AgeFileVault, SecretString, Vault};
    use secrecy::ExposeSecret;

    let dir = std::env::temp_dir().join(format!("secretagent-doctor-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let mut t = AgeFileVault::open_or_init(&dir.join("id.age"), &dir.join("st.age"))?;
    t.set("n", SecretString::new("doctor-nonce".into()))?;
    let got = t
        .get("n")?
        .ok_or_else(|| anyhow::anyhow!("nonce missing after set"))?;
    anyhow::ensure!(got.expose_secret() == "doctor-nonce", "nonce mismatch");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn self_sha256_hashes_the_running_binary_to_64_hex() {
        // Hashes the test binary itself (a real on-disk exe) — proves the integrity helper works.
        let h = super::self_sha256().unwrap();
        assert_eq!(h.len(), 64, "sha256 hex must be 64 chars: {h}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "non-hex in {h}");
    }
}
