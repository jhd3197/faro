# Plan 17 — Transfer queue depth: real queue, pause/resume, retry, throttle

## Context

The `TransferManager` (`src-tauri/src/transfer.rs`) is honest about being
minimal: every `start_*` spawns its task immediately — **`Queued` is just the
pre-start label, not a real queue** (no semaphore, no ordering). Per-transfer
control is exactly one verb: `cancel`. The frontend
(`src/stores/transfersStore.ts`, transfer panel) mirrors that: cancel +
clear-finished.

So today: enqueue 500 files → 500 concurrent transfers fighting each other; a
long upload blocks everything behind it; a flaky network = failed transfer, no
retry; a big transfer saturates the line with no way to cap it. This plan
turns the transfer list into an actual **queue** with the controls every file
client has.

## Scope

**In:**
- A real queue: **bounded concurrency** (active transfers capped; the rest wait
  as `Queued`), FIFO with manual priority (move up/down), pause-all/resume-all.
- **Pause/resume per transfer.**
- **Retry** — manual retry button + bounded auto-retry for transient errors.
- **Bandwidth throttle** — a global cap (KiB/s) applied across active transfers.

**Explicitly out:**
- Offset-resume of partially transferred files (resume re-runs the file; true
  seek-resume per backend is a later refinement — noted in Phase 2).
- Scheduling (start at 2am), per-connection queue profiles, cross-session
  queue persistence across restarts (queue state stays in-memory; a restart
  surfaces unfinished transfers as `Error`/`Canceled` like today).

## Approach

### Phase 1 — Real queue + bounded concurrency
- `TransferManager` gains a scheduler: a `tokio::sync::Semaphore` (default 3
  concurrent, per-session; `transferConcurrency` in the `settings` table) and
  an explicit FIFO of waiting ids. `Queued` now genuinely waits.
- Ordering ops: `transfer_move(id, up|down)` reorders the waiting list
  (active transfers are untouched).
- Pause-all/resume-all: a manager-level gate the scheduler checks before
  admitting the next waiting transfer.

### Phase 2 — Pause/resume per transfer
- A per-transfer **pause token** checked at every chunk boundary in the copy
  loops (the loops already iterate 64–128 KiB chunks — pause parks the task
  there; it does not abort mid-chunk).
- Semantics: **pause parks, resume re-runs the file from the start** with the
  existing `OverwritePolicy` (the partial local file is removed on pause; on
  the remote side a partial upload is truncated on re-run). Honest and correct
  on every backend, no per-backend seek support needed.
- New statuses: `Paused` (plus `Queued` already exists). `cancel` still works
  from any state.
- Later refinement (not this plan): offset-resume for SFTP (`seek`) and object
  stores (ranges) — the chunk-loop checkpoint is where that would hook in.

### Phase 3 — Retry
- Manual: a Retry action on `Error`/`Canceled` rows re-enqueues the transfer
  with its original source/dest/policy.
- Auto: errors classified by Plan 12's structured `FaroError.kind` —
  `Network`/`Timeout` auto-retry up to 2 times with backoff (5s, 20s);
  `Auth`/`NotFound`/`Permission` never auto-retry. The row shows
  "retrying in Ns (attempt 2/3)".

### Phase 4 — Bandwidth throttle
- A shared **token bucket** (global KiB/s cap, `transferThrottleKbps` setting,
  0 = unlimited) that every active copy loop draws from per chunk — the cap is
  split across active transfers, not per-transfer.
- Live-adjustable from the transfer panel header; changing it takes effect on
  the next chunk.

### Frontend
- Transfer panel rows: pause/resume + retry buttons, `Paused` and
  "retrying…" states; header: pause-all/resume-all, active/queued counts,
  throttle input. Queue rows show position; drag or move-up/down buttons for
  priority.
- Settings → Transfers: concurrency + throttle defaults.

## Integration points
- `src-tauri/src/transfer.rs` — scheduler/semaphore, pause token, statuses,
  retry loop, token bucket; all four `start_*` entry points route through the
  queue. The copy loops (download/upload, single + directory) get the
  pause-checkpoint + throttle-draw calls — one shared helper, not per-backend
  code.
- `src-tauri/src/commands.rs` — `transfer_pause/resume/retry/move`,
  `transfer_pause_all/resume_all`, throttle/concurrency settings (reuse the
  Plan 12 `settings` table).
- `src/stores/transfersStore.ts` + transfer panel component — new actions and
  row states (event payloads already carry `status`; add `position`,
  `retryAttempt`).
- `src-tauri/src/error.rs` — reuse `FaroError.kind` for retry classification.

## Risks
- **Checkpoint placement** — pause/throttle must live in the *shared* chunk
  loops, or 11 backends each grow their own copy. Directory transfers walk
  per-file: pause between files is fine-grained enough there.
- Pause-all vs per-transfer pause interplay — define precedence (a
  manager-gate AND a per-transfer token; resume requires both open).
- Throttle on tiny files — floor the bucket so a 1 KiB file doesn't wait a
  full token window.
- Event churn — progress events are already per-chunk; retry/pause add little,
  but batch position updates.

## Verification
- Unit: scheduler admits ≤ N concurrent; FIFO order; pause parks a fake
  transfer mid-stream and resume re-runs it; token bucket caps bytes/sec
  within tolerance.
- Headless mock harness (pattern of `scripts/verify-terminal.mjs`): enqueue a
  batch → only N active → pause one → others proceed → resume → retry a
  forced-failure row → throttle visibly slows throughput; all states render in
  the panel.
