# Plan 23 — Delta sync (block-level differential transfer)

> **Status: ✅ built (Phases 1–3, unit-tested) — 2026-08-10.** Engine property
> tests (24 in `faro-agent-proto`), daemon handler tests (18 in
> `faro-agentd`), app-side transfer tests over an in-process daemon (upload,
> download, both fallbacks, temp-litter assertions). Live two-machine run and
> the Phase 4 transfer-panel indicator remain open.

## Context

Every transfer used to be whole-file: a 1-byte edit to an 800 MiB file
re-sent 800 MiB. Delta sync transfers only the changed blocks, rsync-style,
for the **Faro Agent backend** (the only remote where we run code). All other
backends degrade to the whole-file path, unchanged. Covers folder-sync and
manual drag-and-drop transfers alike (same dispatch path), in both
directions.

## Approach

**Content-defined chunking, not fixed blocks + rolling hash.** FastCDC
(`fastcdc = "3"`, v2020) + BLAKE3 per chunk: identical content produces
identical chunk boundaries on both sides, so matching is a plain
`HashMap<blake3, offset>` lookup — no weak rolling hash at all (the
restic/Borg model). Chunk params: min 32 KiB / avg 256 KiB / max 1 MiB.

### Engine — `src-tauri/faro-agent-proto/src/delta.rs`

Lives in the proto crate so app and daemon run the identical algorithm and
constants. `signature_of_file` (single-pass chunk + whole-file BLAKE3),
`plan_delta` (upload: match local new file against the remote basis
signature, emit recipe + literal patch), `plan_download` (download: match
local basis chunks against the remote target signature, emit recipe +
coalesced missing ranges, ≤16 MiB each), `apply_delta` (range-validates every
recipe op against actual file lengths — a hostile recipe errors, never
panics — writes, fsyncs, BLAKE3-verifies, deletes the output on mismatch).
Gates: `should_attempt_delta` (≥ 8 MiB, basis exists, feature on;
`FARO_DELTA_MIN_SIZE` env override), `delta_worthwhile` (abort at ≥ 60%
literal), `params_match` (foreign chunk params ⇒ no delta).

### Wire protocol (additive, `PROTOCOL_VERSION` unchanged)

`Request::Signature{path}` (ungated read) → `Response::Signature{…chunks…}`;
`Request::DeltaAssemble{basis, patch, recipe, dest, expected_hash}`
(write-gated) → `Response::DeltaDone{bytes_reused, bytes_written}`. The
daemon assembles into `{dest}.faro-new-{pid}-{counter}` (agentd has no uuid
dep; pid+counter is unique enough per process), verifies, atomically renames
over `dest`, and best-effort deletes the patch.

**Old-daemon compatibility.** A pre-delta daemon can't deserialize the new
ops: its request loop treats the decode failure as a hang-up and tears the
channel down. The app's `AgentSession` transparently re-dials on a dead
channel, the failed `Signature` request surfaces as an ordinary error, and
the transfer falls back to whole-file — silent, one wasted round trip, every
subsequent request unaffected.

### Transfers — `src-tauri/src/transfer.rs`

`run_agent_upload_with_delta` / `run_agent_download_with_delta` are the
`Session::Agent` dispatch arms (`supports_delta(session)` pins the
Agent-only contract, exercised by a cross-backend table test). Each is a
thin wrapper over an `*_core` taking `Option<&AppHandle>` so tests skip
progress events. Upload: sign the remote basis → `plan_delta` locally → ship
the patch as `{remote}.faro-patch-{uuid}` via ordinary `WriteChunk`s →
`DeltaAssemble`. Download: sign the remote target → `plan_download` against
the local basis → fetch only missing ranges via ordinary `ReadChunk`s →
assemble into `.faro-delta-{uuid}.tmp` → verify → rename. Local patch temps
are `.faro-patch-{uuid}`. ANY delta error logs a warning and falls back to
the whole-file arm; `dest` is never touched until a hash-verified temp is
renamed over it (strictly safer than the old truncate-on-first-chunk
upload). `Transfer.delta: Option<DeltaStats>` (`{sent, reused}`,
skip-if-none) rides `transfer://done` so the UI can show savings.

### Setting + hygiene (Phase 3)

- `deltaSync` (bool, default on), plumbed exactly like `transferConcurrency`:
  Settings toggle → settings store (persisted to `faro.db`, pre-paint
  injected) → `transfer_set_delta_sync` command →
  `TransferManager::set_delta_enabled` (AtomicBool, live). Startup replays
  the persisted value. `FARO_DELTA=0` still force-disables regardless.
- Temp sweep: each folder-sync reconcile best-effort deletes
  `*.faro-patch-*` / `*.faro-new-*` / `*.faro-delta-*.tmp` entries older than
  24 h, recursively under the LOCAL sync root only (errors ignored,
  `tracing::debug` on failure) — covers crash-stranded litter; the remote
  side's temps are the daemon's own concern.

## Integration points

`faro-agent-proto/src/delta.rs` + `msg.rs` (new variants),
`faro-agentd/src/ops.rs` (`signature`, `delta_assemble` handlers),
`src-tauri/src/transfer.rs` (delta arms, `supports_delta`, `DeltaStats`,
`delta_enabled` switch), `src-tauri/src/commands.rs` +
`src-tauri/src/lib.rs` (`transfer_set_delta_sync`, startup replay),
`src-tauri/src/foldersync.rs` (reconcile-time sweep),
`src/stores/settingsStore.ts` / `src/lib/ipc.ts` /
`src/components/Settings.tsx` / `src/mock` (the `deltaSync` toggle).
No changes to `sync.rs` / `commands.rs` planning or `db.rs` — delta is a
transfer-time decision.

## Risks

- Wrong-file risk: none — final BLAKE3 verify + atomic rename mean failure ⇒
  fallback, never corruption.
- Old daemon: additive ops, no version bump; unknown op ⇒ channel teardown ⇒
  re-dial ⇒ whole-file fallback (verified by the reject-signature fallback
  test).
- Delta worse than copy: 60%-literal heuristic aborts before the patch
  crosses the wire.
- Temp litter: deterministic prefixes, deleted on success and handled
  failure; the reconcile sweep covers crashes.

## Verification

`cargo test -p faro-agent-proto` (24) + `cargo test -p faro-agentd` (18) +
`cargo test --lib` (183: engine round-trips over mutation classes, corrupt
recipes, hash-mismatch cleanup; daemon write-gating; in-process-daemon delta
up/download with <10% wire traffic on a 1 KiB edit of 20 MiB, both fallback
paths, no temp litter; `supports_delta` table; sweep keeps fresh/normal
files) + `npm run build` (tsc clean). Phase 4 (panel indicator, optional
signature cache) not started.
