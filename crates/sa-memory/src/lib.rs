use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 4;

/// Canonical message store. `messages` is the single source of truth; `messages_fts`
/// is a rebuildable derived index (ADR invariant #1).
pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct StoredMsg {
    pub id: i64,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preference {
    pub dimension: String,
    pub value: String,
    /// Serialized `sa_core_types::Provenance` (always `{"kind":"trusted"}` for stated prefs).
    pub provenance: String,
    pub source_session: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub body: String,
    pub status: String,
    /// Serialized `Provenance` (opaque string here — sa-memory has no sa-core-types dep).
    /// Born `{"kind":"untrusted","source":"self-authored"}`; flips to `{"kind":"trusted"}`
    /// ONLY via `activate_skill`.
    pub provenance: String,
    pub score: f64,
    pub runs: i64,
    pub created_from_session: String,
}

/// A trusted+active skill + its body — the ONLY shape that reaches the system preamble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSkill {
    pub name: String,
    pub body: String,
}

/// A rolling per-session summary covering messages up to `through_id` (Phase 3c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub through_id: i64,
    pub text: String,
}

/// Defense-in-depth alarm (NOT the wall — the wall is the sa-core starvation control).
/// A deterministic, dependency-free scan for obvious secret material in a skill body.
fn looks_like_secret(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    if l.contains("secret=")
        || l.contains("secret =")
        || l.contains("api_key=")
        || l.contains("api_key =")
        || l.contains("-----begin")
    {
        return true;
    }
    // Run the token-prefix matchers on the lowercased copy so case can't evade them
    // (`l` is ASCII-lowercased, so byte indices align with `s`). sk-XXXXXXXX (>=8 alnum).
    for (i, _) in l.match_indices("sk-") {
        let n = l[i + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if n >= 8 {
            return true;
        }
    }
    // AKIA + >=12 alphanumerics (AWS access key id).
    if let Some(i) = l.find("akia") {
        let n = l[i + 4..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if n >= 12 {
            return true;
        }
    }
    // Common token prefixes (GitHub / Slack) — defense-in-depth alarm, not the wall.
    for p in [
        "ghp_",
        "gho_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xapp-",
    ] {
        if l.contains(p) {
            return true;
        }
    }
    false
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::migrate(&conn)?;
        Ok(Store { conn })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        use rusqlite::OptionalExtension;
        // ponytail: ordered (version, SQL) list + a single schema_meta version pointer is the
        // minimal versioned migration. Per-migration applied_at history (§6 `migrations` table)
        // is deferred — the audit log + git already record *when*; the version pointer is what
        // prevents re-running. Migration 2+ use plain CREATE (not IF NOT EXISTS) so version
        // gating is the real mechanism, not a silent no-op.
        const MIGRATIONS: &[(u32, &str)] = &[
            (
                1,
                "CREATE TABLE IF NOT EXISTS messages (
                    id         INTEGER PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    role       TEXT NOT NULL,
                    content    TEXT NOT NULL,
                    provenance TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );
                 CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
                 CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                    content, content='messages', content_rowid='id'
                 );
                 CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
                 END;",
            ),
            (
                2,
                "CREATE TABLE user_model (
                    id             INTEGER PRIMARY KEY,
                    dimension      TEXT NOT NULL UNIQUE,
                    value          TEXT NOT NULL,
                    provenance     TEXT NOT NULL,
                    source_session TEXT NOT NULL,
                    updated_at     INTEGER NOT NULL DEFAULT (unixepoch())
                 );",
            ),
            (
                3,
                "CREATE TABLE skills (
                    id                   INTEGER PRIMARY KEY,
                    name                 TEXT NOT NULL UNIQUE,
                    description          TEXT NOT NULL,
                    body                 TEXT NOT NULL,
                    status               TEXT NOT NULL DEFAULT 'draft',
                    provenance           TEXT NOT NULL,
                    score                REAL NOT NULL DEFAULT 0.0,
                    runs                 INTEGER NOT NULL DEFAULT 0,
                    created_from_session TEXT NOT NULL,
                    created_at           INTEGER NOT NULL DEFAULT (unixepoch()),
                    updated_at           INTEGER NOT NULL DEFAULT (unixepoch())
                 );
                 CREATE TABLE skill_versions (
                    id         INTEGER PRIMARY KEY,
                    skill_id   INTEGER NOT NULL REFERENCES skills(id),
                    version    INTEGER NOT NULL,
                    body       TEXT NOT NULL,
                    eval_score REAL NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                    UNIQUE(skill_id, version)
                 );
                 CREATE INDEX idx_skill_versions_skill ON skill_versions(skill_id);
                 CREATE VIRTUAL TABLE skills_fts USING fts5(
                    name, description, content='skills', content_rowid='id'
                 );
                 CREATE TRIGGER skills_ai AFTER INSERT ON skills BEGIN
                    INSERT INTO skills_fts(rowid, name, description)
                        VALUES (new.id, new.name, new.description);
                 END;",
            ),
            (
                4,
                "CREATE TABLE session_summaries (
                    session_id TEXT PRIMARY KEY,
                    through_id INTEGER NOT NULL,
                    summary    TEXT NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );",
            ),
        ];
        conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_meta (version INTEGER NOT NULL);")?;
        let current: u32 = conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        let latest = MIGRATIONS.last().map(|&(v, _)| v).unwrap_or(0);
        if current >= latest {
            return Ok(());
        }
        // One transaction: a half-applied migration must never leave a wedged schema.
        let tx = conn.unchecked_transaction()?;
        for &(v, sql) in MIGRATIONS {
            if v > current {
                tx.execute_batch(sql)?;
            }
        }
        tx.execute("DELETE FROM schema_meta", [])?;
        tx.execute("INSERT INTO schema_meta(version) VALUES (?1)", [latest])?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        provenance_json: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO messages(session_id, role, content, provenance) VALUES (?1,?2,?3,?4)",
            rusqlite::params![session_id, role, content, provenance_json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Last `n` messages for a session, oldest-first.
    pub fn recent(&self, session_id: &str, n: usize) -> Result<Vec<StoredMsg>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content FROM messages WHERE session_id=?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, n as i64], |r| {
            Ok(StoredMsg {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
            })
        })?;
        let mut v: Vec<StoredMsg> = rows.collect::<rusqlite::Result<_>>()?;
        v.reverse();
        Ok(v)
    }

    /// FTS5 keyword recall within a session, best-match first.
    pub fn recall(&self, session_id: &str, query: &str, n: usize) -> Result<Vec<StoredMsg>> {
        // Quote as an FTS5 string literal so a bareword (AND/OR/NOT) or punctuation in the
        // keyword can't be parsed as an operator and abort the query (escape embedded quotes).
        let q = format!("\"{}\"", query.replace('"', "\"\""));
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.role, m.content
               FROM messages_fts f
               JOIN messages m ON m.id = f.rowid
              WHERE m.session_id = ?1 AND messages_fts MATCH ?2
              ORDER BY rank
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, q, n as i64], |r| {
            Ok(StoredMsg {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Drop and repopulate the derived FTS index from the canonical table.
    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM messages_fts;
             INSERT INTO messages_fts(rowid, content) SELECT id, content FROM messages;",
        )?;
        Ok(())
    }

    /// Drop and repopulate the derived skill FTS index from the canonical `skills` table
    /// (ADR invariant #1 — every index rebuildable). Mirrors `rebuild_fts`.
    pub fn rebuild_skill_fts(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM skills_fts;
             INSERT INTO skills_fts(rowid, name, description)
                 SELECT id, name, description FROM skills;",
        )?;
        Ok(())
    }

    /// Upsert a stated preference by dimension (latest-stated wins). The caller passes
    /// serialized provenance — preferences are only ever written by the operator CLI as
    /// `Provenance::Trusted`; nothing in the model/tool path writes here.
    pub fn set_preference(
        &self,
        dimension: &str,
        value: &str,
        provenance_json: &str,
        source_session: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO user_model(dimension, value, provenance, source_session)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(dimension) DO UPDATE SET
                value=excluded.value,
                provenance=excluded.provenance,
                source_session=excluded.source_session,
                updated_at=unixepoch()",
            rusqlite::params![dimension, value, provenance_json, source_session],
        )?;
        Ok(())
    }

    /// All stated preferences, dimension ASC — surfaced into the system preamble.
    pub fn preferences(&self) -> Result<Vec<Preference>> {
        let mut stmt = self.conn.prepare(
            "SELECT dimension, value, provenance, source_session FROM user_model ORDER BY dimension",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Preference {
                dimension: r.get(0)?,
                value: r.get(1)?,
                provenance: r.get(2)?,
                source_session: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn map_skill(r: &rusqlite::Row) -> rusqlite::Result<Skill> {
        Ok(Skill {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            body: r.get(3)?,
            status: r.get(4)?,
            provenance: r.get(5)?,
            score: r.get(6)?,
            runs: r.get(7)?,
            created_from_session: r.get(8)?,
        })
    }

    /// Create a skill, BORN 'draft' + Untrusted (caller passes serialized provenance).
    /// Rejects a body that `looks_like_secret` (defense in depth). Atomically writes the
    /// skill row AND its version-1 lineage row. UNIQUE(name) errors on duplicate.
    pub fn create_skill(
        &self,
        name: &str,
        description: &str,
        body: &str,
        provenance_json: &str,
        created_from_session: &str,
        eval_score: f64,
    ) -> Result<i64> {
        if looks_like_secret(body) {
            anyhow::bail!("refusing to persist a skill body that looks like it contains a secret");
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO skills(name, description, body, provenance, score, created_from_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                name,
                description,
                body,
                provenance_json,
                eval_score,
                created_from_session
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO skill_versions(skill_id, version, body, eval_score) VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![id, body, eval_score],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn get_skill_by_name(&self, name: &str) -> Result<Option<Skill>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, description, body, status, provenance, score, runs, created_from_session
                   FROM skills WHERE name = ?1",
                [name],
                Self::map_skill,
            )
            .optional()?)
    }

    /// FTS5 keyword recall over (name, description), best-match first. Sanitizes the query
    /// per-word (alphanumeric, len>=3) like `recall`, so a crafted task can't throw on FTS5
    /// special chars. Returns ALL matches (drafts included); the caller filters by trust.
    pub fn recall_skills(&self, query: &str, n: usize) -> Result<Vec<Skill>> {
        let mut terms: Vec<String> = query
            .split_whitespace()
            .map(|raw| {
                raw.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
            })
            .filter(|w| w.len() >= 3)
            // Quote each term as an FTS5 string literal: a bareword like AND/OR/NOT in the task
            // text must never be parsed as an FTS5 operator (that aborts the whole query).
            // Terms are alphanumeric-only, so they contain no `"` to escape.
            .map(|w| format!("\"{w}\""))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        terms.sort();
        terms.dedup();
        terms.truncate(32); // bound the MATCH expression for a pathologically long task
        let match_expr = terms.join(" OR ");
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.name, s.description, s.body, s.status, s.provenance,
                    s.score, s.runs, s.created_from_session
               FROM skills_fts f
               JOIN skills s ON s.id = f.rowid
              WHERE skills_fts MATCH ?1
              ORDER BY rank
              LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![match_expr, n as i64], Self::map_skill)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Recall filtered to skills that are BOTH status=='active' AND provenance==Trusted —
    /// the ONLY skills eligible to enter the instruction stream.
    pub fn active_matching_skills(&self, query: &str, n: usize) -> Result<Vec<ActiveSkill>> {
        Ok(self
            .recall_skills(query, n)?
            .into_iter()
            .filter(|s| s.status == "active" && s.provenance == r#"{"kind":"trusted"}"#)
            .map(|s| ActiveSkill {
                name: s.name,
                body: s.body,
            })
            .collect())
    }

    /// The SOLE trust-flip writer: status -> 'active' AND provenance -> Trusted (caller
    /// passes serde_json of Provenance::Trusted). Returns rows affected (0 = no such skill).
    pub fn activate_skill(&self, name: &str, trusted_provenance_json: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE skills SET status='active', provenance=?2, updated_at=unixepoch() WHERE name=?1",
            rusqlite::params![name, trusted_provenance_json],
        )?)
    }

    /// Record a reuse: bump runs + latest score, and append an immutable `skill_versions`
    /// lineage row. The COMPOSED body (`skills.body`) is FROZEN after create — a Trusted+active
    /// skill's instruction must NOT silently change on a reuse run (that would launder a
    /// model-echoed/injected span into the next session's trusted preamble). Adopting a refined
    /// body is a future re-approval flow, never a side effect of use. Re-runs the secret-grep so
    /// the lineage write enjoys the same defense-in-depth as `create_skill`.
    pub fn record_skill_use(&self, name: &str, new_body: &str, eval_score: f64) -> Result<()> {
        if looks_like_secret(new_body) {
            anyhow::bail!("refusing to record a skill refinement whose body looks like a secret");
        }
        let tx = self.conn.unchecked_transaction()?;
        let id: i64 = tx.query_row("SELECT id FROM skills WHERE name=?1", [name], |r| r.get(0))?;
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM skill_versions WHERE skill_id=?1",
            [id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO skill_versions(skill_id, version, body, eval_score) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, next, new_body, eval_score],
        )?;
        // NOTE: skills.body is deliberately NOT updated here (frozen-post-create).
        tx.execute(
            "UPDATE skills SET runs = runs + 1, score = ?2, updated_at = unixepoch() WHERE id = ?1",
            rusqlite::params![id, eval_score],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// All skills, name ASC — backs `secretagent skill list`.
    pub fn list_skills(&self) -> Result<Vec<Skill>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, body, status, provenance, score, runs, created_from_session
               FROM skills ORDER BY name",
        )?;
        let rows = stmt.query_map([], Self::map_skill)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The rolling summary for a session, if any (Phase 3c).
    pub fn summary(&self, session_id: &str) -> Result<Option<Summary>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT through_id, summary FROM session_summaries WHERE session_id=?1",
                [session_id],
                |r| {
                    Ok(Summary {
                        through_id: r.get(0)?,
                        text: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Upsert the rolling summary (covers messages up to `through_id`).
    pub fn set_summary(&self, session_id: &str, through_id: i64, text: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO session_summaries(session_id, through_id, summary) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
                through_id=excluded.through_id, summary=excluded.summary, updated_at=unixepoch()",
            rusqlite::params![session_id, through_id, text],
        )?;
        Ok(())
    }

    /// All messages for a session, oldest-first. ponytail: whole-session load for an
    /// occasional explicit summarize; window/paginate if a session ever gets pathological.
    pub fn all_messages(&self, session_id: &str) -> Result<Vec<StoredMsg>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content FROM messages WHERE session_id=?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([session_id], |r| {
            Ok(StoredMsg {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The serialized provenance strings of a session's messages, oldest-first. Read-only;
    /// used to assert a remote turn was stamped Untrusted{source} (test/forensic).
    pub fn message_provenances(&self, session_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT provenance FROM messages WHERE session_id = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map([session_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_read_recent_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.add_message("s1", "user", "first fact: my cat is named Mochi", "{}")
            .unwrap();
        s.add_message("s1", "assistant", "noted", "{}").unwrap();
        let recent = s.recent("s1", 10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content, "first fact: my cat is named Mochi");
        assert_eq!(recent[1].role, "assistant");
    }

    #[test]
    fn recall_finds_a_fact_by_keyword() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.add_message("s1", "user", "my cat is named Mochi", "{}")
            .unwrap();
        s.add_message("s1", "user", "the weather is nice", "{}")
            .unwrap();
        let hits = s.recall("s1", "cat", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("Mochi"));
    }

    #[test]
    fn fts_is_rebuildable_from_canonical_messages() {
        // ADR invariant #1: every index rebuildable from canonical tables.
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.add_message("s1", "user", "my cat is named Mochi", "{}")
            .unwrap();
        let before = s.recall("s1", "Mochi", 5).unwrap();
        s.rebuild_fts().unwrap();
        let after = s.recall("s1", "Mochi", 5).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(after.len(), 1);
        assert_eq!(before[0].content, after[0].content);
    }

    #[test]
    fn migration_creates_user_model_and_is_idempotent_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        // First open: runner applies migrations 1 and 2.
        {
            let s = Store::open(&db).unwrap();
            // user_model exists and is queryable (count = 0).
            let n: i64 = s
                .conn
                .query_row("SELECT count(*) FROM user_model", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0);
        }
        // Reopen MUST NOT error — version gating prevents re-running the plain CREATE TABLE.
        let s2 = Store::open(&db).unwrap();
        let v: u32 = s2
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn a_v1_db_upgrades_to_v2_without_losing_messages() {
        // Simulate a Phase-1/2 database: schema_meta version=1 + a real message, NO user_model.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("legacy.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_meta (version INTEGER NOT NULL);
                 INSERT INTO schema_meta(version) VALUES (1);
                 CREATE TABLE messages (
                    id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
                    content TEXT NOT NULL, provenance TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL DEFAULT (unixepoch()));
                 INSERT INTO messages(session_id, role, content) VALUES ('s1','user','my cat is Mochi');",
            )
            .unwrap();
        }
        // Opening with the new runner upgrades to the latest version; message intact.
        let s = Store::open(&db).unwrap();
        let v: u32 = s
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let msg: String = s
            .conn
            .query_row(
                "SELECT content FROM messages WHERE session_id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg, "my cat is Mochi");
        let um: i64 = s
            .conn
            .query_row("SELECT count(*) FROM user_model", [], |r| r.get(0))
            .unwrap();
        assert_eq!(um, 0);
    }

    #[test]
    fn set_preference_upserts_by_dimension_latest_wins() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("p.db")).unwrap();
        s.set_preference("tone", "formal", r#"{"kind":"trusted"}"#, "cli")
            .unwrap();
        s.set_preference("tone", "concise", r#"{"kind":"trusted"}"#, "cli")
            .unwrap();
        s.set_preference("timezone", "PST", r#"{"kind":"trusted"}"#, "cli")
            .unwrap();
        let prefs = s.preferences().unwrap();
        assert_eq!(prefs.len(), 2, "tone upserted, not duplicated");
        let tone = prefs.iter().find(|p| p.dimension == "tone").unwrap();
        assert_eq!(tone.value, "concise");
        assert_eq!(tone.provenance, r#"{"kind":"trusted"}"#);
        assert_eq!(tone.source_session, "cli");
    }

    #[test]
    fn preferences_empty_on_fresh_db() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("p.db")).unwrap();
        assert!(s.preferences().unwrap().is_empty());
    }

    #[test]
    fn migration_3_creates_skills_tables_and_bumps_version() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        let v: u32 = s
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        for t in ["skills", "skill_versions"] {
            let n: i64 = s
                .conn
                .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{t} should exist and be empty");
        }
    }

    #[test]
    fn a_v2_db_upgrades_to_v3_without_losing_user_model() {
        // Simulate a 3a database: version=2, user_model populated, NO skills tables.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("v2.db");
        {
            let s = Store::open(&db).unwrap();
            s.set_preference("tone", "concise", r#"{"kind":"trusted"}"#, "cli")
                .unwrap();
            s.conn
                .execute("UPDATE schema_meta SET version = 2", [])
                .unwrap();
            // Drop ALL post-v2 tables so the runner re-applies migrations 3 AND 4.
            s.conn
                .execute("DROP TABLE IF EXISTS session_summaries", [])
                .ok();
            s.conn.execute("DROP TABLE IF EXISTS skills_fts", []).ok();
            s.conn
                .execute("DROP TABLE IF EXISTS skill_versions", [])
                .ok();
            s.conn.execute("DROP TABLE IF EXISTS skills", []).ok();
        }
        let s = Store::open(&db).unwrap();
        let v: u32 = s
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert_eq!(s.preferences().unwrap().len(), 1, "user_model must survive");
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM skills", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    const UNTRUSTED: &str = r#"{"kind":"untrusted","source":"self-authored"}"#;
    const TRUSTED: &str = r#"{"kind":"trusted"}"#;

    #[test]
    fn create_skill_is_born_draft_untrusted_with_a_version_row() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        let id = s
            .create_skill(
                "deploy-app",
                "deploy the app",
                "step1; step2",
                UNTRUSTED,
                "s1",
                0.8,
            )
            .unwrap();
        let got = s.get_skill_by_name("deploy-app").unwrap().unwrap();
        assert_eq!(got.status, "draft");
        assert_eq!(got.provenance, UNTRUSTED);
        assert_eq!(got.score, 0.8);
        let nv: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM skill_versions WHERE skill_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nv, 1);
    }

    #[test]
    fn create_skill_rejects_a_body_that_looks_like_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        let leaky = "do the thing. SECRET=sk-sentinel-7777";
        assert!(
            s.create_skill("leaky", "x", leaky, UNTRUSTED, "s1", 0.5)
                .is_err(),
            "a body carrying a secret must be rejected at the write boundary"
        );
        s.create_skill(
            "leaky",
            "x",
            "call fetch then summarize",
            UNTRUSTED,
            "s1",
            0.5,
        )
        .unwrap();
    }

    #[test]
    fn recall_and_active_filter_only_returns_trusted_active_skills() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.create_skill(
            "summarize-url",
            "summarize a web page",
            "fetch then summarize",
            UNTRUSTED,
            "s1",
            0.7,
        )
        .unwrap();
        assert_eq!(s.recall_skills("summarize web", 5).unwrap().len(), 1);
        assert!(s
            .active_matching_skills("summarize web", 5)
            .unwrap()
            .is_empty());
        assert_eq!(s.activate_skill("summarize-url", TRUSTED).unwrap(), 1);
        let active = s.active_matching_skills("summarize web", 5).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "summarize-url");
        assert_eq!(active[0].body, "fetch then summarize");
    }

    #[test]
    fn record_skill_use_bumps_runs_and_appends_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.create_skill("x", "d", "body-v1", UNTRUSTED, "s1", 0.5)
            .unwrap();
        s.record_skill_use("x", "body-v2", 0.9).unwrap();
        let got = s.get_skill_by_name("x").unwrap().unwrap();
        assert_eq!(got.runs, 1);
        // Composed body is FROZEN after create — reuse must NOT silently substitute it.
        assert_eq!(got.body, "body-v1", "composed body stays as-approved");
        assert_eq!(got.score, 0.9);
        assert_eq!(got.provenance, UNTRUSTED, "reuse never changes trust");
        // ...but the refined body IS recorded in the append-only lineage.
        let nv: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM skill_versions WHERE skill_id=?1",
                [got.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nv, 2);
        let v2: String = s
            .conn
            .query_row(
                "SELECT body FROM skill_versions WHERE skill_id=?1 AND version=2",
                [got.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v2, "body-v2");
    }

    #[test]
    fn record_skill_use_rejects_a_secret_refinement() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.create_skill("x", "d", "clean body", UNTRUSTED, "s1", 0.5)
            .unwrap();
        // a model-echoed secret on a reuse run must be rejected at the write boundary
        assert!(s
            .record_skill_use("x", "leak: SECRET=sk-abcd1234efgh", 0.9)
            .is_err());
        // nothing was recorded — runs unchanged, no v2 lineage row
        let got = s.get_skill_by_name("x").unwrap().unwrap();
        assert_eq!(got.runs, 0);
    }

    #[test]
    fn recall_skills_handles_fts5_bareword_operators_in_a_task() {
        // A natural-language task with AND/OR/NOT must NOT abort the query (FTS5 operators).
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.create_skill(
            "deploy-and-verify",
            "deploy and verify the app",
            "steps",
            UNTRUSTED,
            "s1",
            0.7,
        )
        .unwrap();
        // "deploy AND verify NOT staging" — barewords would be FTS5 operators if unquoted.
        let hits = s.recall_skills("deploy AND verify NOT staging", 5).unwrap();
        assert!(!hits.is_empty(), "bareword operators must not abort recall");
    }

    #[test]
    fn looks_like_secret_is_case_insensitive_on_token_prefixes() {
        // create_skill is the public reach into looks_like_secret; uppercase must still trip.
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        assert!(s
            .create_skill("u", "d", "key SK-ABCD1234EFGH", UNTRUSTED, "s1", 0.5)
            .is_err());
        assert!(s
            .create_skill("g", "d", "token ghp_ABCDEFGH", UNTRUSTED, "s1", 0.5)
            .is_err());
    }

    #[test]
    fn migration_4_creates_session_summaries() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        let v: u32 = s
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 4);
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM session_summaries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn summary_upserts_and_all_messages_is_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        assert!(s.summary("s1").unwrap().is_none());
        s.add_message("s1", "user", "first", "{}").unwrap();
        s.add_message("s1", "assistant", "second", "{}").unwrap();
        let all = s.all_messages("s1").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content, "first");
        s.set_summary("s1", all[1].id, "a summary").unwrap();
        s.set_summary("s1", all[1].id, "a better summary").unwrap(); // upsert
        let got = s.summary("s1").unwrap().unwrap();
        assert_eq!(got.text, "a better summary");
        assert_eq!(got.through_id, all[1].id);
    }
}
