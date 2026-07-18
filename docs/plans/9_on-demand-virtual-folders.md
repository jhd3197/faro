# Plan 9 — On-demand virtual folders (Plan 2, Phase 3)

## Status: WINDOWS PROVIDER BUILT (feature-flagged) — Explorer verification left

The Windows Cloud Filter provider is implemented behind the off-by-default
`virtualfs` cargo feature (`src-tauri/src/virtualfs/`). See
**[Build status](#build-status-implemented)** at the bottom for exactly what's
built, what was verified at runtime, and what still needs manual Explorer
testing. The original design (below) is unchanged and remains the reference.

Phases 1–2 of Plan 2 (continuous **real-file** one-way sync) are implemented
(`src-tauri/src/foldersync.rs`). This document is the implementation-ready
design for **Phase 3 — OneDrive-style on-demand placeholders** ("files show in
the folder, download on open, free-up-space to evict"). True on-demand is per-OS
*native filesystem-provider* code, and shipping unverified provider code risks
leaving orphaned OS sync-roots on a user's disk — which is why the whole thing
is gated behind `--features virtualfs` (the default build never compiles it) and
why the register→unregister orphan-cleanup path is the first thing verified.

## Why it's a separate tier

The engine in Phases 1–2 moves whole files on a schedule/trigger. On-demand
inverts control: the **OS filesystem** drives us — it renders placeholder icons,
and calls *our provider back* the instant a user (or any app) touches a file,
expecting us to stream the bytes ("hydrate") synchronously. That callback
surface is native and per-platform; it cannot sit on the cross-platform
`RemoteFs` trait, though it *consumes* it.

| OS | Mechanism | Rust surface |
|----|-----------|--------------|
| **Windows 10 1709+** | Cloud Filter API (`cldapi.dll`) | `windows` crate `Win32::Storage::CloudFilters` (all functions bound) or the `cloud-filter` crate |
| **macOS 11+** | File Provider (`NSFileProviderReplicatedExtension`) | a separate **app-extension bundle** (Swift/ObjC) — Tauri can't emit this today; needs a sidecar Xcode target |
| **Linux** | FUSE (no native placeholder API) | `fuser` crate; a mounted FS, not in-place placeholders |

Windows is both the highest-demand and the most tractable (single process, no
separate bundle), so build it first and gate the others behind it.

## Windows provider — exact call sequence

New `src-tauri/src/virtualfs/windows.rs`, `#[cfg(windows)]`, behind a
`virtualfs` cargo feature. Add `windows = { version = "0.58", features = [
"Win32_Storage_CloudFilters", "Win32_Foundation", "Win32_System_Com" ] }`
under `[target.'cfg(windows)'.dependencies]`.

1. **Register** the sync root once per on-demand pair:
   `CfRegisterSyncRoot(local_root, &CF_SYNC_REGISTRATION, &CF_SYNC_POLICIES,
   CF_REGISTER_FLAG_UPDATE)` — provider name/version + `HydrationPolicy`
   (`Full`/`Progressive`), `PopulationPolicy = Full` (we enumerate eagerly) or
   `AlwaysFull`. Persist that the root is registered so we `CfUpdateSyncRoot` /
   `CfUnregisterSyncRoot` cleanly on removal (this is the orphan-risk to get
   right).
2. **Connect** the callback table: `CfConnectSyncRoot(local_root, &callbacks,
   ..., CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO)` → keep the returned
   `CF_CONNECTION_KEY` alive for the pair's lifetime; `CfDisconnectSyncRoot` on
   stop.
3. **Populate placeholders** from a remote `RemoteFs::list_dir` walk:
   `CfCreatePlaceholders(dir, &mut [CF_PLACEHOLDER_CREATE_INFO], ...)` — one
   entry per remote file, size + mtime from the `DirEntry`, `FileIdentity` = the
   remote path (our key), `CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION`
   off so subdirs populate lazily.
4. **Serve callbacks** (`CF_CALLBACK_REGISTRATION` table):
   - `CF_CALLBACK_TYPE_FETCH_DATA` → **hydration**: read the requested byte range
     from the backend (this is exactly the existing `transfer.rs` download path /
     the Agent `ReadChunk`), and feed it back with `CfExecute(CF_OPERATION_TYPE_TRANSFER_DATA, ...)`
     in ≤ chunk-sized pieces, reporting progress via `CfReportProviderProgress`.
   - `CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS` → lazy directory enumeration: list the
     remote subdir and `CfCreatePlaceholders` on demand.
   - `CF_CALLBACK_TYPE_CANCEL_FETCH_DATA` → abort an in-flight hydration.
5. **Dehydrate / "free up space"**: `CfDehydratePlaceholder` (per file) or the
   user's Explorer "Free up space" — turns a hydrated file back into a
   placeholder, reclaiming disk while it still shows in the folder. Surface a
   per-pair "free up space" action in the Sync panel that calls this over the
   tree.
6. **Status badges** (the cloud/check overlay) are rendered by the OS from
   placeholder state — no custom shell overlay handler needed, *if* we also
   register a Storage Provider via the Shell (`IStorageProviderStatusUI`,
   optional polish).

## How it hooks the existing engine

The Phase 1–2 engine already has everything the provider needs to *pull* bytes:
- Placeholder population reuses the `RemoteFs::list_dir` walk (`sync::plan`'s
  `walk`), fed a `RemoteToLocal` pair's `remote_root`.
- `FETCH_DATA` hydration reuses the per-backend download streaming in
  `transfer.rs` (SFTP 64 KiB loop / Agent `ReadChunk` / object `get` range).
- A new `SyncPair.mode: Mirror | OnDemand` field selects, per pair, whether the
  engine downloads eagerly (today) or only registers placeholders and hydrates
  on access. `foldersync.rs` gains a `#[cfg(windows)]` branch that, for
  `OnDemand` pairs, drives the provider instead of `execute_sync_plan`.
- Remote→local propagation of *new* remote files still uses the poller: on each
  tick, diff remote listing vs current placeholders and `CfCreatePlaceholders`
  the additions / delete removals.

## Fallback if provider work is deferred: virtual mount (Tier 2)

If native placeholders prove too costly, a `WinFsp` (Windows) / `fuser` (Linux/
macOS) **mount** backed by `RemoteFs` gives "nothing on disk until read" with far
less surface — but it's a *mounted drive*, not files in an existing folder, and
requires the user install the WinFsp/FUSE driver. rclone's `mount` is the
reference. Different UX; keep as a documented alternative, not the default.

## Risks
- **Orphaned sync roots**: registration outlives the app if we crash without
  `CfUnregisterSyncRoot`. Persist registered roots and reconcile on startup.
- Hydration callbacks are latency-sensitive and run on OS threads — must not
  block on a cold session; pre-warm the pair's connection.
- macOS File Provider needs a separate signed app-extension bundle — a Tauri
  packaging gap; scope it only after Windows proves the model.
- Security: the Cloud Files minifilter has had privilege-escalation CVEs; keep
  the `windows` crate current and validate all callback-supplied paths against
  the pair's root.

## Recommended sequencing
Ship Phases 1–2 (done). Build the **Windows Cloud Filter provider** as a
dedicated feature-flagged effort with manual Explorer-driven verification
(hydrate on open, free-up-space, badge states). Only then evaluate macOS File
Provider and the FUSE fallback.

---

## Build status (implemented)

Windows provider built behind the **off-by-default `virtualfs` cargo feature**
(`cargo build -p faro --features virtualfs`). The default build never compiles
any of it — `src-tauri/src/virtualfs/unsupported.rs` is an inert stub, so an
on-demand pair configured on a normal build degrades to a clear "not available".

**What's built**
- `src-tauri/src/virtualfs/mod.rs` — the cross-platform `VirtualFs` subsystem
  (agent_host.rs shape): persisted registered roots (`virtualfs.json`),
  **orphan-safe `reconcile`** (unregister roots whose pair no longer exists,
  start the live ones — pure `plan_reconcile`, unit-tested), a `Hydrator`
  abstraction, and `virtualfs_supported / _status / _free_up_space` commands.
- `SyncPair.mode: Mirror | OnDemand` on folder sync; on-demand pairs skip the
  eager reconcile loop and are driven by `VirtualFs` (every foldersync mutation
  → `reconcile_virtualfs`, centralizing orphan cleanup).
- `src-tauri/src/virtualfs/windows.rs` — the provider, built on the safe
  **`cloud-filter` 0.0.6** wrapper for connect + placeholders + hydration
  callbacks. `fetch_data` hydrates by downloading through the shared
  `TransferManager` path (so every backend works) and streaming to the OS in
  4 KiB-aligned chunks via a captured tokio `Handle` + `block_on` (callbacks run
  on cldapi OS threads). `fetch_placeholders` populates subdirs lazily;
  `dehydrate` allows "free up space"; delete/rename stay local (live view, never
  mutates the backend). Callback paths are validated against the pair's remote
  root.
- Frontend: a Mirror | On-demand toggle in the sync-pair form (shown only where
  `virtualFsSupported`), a cloud badge + Free-up-space action per on-demand pair.

**Key finding (why registration is Win32, not the wrapper's default).**
`cloud-filter` registers sync roots through the **WinRT
`StorageProviderSyncRootManager`, which requires package identity**. Unpackaged
Faro (Tauri/NSIS) doesn't have it, so that `Register` returned `Ok` but
registered nothing (`GetSyncRootInformationForId` → `NOT_FOUND`; a runtime test
caught this). Fix: register/unregister via the **Win32 `CfRegisterSyncRoot` /
`CfUnregisterSyncRoot`** (the API this plan specified — no package identity
needed), keyed by path, and use `cloud-filter` only for the callback machinery
(its `Session::connect` already uses the path-based `CfConnectSyncRoot`, so it
composes). If Faro ever ships as MSIX/sparse-package, the WinRT path becomes
available too.

**Verified at runtime**
- The **register → `CfGetSyncRootInfoByPath` → unregister** round-trip passes on
  real `cldapi` from an unpackaged test process — the orphan-safety primitive
  `reconcile` depends on (`sync_root_register_unregister_round_trips`). This also
  proves the feature-on build links `cldapi.lib` and the FFI executes.
- `cargo check -p faro` (default) and `--features virtualfs` both clean;
  `cargo test -p faro --lib` 85 pass; `npx tsc --noEmit` clean.

**Left for manual Explorer verification (the maintainer's step)**
Build `--features virtualfs`, create an on-demand pair against a live backend,
and confirm in Explorer: placeholders appear, opening a file hydrates it,
right-click → "Free up space" (or the pair's button) dehydrates, and the
cloud/check badges render. Follow-ups: a poll to re-populate top-level
placeholders when the remote gains/loses files after start; ranged (vs
whole-file) hydration; macOS File Provider + FUSE fallback (Tier 2, unbuilt).
