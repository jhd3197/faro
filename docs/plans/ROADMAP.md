# Faro Roadmap

How the individual plans sequence, and where Faro sits in the wider
DeviceKit / ServerKit ecosystem. Each plan doc has the detail; this is the map
and the order.

## The ecosystem in one picture

Faro is the **client/hub**: a desktop app for reaching files and machines
(SFTP, FTP, S3, and Faro's own Noise-paired **agent**). Two sibling "kits"
manage fleets and hand off to Faro:

- **DeviceKit** — Android device fleet (Flask + React + Kotlin agent + extensions).
  **Connects to Faro at the agent-protocol level**: the Android agent embeds
  Faro's `faro-agentd`, so a phone appears in Faro as a first-class connection
  (browse/transfer/sync). *Done — shipped.*
- **ServerKit** — server control panel, the server-fleet analog (Flask + React +
  **Go** host agent + extensions). **Connects to Faro only via a `faro://`
  deep-link** today (`serverkit-faro` prefills Faro's New Connection with a
  site's SFTP details). It does **not** use Faro's agent protocol. *Shallow link
  only — see Track D.*
- Shared libraries: **Prompture** (multi-LLM) and **Tukuy** underpin the AI
  features across CachiBot / DeviceKit / ServerKit.

So Faro has two integration depths: a **deep** one (embed `faro-agentd` → the
machine is controllable/browsable in Faro; DeviceKit does this) and a **shallow**
one (`faro://` deep-link prefill; ServerKit does this).

---

## Plan build order (the file numbers ARE the order)

Read this instead of the roadmap if you just want "what's next." The plan files
are numbered in the order to build them; the Track sections below are the
thematic detail.

| # | Plan file | Status | Why here |
|---|-----------|--------|----------|
| 1 | `1_faro-agent-pairing-and-distribution` | ✅ shipped | The agent — foundation everything else leans on. |
| 2 | `2_continuous-folder-sync` | 🔄 Phases 1–2 + safety merged; runtime test left | The shipped sync engine. |
| 3 | `3_scan-index-foundation` | ✅ built (runtime test on a live backend left) | Shared scan engine + `faro.db`. Substrate for 4/6/7 and the sync state index. |
| 4 | `4_disk-usage-explorer` | ✅ built (GUI click-through on a live backend left) | First *visible*, read-only consumer of #3 — proves the foundation at low risk. |
| 5 | `5_additional-backends` | 🔄 Phases 0–2 + 4 shipped (S3 presets, GCS, WebDAV, HTTP) | More `RemoteFs` impls. Remaining: SMB (blocked on MSVC/libsmbclient), OAuth clouds. |
| 6 | `6_directory-diff` | ⬜ | Reuses #3's scan engine (two trees) + `change_signal`/`etag`. |
| 7 | `7_fleet-search` | ⬜ | Reuses #3's scan engine; later a `faro.db` filename index. |
| 8 | `8_fleet-skills` | ⬜ | AI-authored fleet automations over the bridge. Independent. |
| 9 | `9_on-demand-virtual-folders` | ⬜ | OneDrive-style placeholders (Plan 2 Phase 3). Large, per-OS, Windows-first. |
| 10 | `10_iconify-brand-icons` | ⬜ | Additive brand/protocol logos. Independent polish; do whenever. |
| 11 | `11_scoped-connection-sharing` | 🅿️ deferred | Blocked on a login/auth foundation. |

Cross-project **Track D** (ServerKit ↔ Faro) has no plan file — it's a
convergence effort tracked only in the Track section below.

---

## Track A — Folder sync (Plan 2)

Continuous "attach a local folder, keep it mirrored" for any backend.

- ✅ **Phase 1–2** — continuous one-way sync (both directions), watcher + poll
  reconciler reusing `sync::plan` + `execute_sync_plan`, Additive/Mirror,
  Settings panel + status pill. *Merged.*
- ✅ **Safety hardening** — exclude patterns + Mirror-delete guard. *Merged
  (`feat/foldersync-safety`).*
- ⬜ **Runtime verification**: drive a live sync (easiest: to a paired phone
  agent). Currently compile/type-verified only.
- ⬜ **Bidirectional + conflict resolution** — needs the state index (Track A2);
  own effort.
- ⬜ **Phase 3 → Plan 9** (on-demand placeholders) — see Track C.

The persistent state index + `change_signal` were **cut out of Plan 2 into
[Plan 3](3_scan-index-foundation.md) (Track A2)** — they're shared with disk
usage, diff, and search, not sync-private.

## Track A2 — Shared scan + index foundation (Plan 3)

The primitives Plans 2, 6, 7, and 8 all quietly need, extracted so they're built
**once**: a reusable **scan engine** (bounded-concurrency `RemoteFs` walk with
progress/cancel + generic / exec-fast-path / object-flat strategy selection) and
a shared **`faro.db`** (SQLite via `rusqlite` bundled, in `AppState`, tables
per-feature). Plus `Capabilities.change_signal` + `DirEntry.etag`. The first
consumer is the folder-sync **state index** (`sync_state`) — same-size-edit
detection, remote-delete vs never-existed, resume-without-re-upload, and the
prerequisite for bidirectional.

- ✅ **Phase 1** — `faro.db` + migrations in `AppState` (`db.rs`, rusqlite bundled).
- ✅ **Phase 2** — extract `sync.rs::walk` → `scan.rs`, bounded-concurrency, with
  progress/cancel hooks; `sync::plan` calls it.
- ✅ **Phase 3** — `change_signal` + `etag` across all backends (object stores
  report ETag; SFTP/FTP/local/agent report mtime+size).
- ✅ **Phase 4** — `sync_state` index live in the reconciler: `sync::plan_indexed`
  catches same-size edits (`SyncReason::Edited`), snapshots the source after each
  pass (seed + prune), resume is a no-op. Verified via a real-Db + real-files
  test; a live-backend runtime pass (phone agent / S3) is the remaining check.

Disk Usage (Track F), Diff (Track G), and Search (Track H) build on this — the
scan engine and `faro.db` become additive callers + tables, not new subsystems.

## Track B — More connection backends (Plan 5)

Widen the connection list; each is one `RemoteFs` impl and inherits browse /
transfer / sync for free.

- ✅ Already present (pre-existing): SFTP, FTP/FTPS, S3, **Azure Blob** (via
  `object_store`), Faro Agent, local.
- ✅ **Phase 0** — S3-compatible presets: AWS/R2/B2 + Wasabi, DO Spaces, MinIO,
  Storj, Hetzner, Scaleway, Oracle OCI, IBM COS, Supabase, and a generic
  self-hosted preset. No backend code. *Shipped.*
- ✅ **Phase 1** — native GCS: `object_store` `gcp` feature + one builder, same
  one-builder move that shipped Azure. *Shipped.*
- ✅ **Phase 2** — WebDAV (Nextcloud/ownCloud/Storage Box/generic): new
  `RemoteFs` impl, PROPFIND/GET/PUT/DELETE/MKCOL/MOVE, ETag change signal.
  Verified against a live wsgidav server. *Shipped.*
- ⬜ **Phase 3** — SMB/CIFS (NAS/Windows shares; Azure Files for free). Biggest
  single gap. **Blocked on the Windows dev box:** `pavao` links libsmbclient
  (Samba C lib, no MSVC build) and the pure-Rust `smb` crate is still immature —
  re-evaluate at build time, and it needs a live NAS to verify. NFS as stretch.
- ✅ **Phase 4** — read-only HTTP(S) source: nginx/Apache autoindex browse +
  nginx JSON + direct-URL mode, streaming GET reads, mutations refused. Verified
  against `python -m http.server`. *Shipped.*
- ⬜ **Phase 5** — OAuth clouds, easiest API first: Dropbox → OneDrive → Drive →
  Box. Hardest (OAuth + ID-vs-path for Drive/Box).

## Track C — On-demand virtual folders (Plan 9)

OneDrive-style placeholders: files show in the folder, download on open, free-up-
space to evict. Native, per-OS, **Windows-first** (Cloud Filter API).

- ⬜ Windows provider (Cloud Filter API via the `windows` crate) — its own
  feature-flagged effort; reuses the Track A engine for listing + hydration.
- ⬜ macOS File Provider extension, ⬜ Linux FUSE, or the WinFsp/FUSE virtual-mount
  fallback. *Designed in Plan 9; not built.*

## Track D — ServerKit ↔ Faro convergence (cross-project, greenfield)

Today ServerKit reaches Faro only through the `faro://` deep-link. The natural
next step mirrors DeviceKit: **let a ServerKit-managed host expose a Faro agent**
so servers become first-class, browsable/syncable Faro connections (not just a
prefilled SFTP handoff).

- ✅ Shallow: `serverkit-faro` `faro://connect` deep-link (prefill only, no
  credentials). *Shipped.*
- ⬜ **Deep: embed/expose `faro-agentd` on ServerKit hosts.** Two options:
  (a) ServerKit's Go agent speaks the Faro Noise protocol natively (port a
  minimal `faro-agent-proto` responder to Go), or (b) ServerKit installs the
  existing `faro-agentd` binary as a managed service (it already ships
  cross-platform binaries + a one-line installer). Option (b) is far cheaper and
  reuses everything.
- ⬜ With that in place, **Track A folder sync targets a ServerKit host** for
  free — the payoff of the shared `RemoteFs`/agent design compounds across the
  ecosystem.

Note: ServerKit's own roadmap has "remote app/site deployment through connected
agents" as its open item — Track D dovetails with it (Faro becomes the file/sync
surface for those agents).

