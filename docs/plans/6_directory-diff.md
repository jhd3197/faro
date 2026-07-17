# Plan 6 — Directory Diff (GUI + CLI + MCP)

## Context

A Meld/Beyond Compare for directory trees, across **any** Faro backend —
including **remote↔remote** (staging vs prod, two servers, two buckets), which
no local diff tool can do. It reuses the diff `sync.rs` already computes; the new
work is surfacing it three ways: the **GUI**, the **`faro-cli`**, and as an
**MCP/Agent-Bridge tool** so the AI can diff and reason about the result.

## Approach

### Diff engine — `src-tauri/src/diff.rs`
`sync::plan` already walks two `RemoteFs` sides and classifies
Missing/Newer/SizeChanged. Generalize that into a symmetric diff:
`{ onlyInA[], onlyInB[], different[] (size/mtime, optional content hash), same[] }`.
Optional `--hash` mode confirms "different" by content (server-side `sha256sum`
via exec / S3 ETag where available; falls back to streamed hashing). Files-only
first (as `walk` already is), directories implied.

### Three surfaces
1. **CLI** — `faro-cli diff <a> <b> [--hash] [--json]`. `<a>`/`<b>` are
   `profile:path` (or `local:path`), so **remote↔remote** works from the
   terminal. `--json` for scripting. Reuses `resolve_server`/profile resolution
   already in `faro-cli/src/main.rs`.
2. **MCP / Agent Bridge tool** — add `faro_diff` alongside the existing
   `faro_*` tools (`src-tauri/src/bridge.rs`), so an AI agent can call
   "diff these two trees" and get structured results to act on (e.g. "sync the
   3 files that differ"). This is the piece you asked for — diff available to
   the MCP.
3. **GUI** — a diff view built from `packages/file-ui`: two trees side by side,
   rows colour-coded (only-in-A, only-in-B, differ, same), filters, and
   per-row actions (copy A→B / B→A, reuse the transfer engine). Opens from a
   "Compare with…" action or as a workspace tab.

## Phases
1. `diff.rs` engine (symmetric classification, optional hash).
2. `faro-cli diff` + `--json` (fastest to ship, unlocks remote↔remote in the
   terminal).
3. `faro_diff` MCP/bridge tool.
4. GUI two-tree diff view + copy-across actions.

## Integration points
`src-tauri/src/diff.rs` (new; reuse `sync.rs::walk`, `RemoteFs`, exec-hash on
SSH/agent), `faro-cli/src/main.rs` (new `Diff` subcommand), `src-tauri/src/bridge.rs`
(register `faro_diff`), `packages/file-ui` (diff view) + `src/lib/ipc.ts`.

## Risks
- Cost of hashing large trees — make `--hash` opt-in; prefer server-side hashing.
- Object-store "directories" are prefixes — normalize paths before comparing.
- Symlinks skipped (as today) — note in output.

## Verification
`cargo check` + `tsc`; `faro-cli diff` local↔remote and **remote↔remote** (two
profiles) with/without `--hash`; the `faro_diff` tool returns structured output
to the bridge; GUI colour-coding + copy-across match the CLI result.
