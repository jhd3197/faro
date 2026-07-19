//! `faro.db` — one shared SQLite database for every subsystem that needs a
//! persisted per-connection index (see `docs/plans/3_scan-index-foundation.md`).
//!
//! The win is deliberately modest: **one** connection + **one** migration runner,
//! with **per-feature tables** in the same file. No grand unified schema. This
//! plan lands the `sync_state` table (folder-sync's per-file memory); Plans 6/7/8
//! add `scan_cache` / `search_index` / `diff_hash` as *additive* migrations —
//! never a rewrite.
//!
//! Migrations are forward-only and versioned via SQLite's `user_version` pragma:
//! append a statement to [`MIGRATIONS`] and bump nothing else. `rusqlite`'s
//! `bundled` feature compiles SQLite from source, so there's no system dependency.
//!
//! The connection is guarded by a `std::sync::Mutex`. Every query runs to
//! completion under the lock and returns owned data — the guard is never held
//! across an `.await`, so callers stay `Send`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch — the timestamp unit every `faro.db` row
/// uses. Monotonic enough for ordering; not security-sensitive.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Forward-only schema migrations. Index = target `user_version - 1`; appending
/// a statement is the *only* supported schema change. Never edit or reorder an
/// existing entry — a shipped `faro.db` has already run it.
const MIGRATIONS: &[&str] = &[
    // v1 — folder-sync per-file index. Keyed by (pair_id, rel_path). `size`/
    // `mtime` are the *source-side* values captured at last sync; `remote_signal`
    // is the backend's opaque change token (ETag/hash) when it has one, else NULL.
    "CREATE TABLE sync_state (
        pair_id        TEXT    NOT NULL,
        rel_path       TEXT    NOT NULL,
        size           INTEGER NOT NULL,
        mtime          INTEGER NOT NULL,
        remote_signal  TEXT,
        last_synced_ms INTEGER NOT NULL,
        state          TEXT    NOT NULL DEFAULT 'synced',
        PRIMARY KEY (pair_id, rel_path)
    ) WITHOUT ROWID;",
    // v2 — command snippets (Plan 11 Phase 4). The low-friction, single-session
    // counterpart to Fleet Skills: a saved command line with optional
    // `{{variable}}` placeholders, insertable into a live shell with one
    // keystroke. `folder` groups them; `use_count` feeds palette ordering.
    "CREATE TABLE snippets (
        id         TEXT    PRIMARY KEY,
        name       TEXT    NOT NULL,
        body       TEXT    NOT NULL,
        folder     TEXT,
        use_count  INTEGER NOT NULL DEFAULT 0,
        created_ms INTEGER NOT NULL,
        updated_ms INTEGER NOT NULL
    );",
];

/// A persisted per-file sync record — what the source looked like the last time
/// this pair synced it. The reconciler diffs the *current* source against this to
/// catch same-size edits the live size+mtime comparison would miss.
#[derive(Debug, Clone)]
pub struct SyncStateRow {
    pub size: u64,
    pub mtime: i64,
    pub remote_signal: Option<String>,
    pub last_synced_ms: i64,
}

/// A saved command snippet (Plan 11 Phase 4). `body` may contain `{{variable}}`
/// placeholders the UI resolves before inserting into a shell. Serialized to the
/// frontend as-is; `id` is a client-minted UUID.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub use_count: i64,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub updated_ms: i64,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Open (creating if absent) `faro.db` at `path` and run pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("open faro.db at {}", path.display()))?;
        // WAL keeps readers from blocking the single writer — this is a desktop
        // app with occasional background writes, not a hot OLTP path.
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        migrate(&conn).context("running faro.db migrations")?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Open an in-memory database (tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Load every remembered file for a pair, keyed by relative path.
    pub fn load_sync_state(&self, pair_id: &str) -> Result<HashMap<String, SyncStateRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT rel_path, size, mtime, remote_signal, last_synced_ms
               FROM sync_state WHERE pair_id = ?1",
        )?;
        let rows = stmt.query_map([pair_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                SyncStateRow {
                    size: r.get::<_, i64>(1)? as u64,
                    mtime: r.get(2)?,
                    remote_signal: r.get(3)?,
                    last_synced_ms: r.get(4)?,
                },
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (k, v) = row?;
            out.insert(k, v);
        }
        Ok(out)
    }

    /// Record (insert or replace) one file's synced state.
    pub fn upsert_sync_state(
        &self,
        pair_id: &str,
        rel_path: &str,
        size: u64,
        mtime: i64,
        remote_signal: Option<&str>,
        last_synced_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_state
                 (pair_id, rel_path, size, mtime, remote_signal, last_synced_ms, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'synced')
             ON CONFLICT(pair_id, rel_path) DO UPDATE SET
                 size = excluded.size,
                 mtime = excluded.mtime,
                 remote_signal = excluded.remote_signal,
                 last_synced_ms = excluded.last_synced_ms,
                 state = 'synced'",
            rusqlite::params![pair_id, rel_path, size as i64, mtime, remote_signal, last_synced_ms],
        )?;
        Ok(())
    }

    /// Forget one file (it's gone from the source and has been mirrored away).
    pub fn delete_sync_state(&self, pair_id: &str, rel_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE pair_id = ?1 AND rel_path = ?2",
            rusqlite::params![pair_id, rel_path],
        )?;
        Ok(())
    }

    /// Drop a pair's entire index (the pair was deleted).
    pub fn clear_pair(&self, pair_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_state WHERE pair_id = ?1", [pair_id])?;
        Ok(())
    }

    // ---- Command snippets (Plan 11 Phase 4) ----

    /// Every snippet, most-used first (ties broken by most-recently-edited).
    /// The palette and panel both consume this ordering directly.
    pub fn list_snippets(&self) -> Result<Vec<Snippet>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, body, folder, use_count, created_ms, updated_ms
               FROM snippets
              ORDER BY use_count DESC, updated_ms DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Snippet {
                id: r.get(0)?,
                name: r.get(1)?,
                body: r.get(2)?,
                folder: r.get(3)?,
                use_count: r.get(4)?,
                created_ms: r.get(5)?,
                updated_ms: r.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Insert or update a snippet. An update preserves `use_count`/`created_ms`
    /// (they're the snippet's history, not part of the edit) and refreshes
    /// `updated_ms`. Returns the full, re-ordered list so callers stay in sync.
    pub fn upsert_snippet(
        &self,
        id: &str,
        name: &str,
        body: &str,
        folder: Option<&str>,
    ) -> Result<Vec<Snippet>> {
        let now = now_ms();
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO snippets (id, name, body, folder, use_count, created_ms, updated_ms)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     body = excluded.body,
                     folder = excluded.folder,
                     updated_ms = excluded.updated_ms",
                rusqlite::params![id, name, body, folder, now],
            )?;
        }
        self.list_snippets()
    }

    /// Delete one snippet; returns the surviving list.
    pub fn delete_snippet(&self, id: &str) -> Result<Vec<Snippet>> {
        {
            let conn = self.conn.lock().unwrap();
            conn.execute("DELETE FROM snippets WHERE id = ?1", [id])?;
        }
        self.list_snippets()
    }

    /// Record one use of a snippet (its text was inserted into a shell) so it
    /// floats up the ordering. Returns the re-ordered list.
    pub fn touch_snippet(&self, id: &str) -> Result<Vec<Snippet>> {
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE snippets SET use_count = use_count + 1 WHERE id = ?1",
                [id],
            )?;
        }
        self.list_snippets()
    }
}