---

## Track E — Brand & protocol logos (Plan 10)

Additive icon layer for recognizable brand marks (S3/Azure/SSH/WordPress) via
Iconify, bundled offline. Deliberately does **not** touch the file-type icons
(Material Icon Theme — more complete + has the extension mapping) or the lucide
UI icons.

- ✅ Files already use Material Icon Theme; UI uses lucide. *(shipped/present)*
- ⬜ **Phase 1–2** — Iconify offline foundation + protocol logos on the rail,
  connection list, and New-Connection picker (logo *plus* the colour monogram).
- ⬜ Phase 3 — tech badges elsewhere. ⬜ Phase 4 — (deferred) evaluate
  consolidating lucide UI icons onto Iconify.

## Track F — Disk Usage Explorer (Plan 4)

A WinDirStat/WizTree-style treemap + size-ranked tree, but over **any** backend
(SFTP/S3/FTP/Agent/local) — opens as a workspace tab like the SSH terminal. The
differentiator vs the desktop tools: it works on **remote servers and buckets**,
with a shell `du` fast path (SSH/Agent) and object-store flat listing so it's
actually fast at scale.

- ✅ **Phase 1–2** — a Canvas treemap + size-ranked list on top of the
  **Track A2 scan engine**, opening as a full-screen explorer overlay from the
  file-browser toolbar / a directory's context menu.
  ✅ Phase 3 — exec `find -printf` fast path (SSH + Faro Agent, exec-gated,
  falls back to the walk) + object-store flat listing; the chosen strategy shows
  as a header badge with a fallback note.
  ✅ Phase 4 — reveal / copy-path / delete (live tree prune) from the map +
  colour-by type/depth + rescan. "Remember last scan per connection" deferred
  (the entry point always carries the current dir — no `faro.db` table yet).
  ⬜ Remaining: GUI click-through on a live SSH/S3/agent backend (core scan
  verified at runtime against a real filesystem; app boots clean).

