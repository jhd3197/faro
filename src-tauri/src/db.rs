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

use anyhow::{Context, Result};
use rusqlite::Connection;

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
    fn migrate_is_idempotent_across_opens() {
        // Running migrate twice on the same connection must not error or duplicate.
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }
}