/// Apply every migration whose index is `>= user_version`, bumping the pragma in
/// lockstep so a half-applied run resumes cleanly on the next open.
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        conn.execute_batch(sql)
            .with_context(|| format!("apply migration v{}", i + 1))?;
        // `user_version` can't be a bound parameter — the value is our own index.
        conn.pragma_update(None, "user_version", (i + 1) as i64)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_round_trips_sync_state() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.load_sync_state("p1").unwrap().is_empty());

        db.upsert_sync_state("p1", "a/b.txt", 42, 1000, Some("etag-1"), 5000)
            .unwrap();
        db.upsert_sync_state("p1", "c.txt", 7, 2000, None, 5001).unwrap();
        db.upsert_sync_state("p2", "a/b.txt", 99, 3000, None, 5002)
            .unwrap();

        let p1 = db.load_sync_state("p1").unwrap();
        assert_eq!(p1.len(), 2);
        let row = &p1["a/b.txt"];
        assert_eq!(row.size, 42);
        assert_eq!(row.mtime, 1000);
        assert_eq!(row.remote_signal.as_deref(), Some("etag-1"));
        assert_eq!(row.last_synced_ms, 5000);

        // upsert replaces in place
        db.upsert_sync_state("p1", "a/b.txt", 43, 1500, None, 6000)
            .unwrap();
        let p1 = db.load_sync_state("p1").unwrap();
        assert_eq!(p1.len(), 2);
        assert_eq!(p1["a/b.txt"].size, 43);
        assert_eq!(p1["a/b.txt"].remote_signal, None);

        db.delete_sync_state("p1", "a/b.txt").unwrap();
        assert_eq!(db.load_sync_state("p1").unwrap().len(), 1);

        // p2 is untouched by p1 operations
        assert_eq!(db.load_sync_state("p2").unwrap().len(), 1);
        db.clear_pair("p2").unwrap();
        assert!(db.load_sync_state("p2").unwrap().is_empty());
    }

    #[test]
    fn snippets_crud_and_ordering() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.list_snippets().unwrap().is_empty());

        db.upsert_snippet("a", "List sites", "ls {{dir}}", Some("wp"))
            .unwrap();
        let list = db
            .upsert_snippet("b", "Restart nginx", "sudo systemctl restart nginx", None)
            .unwrap();
        assert_eq!(list.len(), 2);

        // Using `a` twice floats it above `b` — use_count is the primary sort key.
        db.touch_snippet("a").unwrap();
        let list = db.touch_snippet("a").unwrap();
        assert_eq!(list[0].id, "a");
        assert_eq!(list[0].use_count, 2);

        // Editing preserves use_count and created_ms, refreshes body.
        let created = list[0].created_ms;
        let list = db
            .upsert_snippet("a", "List a dir", "ls -la {{dir}}", Some("wp"))
            .unwrap();
        let a = list.iter().find(|s| s.id == "a").unwrap();
        assert_eq!(a.use_count, 2, "edit must not reset usage");
        assert_eq!(a.created_ms, created, "edit must not reset created_ms");
        assert_eq!(a.body, "ls -la {{dir}}");
        assert_eq!(a.name, "List a dir");

        let list = db.delete_snippet("b").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
    }

    #[test]
    fn open_creates_file_and_persists_across_reopen() {
        // Exercises the exact code path startup uses: Db::open on a real path,
        // WAL pragma, migrations, and durability across a reopen.
        let mut path = std::env::temp_dir();
        path.push(format!("faro_db_open_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

        {
            let db = Db::open(&path).unwrap();
            db.upsert_sync_state("p", "f.txt", 1, 2, None, 3).unwrap();
        }
        assert!(path.exists(), "Db::open must create the file");

        // Reopen: migrations don't re-run destructively and data survives.
        let db = Db::open(&path).unwrap();
        let rows = db.load_sync_state("p").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["f.txt"].size, 1);

        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn migrate_is_idempotent_across_opens() {
        // Running migrate twice on the same connection must not error or duplicate.
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }
}