**Built on Track A2** (scan engine + `faro.db`). This is the first *visible*
consumer of the foundation and it's read-only, so it's the lowest-risk way to
prove the walk / exec / object-flat strategies on real backends.

## Track G — Directory Diff (Plan 6)
Meld/Beyond Compare for any two backends (incl. remote↔remote), surfaced in the
**GUI, `faro-cli`, and as an MCP `faro_diff` tool**. Reuses `sync.rs`'s diff +
the **Track A2** scan engine (walks two trees) and its `change_signal`/`etag`
for `--hash` mode. ⬜

## Track H — Fleet Search (Plan 7)
Name + content search across any connection; exec `rg`/`grep` fast path on
SSH/agent, walk fallback. Reuses the **Track A2** scan engine (and, later, a
`faro.db` filename index). GUI + CLI + `faro_search` MCP tool. ⬜

## Track I — Fleet Skills (Plan 8)
Reusable, parameterized, **AI-authorable** automations over the fleet, MCP-native
(the AI composes/saves Skills, then runs them across servers). Builds on the
bridge's existing saved-commands + `faro_exec` + approvals. Safety-gated. ⬜

## Track J — Scoped connection sharing (Plan 11) — DEFERRED
Share a box read-only / path-jailed / time-boxed. **Parked until a login/auth
foundation exists** — the real use case (remote employee, link-based) needs auth
regardless, so we hold the whole track rather than ship the LAN-only half. The
scoped-grant model remains the permission backbone once login lands. 🅿️

