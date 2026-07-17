# Plan 3 — Shared scan engine + connection index (the `faro.db` foundation)

## Context

**Cut out of Plan 2.** The continuous-sync engine (Plan 2, Phases 1–2) shipped
*without* persistent per-file memory: it diffs the two **live** sides via
`sync::plan` (size + mtime). That is correct for one-shot copy but blind to
same-size edits and unable to tell "deleted on the remote" from "never existed."
The fix — a persisted per-file index — turns out to be the first instance of a
primitive that **four** plans want:

- **Plan 2** — per-file sync state: distinguishes remote deletes, detects
  same-size edits, resumes without a full re-upload, and is the prerequisite for
  bidirectional.
- **Plan 4 (Disk Usage)** — "remember last scan per connection": a cached
  per-file size tree.
- **Plan 6 (Diff)** — cache content hashes so repeat diffs don't re-hash.
- **Plan 7 (Search)** — a filename index (later, SQLite FTS).

They are the same row shape — `(connection_id, path, size, mtime, etag/hash,
seen_at)` keyed by connection — and they are all fed by the same operation: a
**recursive `RemoteFs` walk**. That walk *also* already exists three times across
the plans (Plan 2's poller, Plan 4's `ScanManager`, Plan 7's fallback), each
re-specifying the same three strategies. This plan extracts both shared pieces so
the later plans become **additive tables and new callers**, not new databases and
re-implemented walks. It ships no new user-facing surface of its own beyond
making sync smarter — it is foundation.

## Scope

**In:** (1) a reusable **scan engine** — bounded-concurrency recursive walk with
progress + cancel and per-backend strategy selection; (2) a shared **SQLite
`faro.db`** handle in `AppState` with migrations, tables per-feature; (3)
`change_signal` on `Capabilities` + an optional `etag` on `DirEntry`; (4) the
**sync state index** (`sync_state` table) wired into the folder-sync reconciler
as the first consumer.

**Out:** the disk-usage tree/treemap (Plan 4), diff/search surfaces (Plans 7/8),
and bidirectional sync (deferred — but this unlocks it).

## Architecture

### Scan engine — `src-tauri/src/scan.rs` (extract from `sync.rs::walk`)
One place that walks a `RemoteFs` tree, picking the fastest strategy available
from `Capabilities` + protocol:

1. **Generic walk (always works).** Recurse `RemoteFs::list_dir`, bounded
   concurrency (latency, not CPU, dominates on SFTP/FTP), streaming partial
   results.
2. **Exec fast path (SSH / exec-allowed agent).** One command instead of
   thousands of round-trips — the caller supplies the command shape (`du -ab` /
   `find -printf` for sizes, `rg`/`grep` for search). Falls back to the generic
   walk if exec is disabled or the tool is missing.
3. **Object-store flat listing (S3 / Azure).** One flat listing under the prefix;
   build the tree from key segments. No recursion.

Progress + cancel mirror `TransferManager` / `agent_host.rs` (`Arc` in
`AppState`, abort handle, `scan://…` events). Plan 2's poller, Plan 4's disk
scan, Plan 6's diff, and Plan 7's search all call **this** instead of rolling
their own.

### `faro.db` — shared SQLite in `AppState`
`rusqlite` with the **bundled** feature (compiles SQLite from source — no system
SQLite dependency), one `faro.db` under `app.path().app_data_dir()`. A tiny
`db.rs` opens the connection and runs forward-only, versioned migrations at
startup; every subsystem gets tables in the **same** file:

- `sync_state(pair_id, rel_path, size, mtime, remote_signal, last_synced_ms, state)`
  — this plan.
- (later) `scan_cache`, `search_index`, `diff_hash` — Plans 6 / 8 / 7, additive.

Shared **connection + migrations**; **per-feature tables**. No grand unified
schema — the win is not spawning four private DBs, not a universal index.

### `Capabilities.change_signal` + `DirEntry.etag`
The cheapest reliable "did this file change" differs per backend: object stores
have ETags, POSIX has mtime+size, some need a content hash. Add a `change_signal`
hint to `Capabilities` (`MtimeSize` / `Etag` / `Hash`) and an optional
`etag: Option<String>` on `DirEntry` so object stores stop guessing from mtime.
Every backend's `capabilities()` (`sftp.rs`, `ftp.rs`, `object.rs`, `local.rs`,
`agent.rs`) declares it honestly. Consumed by Plan 2 (reconcile), Plan 5 (each
new backend declares it), and Plan 6 (`--hash` diff).

### State index wired into the reconciler
`foldersync.rs`'s reconcile consults + updates `sync_state`: detect same-size
local edits (mtime/etag changed, size did not), distinguish a remote deletion
(was in the index, gone from the listing) from never-existed, and skip when
nothing changed since `last_synced_ms`. Kill & relaunch resumes with **no full
re-upload**. The index is an optimization layered over — never a replacement for
— a real listing; startup always reconciles against reality.

## Phases

1. **`faro.db` + `db.rs`** — `rusqlite` bundled, `AppState` handle, migration
   runner, `sync_state` schema. No behavior change yet.
2. **Scan engine extraction** — pull `sync.rs::walk` into `scan.rs` with
   progress / cancel / bounded concurrency + a strategy hook. The folder-sync
   poller switches to it (no behavior change — just the shared path).
3. **`change_signal` + `etag`** — extend `Capabilities` + `DirEntry`; every
   backend declares honestly.
4. **State index live** — the reconciler reads/writes `sync_state`:
   same-size-edit detection, remote-delete vs never-existed, resume without
   re-upload. This is the payoff.

## Key files
- new `src-tauri/src/scan.rs` (extract from `sync.rs::walk`),
  `src-tauri/src/db.rs` (faro.db + migrations)
- `src-tauri/Cargo.toml` — `rusqlite = { version = "0.32", features = ["bundled"] }`
- `src-tauri/src/remotefs/mod.rs` (`Capabilities.change_signal`, `DirEntry.etag`)
  + every backend's `capabilities()`
- `src-tauri/src/foldersync.rs` (reconcile against `sync_state`)
- `src-tauri/src/lib.rs` (`AppState.db` field + setup construction)

## Risks
- **Schema churn** — keep migrations forward-only and versioned from day one
  (a `user_version` pragma), so Plans 6/7/8 add tables without a rewrite.
- **Bundled SQLite build cost** — `rusqlite`'s `bundled` feature compiles SQLite
  from source (one-time; fine on the Windows MSVC toolchain — see the dlltool
  note in the runbook).
- **ETag ≠ MD5** on multipart S3 uploads — `change_signal = Etag` must treat the
  ETag as an opaque change *token*, not a content hash.
- **Index/reality drift** — always reconcile against a real listing on startup;
  the index accelerates, it never becomes the source of truth.

## Verification
`cargo check -p faro` + `npx tsc --noEmit`; then runtime: attach a folder to the
paired phone agent (or a test S3 bucket), edit a file **without changing its
size** → confirm it re-syncs (proves the index; today's mtime-only path may miss
it); delete a remote file → the poller sees "was indexed, now gone" and mirrors
the delete; kill & relaunch → **no full re-upload**. Then confirm the extracted
scan engine still drives the existing poller unchanged on a second backend
(Azure/SFTP), proving the extraction was behavior-preserving.
