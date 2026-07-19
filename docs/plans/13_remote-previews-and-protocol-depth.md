# Plan 13 — Remote image previews & protocol depth

## Context

A grab-bag with one user-facing star. The star comes from a real user question
("how do I get my FTP client to show image previews?"): Faro only thumbnails
**local** images today, because remote thumbnails mean downloading. Naively
done this is a footgun — open `wp-content/uploads` with 100k images and the
app fires 100k requests just because you scrolled. The rest of the plan is
protocol depth Faro currently lacks, plus the test rig to prove it:

1. **Remote image previews, done lazily** — viewport-driven fetching with hard
   budgets, so a 100k-file folder costs exactly as many requests as rows you
   actually look at.
2. **Native drag-out download** — drag a remote file from Faro onto the
   desktop. HTML5 DnD can't do this from a webview; the `drag` crate stages to
   temp and hands real OS file handles.
3. **SCP fallback** — servers with the SFTP subsystem disabled (busybox boxes,
   locked-down hosts) get full browse/transfer via the SCP wire protocol.
4. **Port forwarding** — local/remote tunnels with persisted rules and DB
   presets (Postgres/MySQL/Redis/…). Table stakes for an SSH-adjacent tool;
   Faro has none.
5. **Docker SSH E2E fixtures** — the reason half the roadmap says
   "compile-verified only." Spin real SSH servers (bastion, SCP-only, sudo) in
   CI/dev and click the actual features.

## What already exists (don't rebuild)

- Local image thumbnails in `packages/file-ui` (`Thumbnail`) — the UI slot
  this plan feeds; the package is transport-agnostic, so the adapter just
  learns a new source.
- `TransferManager` (`transfer.rs`) — chunked downloads with cancel; preview
  fetches ride the same session pool, not new connections.
- `SessionManager` — pooled per-profile sessions; port-forward tunnels open
  dedicated connections by design, preview fetches reuse the pool.
- `faro.db` — port-forward rules + thumbnail cache index live here.
- `settingsStore` / `Settings.tsx` — the preview settings land here (and
  migrate to `faro.db` with Plan 12 Phase 2, whichever ships second adapts).

## Approach

### Phase 1 — Remote image previews (lazy, budgeted, opt-in)

The non-negotiable invariant: **scrolling must never trigger unbounded
requests.** Every mechanism below exists to protect that.

- **Settings:** `remoteImagePreviews: off | on` global (default **off** for
  remote backends, local behavior unchanged) + per-connection override
  ("Always / Never / Follow global"). A visible toggle in the file-browser
  toolbar too — this is the kind of setting people want to flip per session.
- **Viewport-driven fetching only:** thumbnails load via IntersectionObserver
  on the row (+ ~1 screen of overscan). No intersection, no request. Scrolling
  fast past a folder costs **zero** requests.
- **In-flight budget:** concurrency cap (e.g. 4) on preview fetches per
  connection; requests for rows that scrolled out of the overscan are
  **cancelled** before dispatch and aborted if in flight (CancellationToken).
- **Size guards:** skip files over a cap (default 25 MB, configurable); only
  known image extensions (png/jpg/jpeg/gif/webp/avif/bmp/ico); SVG rendered
  from bytes, never executed.
- **Backend command `preview_thumbnail`:** bounded read (cap bytes) → decode +
  downscale in Rust (`image` crate) → write to a cache dir → return the cache
  path (served via the asset protocol). Cache keyed by
  `(connection_id, path, size, mtime/etag)` so edits invalidate; index in
  `faro.db`, LRU-evicted with a configurable disk budget (default 256 MB).
- **Object stores** (S3/Azure/GCS/clouds) use ranged/GET with the same cap —
  no special casing beyond `change_signal` for cache keys.
- **Grid view** reuses the same pipeline; list view shows thumbs when enabled.

### Phase 2 — Native drag-out download
- New `transfer_stage_for_drag(paths)` command: downloads to a temp staging
  dir (reusing `TransferManager`, showing progress in the queue for big
  files), then the `drag` crate begins an OS drag with real file handles.
- Trigger: pointer-event-driven drag start on the file pane — **not** HTML5
  drag events, which Tauri's OS drop handler suppresses. Use pointer-driven
  detection for in-app DnD too if the two ever conflict.
- Staging dir cleanup on app exit + LRU.

### Phase 3 — SCP fallback
- Hand-rolled SCP wire protocol (`C`/`D`/`E`/`T` records + ack bytes) over
  generic `AsyncRead + AsyncWrite` so it's unit-testable via
  `tokio::io::duplex` — no server needed for the protocol tests.
- Session capability detection: if the SFTP subsystem fails, offer "try SCP
  mode" (banner + profile toggle); `RemoteFs` impl mirrors the SFTP surface so
  browse/transfer/sync inherit it.

### Phase 4 — Port forwarding
- `forward_rules` table in `faro.db`; per-tunnel dedicated SSH connection
  (never the pooled browsing session), `TcpListener` bind, CancellationToken
  cancel, live status events to a Forwarding panel.
- Local forwards first; remote forwards second. Presets:
  Postgres/MySQL/Redis/Mongo/HTTP/K8s. Auto-start toggle per rule.

### Phase 5 — Docker SSH E2E fixtures
- `tests/e2e/` with Docker SSH fixtures: an OpenSSH baseline,
  a **bastion** (ProxyJump), an **SCP-only busybox** server (proves Phase 3),
  a **sudo** server; WebdriverIO (or a headless `faro-cli` harness where UI
  isn't needed) driving real connect/browse/transfer against them.
- First consumers: SCP fallback, port forwarding, ProxyJump-through-forward —
  then grow toward the roadmap's standing "live-backend click-through" items.

## Integration points
- `packages/file-ui` (`Thumbnail`, row viewport hooks), `src/lib/fileUiAdapter.ts`,
  `transfer.rs` (bounded preview reads + staging), new `src-tauri/src/preview.rs`,
  `scp/`, `portforward.rs`, `db.rs` (two tables), `Settings.tsx`, file-browser
  toolbar, `tests/e2e/`.

## Risks
- **Preview cost regression** — the whole point; add a debug counter (requests
  fired while scrolling a large dir) and assert it stays ≤ viewport + overscan
  in the E2E rig. Default-off + caps are the second line of defense.
- **Cache poisoning by mtime collisions** — include size + etag/change_signal
  in the key, not mtime alone.
- **Drag staging doubles disk usage for huge files** — stage lazily per file,
  stream, and warn above a threshold.
- **SCP is a legacy protocol** — implement conservatively (no shell expansion:
  always `scp -f/-t` with quoted single paths), test against the busybox fixture.
- **Feature load** — five phases; each ships independently behind the normal
  `dev` flow. Phase 1 is the user-facing win; do it first.

## Verification
`cargo check -p faro` + `npx tsc --noEmit` clean. Runtime, per phase:
1. Point at a real `wp-content/uploads` (or a generated 50k-image dir):
   thumbs appear only for scrolled-to rows, request counter stays bounded,
   scrolling fast fires ~zero, reopening the folder hits the disk cache.
2. Drag a remote file to the desktop → file lands; big file shows queue
   progress.
3. Connect to the SCP-only fixture → full browse/transfer works.
4. Add a Postgres preset forward → `psql localhost:port` connects through.
5. `tests/e2e` green locally (Docker) against all fixtures.