## Near-term quick wins (small, high-value)
- **Editable permissions dialog** — today Properties *shows* mode read-only; add
  a FileZilla-style chmod editor (rwx checkboxes + octal). Backend (`chmod_path`,
  `can_chmod`) already exists. Also the visual foundation for idea #10
  (permissions/security view).
- **CLI conflict policy** — expose `--overwrite` / `--on-conflict
  overwrite|skip|rename` on `faro-cli` single-file `upload`/`download`/`cp`
  (only `upload-dir` has it today; default is rename). Backend `OverwritePolicy`
  already supports all three.

## Recommended global sequence

The plan **file numbers now follow this execution order** (see the build-order
index at the top). The Track sections above are a *thematic* map — their letters
are stable labels, so a Track's `(Plan N)` no longer matches its position in the
alphabet. This list is the order to actually build in; the numbered files are its
mirror.

1. **Track A — runtime test.** Safety hardening is merged; drive a live sync
   (paired phone agent) so the shipped feature is trustworthy. (Small.)
2. **Track A2 — scan engine + `faro.db`.** Build the shared foundation: extract
   the walk into `scan.rs`, stand up SQLite in `AppState`, add
   `change_signal`/`etag`. Nothing user-facing yet — it's the substrate for the
   next three.
3. **Track F — Disk Usage (Plan 4).** The first *visible* consumer of the scan
   engine + DB, and read-only (zero risk to shipped sync). Proves the walk /
   exec / object-flat strategies on real backends before the sync engine takes a
   hard dependency on them. Best payoff-to-risk win on the board.
4. **Track A2 — state index live.** Now layer the `sync_state` index into the
   folder-sync reconciler on the foundation Disk Usage already exercised.
   Robustness + unlocks bidirectional.
5. **Track B Phase 0/1/2** — S3 presets + WebDAV + SMB. Biggest coverage gain,
   no OAuth, all fit the trait. Independent of the DB — can interleave anywhere.
6. **Tracks G + H — Diff + Search.** Same scan engine, new surfaces (GUI + CLI +
   MCP `faro_diff` / `faro_search`).
7. **Track D option (b)** — ServerKit installs `faro-agentd` as a managed
   service; every server becomes syncable via Track A.
8. **Track C Windows on-demand** and **Track B Phase 4 OAuth clouds** — the two
   large, later efforts.

To execute any of these tracks end-to-end in a fresh session, use the local
`docs/plans/prompt.md` runbook — set its one plan-filename knob and paste it.
