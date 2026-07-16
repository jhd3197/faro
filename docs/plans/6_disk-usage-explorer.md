# Plan 6 — Disk Usage Explorer (WinDirStat / WizTree, for any backend)

## Context

WinDirStat and WizTree answer "what's eating my disk?" with a **treemap** +
a size-sorted tree. They're local-only (WizTree is fast because it reads the
NTFS MFT directly). Faro can do something they can't: the **same analysis over
any connection** — an SFTP server, an S3 bucket, an FTP host, or a paired Faro
Agent — because every backend already implements the same `RemoteFs` walk. "See
where the space went on a remote server (or an S3 bucket), visually" is the hook.

It opens as a **workspace tab**, exactly like the SSH terminal: a toolbar button
("Analyze disk usage") on the file browser spawns a *Disk Usage* tab that scans
the current directory on the active connection.

## Scope

**In:** recursive size scan over `RemoteFs`, a treemap + size-ranked list with
drill-down, progress + cancel, and per-backend fast paths so it's usable at
scale. **Out:** the MFT trick (NTFS-only; we go cross-backend). Local NTFS could
get an MFT fast path later, but the generic + exec paths cover everything first.

## Architecture

### The scan (backend) — `src-tauri/src/diskscan.rs`
A `ScanManager` (mirrors the `agent_host.rs` / `TransferManager` pattern:
`Arc` in `AppState`, running scans in a `Mutex<HashMap<scanId, …>>`, progress via
`diskscan://…` events, cancel via an abort handle). A scan produces a **tree of
aggregated sizes**: each node = { name, path, kind, ownSize, totalSize, children }.

Three scan strategies, picked by `Capabilities` + protocol (fastest available):

1. **Generic walk (always works).** Recurse `RemoteFs::list_dir` (the `walk` in
   `src-tauri/src/sync.rs` is the starting point), summing file sizes up the
   tree. Parallelize directory listings (bounded concurrency) since latency, not
   CPU, dominates on SFTP/FTP. Stream partial results so the treemap fills in.
2. **Shell fast path (SSH / servers — the killer feature).** When
   `capabilities.has_shell` (SSH) or a Faro Agent with exec allowed, run one
   command instead of thousands of round-trips:
   `du -ab <path>` (or `find <path> -printf '%s %p\n'`) and parse the output into
   the tree. Orders of magnitude faster than an SFTP walk on a big server —
   this is what makes "WinDirStat on a server" actually practical. Falls back to
   the generic walk if exec is disabled or `du` is missing.
   - SSH: run via the existing SSH exec channel (`src-tauri/src/session/…` /
     `terminal.rs`).
   - Faro Agent: the `Exec { command, timeoutMs, maxBytes }` request
     (`faro-agent-proto` msg.rs), gated by the daemon's `allowExec` policy.
3. **Object-store flat listing (S3 / Azure — naturally fast).** Object stores
   have no real directories; one flat listing under the prefix returns every key
   + size. Sum by synthetic path segments to build the tree — no recursion, one
   pass. `ObjectFs` (`src-tauri/src/remotefs/object.rs`) already lists objects.
   Instant "what's in this bucket / what's it costing" breakdown.

Tauri commands: `diskscan_start(sessionId, path) -> scanId`,
`diskscan_tree(scanId)` (snapshot for progressive render), `diskscan_cancel`,
`diskscan_status`. Registered in `src-tauri/src/lib.rs` like the other subsystems.

### The visualization (frontend)
A new **Disk Usage tab** in the workspace tab system, alongside the terminal
(mirror `terminalsStore` + the tab bar). Contents:
- **Treemap** — the centerpiece. Squarified treemap on a **Canvas** (not SVG —
  thousands of rects; canvas keeps it smooth), color-by-type (reuse the file-type
  palette / Material Icon Theme colours) or by depth. Hover → path + size;
  click → drill in; breadcrumb to go back up. Layout math via `d3-hierarchy`
  (MIT, tiny) or a hand-rolled squarify.
- **Size-ranked list** beside it — folders/files sorted by `totalSize` with
  percentage-of-parent bars, the "biggest offenders" view.
- **Progress + cancel** while scanning; the map fills in live from streamed
  partials.
- **Actions** — reveal in the file browser, delete (with confirm), re-scan.
- Store: `src/stores/diskScanStore.ts` (Zustand, modeled on `bridgeStore.ts`),
  wired through `src/lib/ipc.ts` wrappers + a `onDiskScanProgress` event.

### Entry point
An "Analyze disk usage" button in the file-browser toolbar
(`packages/file-ui`) that opens a Disk Usage tab for the current path + session.

## Phases

1. **Scan engine + generic walk** — `diskscan.rs`, the aggregated-size tree,
   progress/cancel, commands. Correct on every backend, no fast paths yet.
2. **Treemap tab + size list** — the Disk Usage workspace tab, canvas treemap,
   ranked list, drill-down, the toolbar button. This is the "wow".
3. **Fast paths** — shell `du`/`find` for SSH + Faro Agent (exec-gated), and
   object-store flat listing. Makes it usable on real servers/buckets. Show which
   path was used (and why the generic fallback, if exec was denied).
4. **Actions + polish** — delete/reveal from the map, re-scan, remember last
   scan per connection, colour-by options.

## Integration points
- `src-tauri/src/diskscan.rs` (new); `src-tauri/src/lib.rs` (module + `AppState`
  + commands + auto-nothing); reuse `sync.rs::walk`, `RemoteFs`, `Capabilities`,
  the SSH exec channel, `ObjectFs` listing, the Agent `Exec` request.
- `src/stores/diskScanStore.ts` (new), `src/lib/ipc.ts` (wrappers + event), a new
  Disk Usage tab component + a Canvas treemap component, the file-browser toolbar
  button in `packages/file-ui`.

## Risks
- **Latency on deep trees over SFTP/FTP** — the whole reason Phase 3 exists;
  Phase 1 must stream + cancel so a slow scan is never a hang. Bound concurrency
  so we don't exhaust the SFTP channel.
- **Exec availability** — `du` may be absent, or exec disabled (read-only agent,
  policy off). Always fall back to the generic walk; never hard-depend on exec.
- **Object-store listing cost/limits** — huge buckets paginate; stream and cap,
  and `log` when truncated (no silent partial totals).
- **Symlinks / hardlinks** — `du` and a naive walk can double-count or loop;
  skip symlinks (as `sync.rs::walk` already does) and note the caveat.
- **Treemap perf** — canvas + squarify; virtualize the ranked list.

## Verification
`cargo check` + `tsc` + `vite build`; then a real scan on each strategy:
a local folder (generic walk), an SSH server (confirm the `du` fast path fires
and matches `du` run manually), the paired phone agent (`/sdcard`), and an S3
bucket (flat-listing breakdown). Confirm progress streams, cancel works, and the
treemap drill-down + delete behave.
