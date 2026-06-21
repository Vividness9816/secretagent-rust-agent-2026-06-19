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

    if ok {
        println!("doctor: OK");
        Ok(())
    } else {
        std::process::exit(1);
    }
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
