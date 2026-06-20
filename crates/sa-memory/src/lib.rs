use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 3;

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
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.role, m.content
               FROM messages_fts f
               JOIN messages m ON m.id = f.rowid
              WHERE m.session_id = ?1 AND messages_fts MATCH ?2
              ORDER BY rank
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, query, n as i64], |r| {
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
        assert_eq!(v, 3);
        assert_eq!(SCHEMA_VERSION, 3);
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
        assert_eq!(v, 3);
        assert_eq!(s.preferences().unwrap().len(), 1, "user_model must survive");
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM skills", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
