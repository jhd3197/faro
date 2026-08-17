# Plan 22 — Find Duplicates & cleanup (dedupe)

> **Status: ✅ built (engine + CLI + MCP tool + GUI) — 2026-08-09.** Engine
> unit-tested (6 tests); `faro-cli dedupe` runtime-verified local (name + hash
> modes, `--json`, `--delete --yes`). Live-backend run + GUI click-through
> still open.

## Context

Faro's rename-on-conflict policy (`OverwritePolicy::Rename`) appends `_1`,
`_2`, … to colliding file names on upload/download — so real-world trees
accumulate `photo_1.jpg`-style copies with no way to see or clean them. This
adds a duplicate-file finder over **any** Faro backend: scan one tree, review
the alike-file groups, delete the extras. Reuses the Plan 3 scan engine (walk)
and Plan 6's content-hash machinery.

## Approach

### Engine — `src-tauri/src/dedupe.rs`
Two modes over one `scan::walk` of the tree:

- **Name mode** (default, metadata-only — no byte transfer): files in the
  *same directory* whose names collapse to the same stem once copy suffixes
  are stripped (`_N`, ` (N)`, ` - copy`, incl. chained ` - copy 2`) **and**
  whose sizes match → a duplicate group.
- **Hash mode** (opt-in): bucket by size, hash every multi-file bucket via
  `diff::hash_path` (server-side `sha256sum` over SSH, streamed sha256 for
  local/object/FTP), group by digest. Catches duplicates with unrelated names
  anywhere in the tree.

Every group carries a suggested `keep` index (unsuffixed name → oldest copy →
smallest path). **The engine never deletes** — surfaces get groups +
`duplicate_paths()` and issue explicit deletes.

### Surfaces
1. **GUI** — "Find duplicates" on a folder's context menu / pane toolbar opens
   an overlay (`src/components/FindDuplicates.tsx` + `src/stores/dedupeStore.ts`,
   shaped like the Directory Diff view): cancellable scan with progress, groups
   sorted by reclaimable bytes, per-file checkboxes (all non-keepers checked by
   default, keeper badged), confirm-then-delete via `dedupe_delete`, reveal /
   copy-path row actions.
2. **CLI** — `faro-cli dedupe <target> [--hash] [--json] [--delete --yes]`;
   `profile:path` / `local:path` / plain path, so remote trees work from the
   terminal.
3. **MCP / Agent Bridge** — `faro_dedupe(session, path, hash)` tool, gated as a
   read, groups capped at 200; the response notes deletion is the caller's
   explicit follow-up so an agent reviews with the user before removing files.

## Integration points
`src-tauri/src/dedupe.rs` (new; `scan.rs` walk, `diff.rs::hash_path`,
`DiffManager`-shaped `DedupeManager` + `dedupe_start/status/result/cancel/
forget/delete` commands), `src-tauri/src/bridge.rs` (`op_dedupe` + tool def +
dispatch), `faro-cli/src/main.rs` (`Dedupe` subcommand), `packages/file-ui`
(`findDuplicates` adapter op + context-menu/toolbar entries), `src/lib/ipc.ts` /
`types.ts` / `fileUiAdapter.ts`, `src/App.tsx` (host mount).

## Risks
- False positives in name mode (a legit `photo (1).jpg` with the same size) —
  mitigated: groups are review-first, deletion is always explicit, keeper is
  only a suggestion.
- Hash cost on big trees — opt-in, like diff's `--hash`; server-side over SSH.
- Zero-byte files all hash equal — skipped in hash mode (never worth cleaning).

## Verification
`cargo check -p faro` + `cargo test -p faro dedupe` (6 tests: suffix stripping,
name-key normalization, name-mode grouping, keeper heuristic, hash mode
end-to-end on LocalFs) + `npx tsc --noEmit`; `faro-cli dedupe` on a local tree
in name mode (`_1`/` (1)` groups found, keepers correct) and `--hash` mode
(cross-directory renames grouped), `--delete --yes` removes non-keepers. Live
SSH/S3/agent run + GUI click-through left.
