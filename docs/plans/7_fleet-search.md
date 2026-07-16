# Plan 7 — Fleet Search (name + content, any backend)

## Context

Spotlight/Everything for a connection: instant **filename** search and
**content grep** across a remote tree — on servers, buckets, agents, or local.
The differentiator vs desktop search tools is the same as Disk Usage: it works
over `RemoteFs`, with an exec fast path that makes it genuinely fast on servers.

## Approach

### Search engine — `src-tauri/src/search.rs`
Two query kinds: **name** (glob/substring) and **content** (regex/literal grep).
Strategy picked by capability (fastest first):

1. **Exec fast path (SSH / agent)** — one command instead of a walk:
   `rg --json <pattern> <path>` (ripgrep, if present) or
   `grep -rn` / `find -name`. Parse to structured hits. `has_shell` (SSH) or an
   exec-allowed Faro Agent. This is what makes content search on a big server
   usable.
2. **Object stores** — name search over a flat key listing (S3/Azure); content
   search requires fetching objects (cap + warn), so default to name-only unless
   the user opts in.
3. **Generic walk fallback** — `RemoteFs::list_dir` for names; for content,
   stream-read candidate files and match client-side (bounded concurrency + size
   cap). Always available.

Streams hits with progress + cancel (mirror the `diskscan`/scan-manager pattern:
`Arc` in `AppState`, `search://hit`/`search://done` events, abort handle).

### Surfaces
- **GUI** — a search panel/tab: query box (name | content toggle, regex, case,
  include/exclude globs), streaming results grouped by file with line previews,
  click-to-open in the browser or (content hit) jump to the line.
- **CLI** — `faro-cli search <profile:path> <pattern> [--content] [--json]`.
- **MCP** — a `faro_search` bridge tool so the AI can locate files/log lines.

## Phases
1. Engine + generic walk (names) + client-side content match.
2. Exec fast path (`rg`/`grep`/`find`) for SSH + agent.
3. GUI search panel (streaming, previews, jump-to-line).
4. CLI + `faro_search` MCP tool.

## Integration points
`src-tauri/src/search.rs` (new), `src-tauri/src/lib.rs` (commands + events),
reuse `RemoteFs`, `Capabilities.has_shell`, the SSH exec channel, the Agent
`Exec` request, `ObjectFs` listing; `packages/file-ui` search panel;
`faro-cli/src/main.rs`; `src-tauri/src/bridge.rs` (`faro_search`).

## Risks
- Content search over SFTP/object = many reads → keep exec the default on SSH,
  cap + warn on the fallback, always cancellable.
- ripgrep may be absent → probe, fall back to `grep`/`find`, then the walk.
- Huge result sets → stream + virtualize the list, cap with a visible notice.

## Verification
`cargo check` + `tsc`; name + content search on: a local folder (walk), an SSH
server (confirm `rg`/`grep` fires and matches a manual run), the phone agent,
and an S3 bucket (name-only default). Progress streams; cancel works; CLI
`--json` and the `faro_search` tool return structured hits.
