use anyhow::Result;
use sa_core::SystemContext;
use sa_core_types::config;
use sa_core_types::types::Provenance;
use sa_memory::Store;

/// Read SOUL.md + context.md from the config dir (missing file = empty section).
pub fn load_system_context() -> SystemContext {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    SystemContext {
        soul: read(config::soul_path()),
        context: read(config::context_path()),
    }
}

/// Store a stated preference — always `Provenance::Trusted`, source "cli". This is the
/// ONLY writer of the user model; the model/tool path never writes here.
pub fn set(dimension: &str, value: &str) -> Result<()> {
    let store = Store::open(&config::db_path())?;
    let prov = serde_json::to_string(&Provenance::Trusted)?;
    store.set_preference(dimension, value, &prov, "cli")?;
    println!("remembered {dimension}: {value}");
    Ok(())
}

/// Print stated preferences (dimension: value).
pub fn list() -> Result<()> {
    let store = Store::open(&config::db_path())?;
    for p in store.preferences()? {
        println!("{}: {}", p.dimension, p.value);
    }
    Ok(())
}
