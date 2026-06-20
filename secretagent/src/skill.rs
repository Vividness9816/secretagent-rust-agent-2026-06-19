use anyhow::Result;
use sa_audit::{Audit, AuditEvent};
use sa_core_types::config;
use sa_core_types::types::Provenance;
use sa_memory::Store;

/// List skills: name, status, runs, score.
pub fn list() -> Result<()> {
    let store = Store::open(&config::db_path())?;
    for s in store.list_skills()? {
        println!(
            "{}  [{}]  runs={} score={:.2}",
            s.name, s.status, s.runs, s.score
        );
    }
    Ok(())
}

/// Operator activation: flip a skill to Trusted + active. Audited by name. Exits 2 if absent.
pub fn activate(name: &str) -> Result<()> {
    let store = Store::open(&config::db_path())?;
    let mut audit = Audit::open(&config::audit_path())?;
    let trusted = serde_json::to_string(&Provenance::Trusted)?;
    if store.activate_skill(name, &trusted)? == 0 {
        eprintln!("no such skill: {name}");
        std::process::exit(2);
    }
    audit.append_synced(AuditEvent {
        action: "skill.activate".into(),
        key_id: name.into(),
    })?;
    println!("activated skill: {name}");
    Ok(())
}
