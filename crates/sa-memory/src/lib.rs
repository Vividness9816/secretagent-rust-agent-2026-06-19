use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 2;

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
        assert_eq!(SCHEMA_VERSION, 2);
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
        // Opening with the new runner upgrades: user_model appears, version=2, message intact.
        let s = Store::open(&db).unwrap();
        let v: u32 = s
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2);
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
}
