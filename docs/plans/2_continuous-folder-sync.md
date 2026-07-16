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

**State index → cut out into [Plan 3](3_scan-index-foundation.md).** The
piece that turns *copy* into *sync* — a persisted per-file index — is **shared
infrastructure** (Plan 4's scan cache, Plan 6's hash cache, and Plan 7's search
index want the same `faro.db`), so it was moved out of this plan into the
foundation plan. Phases 1–2 shipped **without** it, on live two-sided diff
(`SyncReason` Missing/Newer/SizeChanged) — correct for one-way, but blind to
same-size edits and unable to distinguish a remote delete from never-existed.
Plan 3 adds the index (and the `change_signal` capability below); **bidirectional
sync depends on it.**

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
  **Added in [Plan 3](3_scan-index-foundation.md)** as `change_signal` +
  an optional `DirEntry.etag`.
- *has real directories* / *atomic rename*: object stores fake dirs (key
  prefixes) and have no atomic rename (copy+delete). `object.rs` already fakes
  folders; the engine must treat folderness/rename as flags, not givens.

---

## Phases

### Phase 1 — Continuous one-way, real files (LocalToRemote) — ✅ shipped
Watcher + poller + reconciler on top of `sync.rs`, diffing the two **live**
sides (the persistent index is [Plan 3](3_scan-index-foundation.md), not
here). Ship **Additive** first (copy new/changed up), then **Mirror** (also
delete remote files gone locally). Per-pair status + a StatusBar pill. This alone
delivers "attach a folder → edits auto-push to S3 / Azure / SFTP / the phone."

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
the state index ([Plan 3](3_scan-index-foundation.md) builds it) plus a
conflict policy (newest-wins / keep-both "conflicted copy" / prompt). Its own
future plan.

---

## Key files
- `src-tauri/src/foldersync.rs` (the shipped engine — pairs, watcher, poller,
  reconciler; JSON config `foldersync.json`). Mirrors `agent_host.rs`; Tauri
  commands `foldersync_*` registered in `lib.rs`.
- `src-tauri/src/sync.rs` (one-shot `plan()`/`execute()`, reused as primitives)
- `src-tauri/src/transfer.rs` (chunked transfer)
- **Persistence + `change_signal`: [Plan 3](3_scan-index-foundation.md)** —
  the state index (`faro.db`) and the `Capabilities.change_signal` hint live
  there, not here.
- frontend: a Sync panel in `Settings.tsx` + a StatusBar pill

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
