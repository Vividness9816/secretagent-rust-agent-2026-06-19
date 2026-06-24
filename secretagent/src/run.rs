use anyhow::Context;
use sa_audit::Audit;
use sa_core_types::config;

pub async fn run(
    session: &str,
    task: &str,
    auto_approve: bool,
    allow_unsandboxed_exec: bool,
) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let mut audit = Audit::open(&config::audit_path())?;

    // The screaming override is per-invocation + never persisted. Announce it LOUDLY and
    // record it (fsync) before anything runs, so the grant is the first durable event.
    if allow_unsandboxed_exec {
        audit.append_synced(sa_audit::AuditEvent {
            action: "exec.override.UNSANDBOXED".into(),
            key_id: "run".into(),
            principal: Some("operator".into()), // CLI-only flag; always the operator
        })?;
        eprintln!(
            "!!! --allow-unsandboxed-exec ENABLED: execute_code may run with NO sandbox this run !!!"
        );
    }

    // Assemble the agent + registry via the shared seam (Phase 6a). execute_code is always
    // registered; it refuses at dispatch unless its backend is enforced (or the per-invocation
    // override is on, local only). The backend is operator-frozen config (BLOCKER #2), never a
    // model arg. The 3 safe tools come default; configured MCP tools load namespaced + allow-listed.
    let agent = crate::setup::build_agent(&cfg)?;
    let (registry, backend_label) =
        crate::setup::build_registry(&cfg, allow_unsandboxed_exec).await?;
    // Record which backend execute_code is armed with (5a gate: the audit records the backend).
    crate::exec::audit_backend_armed(&mut audit, &backend_label)?;

    let answer = agent
        .run_task(
            session,
            task,
            &registry,
            &cfg.policy,
            &mut audit,
            &sa_core::RunContext::operator(auto_approve),
        )
        .await
        .context("agentic task failed — is the model endpoint reachable?")?;
    println!("{answer}");
    Ok(())
}
