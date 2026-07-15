# Plan 2 — Continuous folder sync (one-way → on-demand virtual folders)

## Context

Faro can already do a **one-shot** directory sync (`src-tauri/src/sync.rs`:
`SyncPlan`/`plan()`/`execute()` over the `RemoteFs` trait). This plan turns that
into a **continuous, stateful, backend-agnostic sync engine**: attach a local
folder to a remote root, flip it on, and edits propagate automatically — for
**any** backend (S3, Azure Blob, SFTP, FTP, the Faro Agent phone) with zero
per-backend sync code, because the engine only ever calls the trait.

The end-goal vision is **OneDrive-style virtual folders**: files appear in a
normal folder as placeholders, download on open ("hydrate"), and can be freed
from disk ("dehydrate") while still showing in the folder with a status badge.
That is a real feature but a **large, per-OS native project** — so this plan is
**staged**: real-file continuous sync first (tractable, ships value), with the
on-demand layer as a clearly-flagged later phase.

### The three tiers (why staging matters)

| Tier | What the user sees | Where it lives | Effort |
|------|--------------------|----------------|--------|
| **1. Real-file sync** | Files truly on disk, auto-mirrored | The engine below, on `RemoteFs` | Weeks — this plan's Phase 1–2 |
| **2. Virtual mount** | Remote as a mounted drive; nothing local until read | Filesystem driver (WinFsp / FUSE) | Medium, but it's a *mount*, not files-in-Documents |
| **3. On-demand placeholders** | 0-byte placeholders + cloud badge; hydrate on open; "free up space" | Per-OS native provider (below) | Large, per-OS — Phase 3 |

The **engine is written once**; only Tier 3's OS hooks are platform-specific
(and per-OS, not per-backend). That is the whole payoff of the trait.

---

## Architecture (the shared engine — new `src-tauri/src/sync/` module)

**`SyncPair`** (persisted config; new store alongside profiles):
`{ id, local_root, profile_id (→ the connection/backend), remote_root,
direction, strategy, enabled, poll_interval, conflict_policy }`.
Reuses the existing `SyncDirection` (LocalToRemote/RemoteToLocal) and
`SyncStrategy` (Additive/Mirror) enums from `sync.rs`.

**State index** — the piece that turns *copy* into *sync*. A local **SQLite**
DB (one row per file per pair): `sync_state(pair_id, rel_path, local_mtime,
local_size, local_hash?, remote_signal, last_synced_rev, state)`. Without this
the engine can't tell "deleted on the remote" from "never existed," nor detect
that nothing changed since last run. Today's `SyncReason`
(Missing/Newer/SizeChanged) compares the two *live* sides — fine for one-shot,
insufficient for continuous. The index is the engine's memory.

**Local watcher** — the [`notify`](https://docs.rs/notify) crate
(inotify / FSEvents / ReadDirectoryChangesW), debounced, marks dirty rel-paths.

**Remote poller** — interval `list_dir` walk diffed against the state index.
**Critical constraint:** S3, Azure Blob, SFTP, FTP, and the Faro Agent have
**no change feed** — remote change detection is *polling*, full stop. (Ironically
the future Dropbox/Drive backends *do* have delta cursors, so those can skip
polling — model push/delta as an optional per-backend capability, not the norm.)

**Reconciler** — merges local events + remote poll against the state index to
emit actions (upload / download / delete / skip). Reuses `sync.rs`
`plan()`/`execute()` as primitives. **One-way keeps this simple:** the source
side is authoritative, so a two-sided change collapses to "source wins" — almost
no conflict logic (that is deferred; see below).

**Executor** — chunked read/write via `RemoteFs` + the existing `transfer.rs`
engine (128 KiB chunks, resumable offsets).

**Status surface** — per-pair state (idle/scanning/syncing/error), pending
count, per-file state; a tray/menu "mini sync" indicator (the thing you asked
about). Emits Tauri events like the agent host does.

**Capabilities the engine queries** (via the existing `Capabilities` struct):
- *change signal*: cheapest reliable "did this file change" — S3 ETag,
  mtime+size, or a content hash (backends differ; ask, don't assume).
- *has real directories* / *atomic rename*: object stores fake dirs (key
  prefixes) and have no atomic rename (copy+delete). `object.rs` already fakes
  folders; the engine must treat folderness/rename as flags, not givens.

---

## Phases

### Phase 1 — Continuous one-way, real files (LocalToRemote)
Watcher + poller + SQLite state index + reconciler on top of `sync.rs`. Ship
**Additive** first (copy new/changed up), then **Mirror** (also delete remote
files gone locally). Per-pair status + tray indicator. This alone delivers
"attach a folder → edits auto-push to S3 / Azure / SFTP / the phone."

### Phase 2 — Continuous one-way, RemoteToLocal
Same engine, reversed authority (remote is source → download mirror). Enables
"pull a bucket/prefix down and keep it fresh."

### Phase 3 — On-demand / virtual folders (the OneDrive-style tier) — LARGE, per-OS
Layered above the engine; the engine feeds it "what exists remotely" and
services hydration reads. Platform providers:
- **Windows:** Cloud Filter API (`cldapi`) — register a sync root, place
  placeholders, service `FETCH_DATA` callbacks to hydrate; "free up space"
  dehydrates; the OS renders the cloud/check badge natively.
- **macOS:** `NSFileProviderReplicatedExtension`.
- **Linux:** FUSE mount (no native placeholder API).

Alternative/simpler UX if placeholders prove too heavy: **Tier 2 virtual mount**
via WinFsp/FUSE (whole mounted drive backed by the remote, rclone-style) —
different UX (a mount, not files in Documents) but far less native surface.
Flag this as its own sub-decision when we reach it.

### Deferred — bidirectional + conflict resolution
Out of scope here, matching the existing note at `sync.rs:5` ("Bidirectional …
needs conflict resolution UI that's a project of its own"). Bidirectional needs
the state index (Phase 1 builds it) plus a conflict policy (newest-wins /
keep-both "conflicted copy" / prompt). Its own future plan.

---

## Key files
- `src-tauri/src/sync.rs` (existing one-shot engine → refactor into `sync/`)
- new `src-tauri/src/sync/{engine.rs,index.rs,watcher.rs,poller.rs,pairs.rs}`
- `src-tauri/src/transfer.rs` (reuse chunked transfer)
- `src-tauri/src/remotefs/mod.rs` (`Capabilities` — add a `change_signal` hint)
- new SQLite store + a `sync-pairs.json` config; Tauri commands mirroring
  `agent_host.rs` (`sync_pair_list/add/remove/set_enabled/status`)
- frontend: a Sync panel + tray status

## Risks / gotchas
- **No remote change feed** on S3/Azure/SFTP/FTP/Agent → polling cost, latency,
  and (S3) per-`LIST` billing. Make interval configurable; consider push/delta
  only where a backend offers it.
- **Object-store semantics**: faked dirs, no atomic rename, multipart ETag ≠
  MD5 (breaks naive hash compares) — lean on `change_signal` capability.
- Large trees: poll walks get expensive → incremental/segmented scans.
- Partial writes / editors' atomic-save temp files → debounce + settle window.
- Cross-OS case sensitivity, symlinks, path length.
- **Phase 3 is a multi-month per-OS native effort** — keep Phases 1–2 shippable
  and independently valuable so on-demand can be a later commitment, not a
  blocker.

## Verification
- Attach a folder to the **Faro Agent phone** (already working) or a test S3
  bucket; edit a local file → observe upload; add a remote file → poller pulls
  it; delete under each strategy; kill & relaunch → state index resumes with **no
  full re-upload**. Then repeat against a second backend (Azure/SFTP) unchanged,
  proving the engine is backend-agnostic.
