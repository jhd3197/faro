# Plan 10 — faro-cli & Agent Bridge: remote exec/write DX

## Context

This plan comes straight from a real session driving a WordPress staging box
through `faro-cli agent …`. The core workflow — diagnose via the DB, run WP-CLI,
clear caches — worked well, but four rough edges forced a lot of base64
gymnastics and `nohup`/poll workarounds. The goal here is to make "drop a small
script on the server, run something that takes a few minutes, read a page behind
auth" first-class, so neither a human nor an AI agent has to hand-roll the
escapes.

The verbatim pain points, and what the code actually does today:

1. **Remote file upload/edit mangled the destination path** — `faro-cli agent
   upload <server> <local> /var/www/html` turned the remote dir into a local
   Windows path (`C:/Program Files/Git/var/www/…`). **Root cause:** Git Bash /
   MSYS2 POSIX-path conversion rewrites the `/var/www/…` argument *before*
   `faro-cli` ever sees it — `op_upload` (`bridge.rs`) passes `remoteDir`
   straight through untouched. There is also **no "write text to a remote file"
   command** at all, so the session base64-encoded scripts and piped them
   through `printf | base64 -d`.
2. **Exec timed out at ~60 s**, forcing a `nohup` + tail-the-log loop. The 60 s
   default is real (`EXEC_TIMEOUT` in `bridge.rs`). A `--timeout-ms` override
   **already exists** (see #5) but there is **no background/detached job** with a
   pollable id — the exact thing a multi-minute backfill (`wp p7
   reindex-facets`) wants.
3. **Multi-line scripts broke** — heredocs and nested quoting don't survive the
   shell → HTTP → remote-shell layering, which is *why* everything got
   base64-encoded. There is no "run this local script file / stdin as-is"
   escape hatch.
4. **No read access to pages behind HTTP Basic Auth** — staging is auth-walled,
   so the agent couldn't `curl` rendered HTML and fell back to injecting debug
   logs. Faro already has an authenticated HTTP(S) backend (Plan 5 Phase 4) whose
   credentials could serve this.
5. **`--timeout-ms` was rejected** (`error: unexpected argument '--timeout-ms'`).
   **This is a version-lag artifact, not a missing feature:** the flag is present
   in `faro-cli/src/main.rs` today (`AgentCmd::Exec { … timeout_ms: Option<u64> }`,
   `--timeout-ms`, forwarded as `timeoutMs`, clamped by the bridge to
   [1 s, 15 min]). The installed binary on that box predated it. **The maintainer
   diagnosed the cause:** the desktop *app* was updated but the standalone
   `faro-cli` was not — the installer ships only the GUI; `faro-cli` (and
   `faro-agentd`) are **separate release downloads** (see `docs/remote-agent.md`).
   So the app's docs and MCP tool schemas advertised `--timeout-ms` while the
   on-disk CLI predated it, and **the app had no way to know the CLI was stale
   (or missing).** The fix is a version-drift check + update flow (Phase 0), plus
   closing a ceiling mismatch (below).

## What already exists (don't rebuild)

- **`--timeout-ms`** on `faro-cli agent exec` → bridge `timeoutMs` →
  `exec_timeout_from()` clamps to `[EXEC_TIMEOUT_MS_MIN=1_000,
  EXEC_TIMEOUT_MS_MAX=900_000]`. Default 60 s. The MCP `faro_exec` /
  `faro_run_command` tools expose the same `timeoutMs`.
- **`WriteChunk{path,offset,data,truncate,done}`** in `faro-agent-proto` — the
  ranged-write primitive uploads already use; a "write text" op reuses it for
  agent targets, and SFTP `create` for SSH.
- **`agent upload` / `upload-dir`** — real transfers through `TransferManager`;
  they just take a *local file path*, so they can't drop inline/stdin content.

## Approach

Six phases, cheapest and highest-leverage first. Phase 0 is the priority and the
only app-side (frontend + backend) work; Phases 1–3 are `faro-cli` + `bridge.rs`
(no protocol change); Phase 4's agent arm is the only piece that touches
`faro-agent-proto`; Phase 5 reuses Plan 5's HTTP backend.

### Phase 0 — CLI version-drift: detect, surface, update (the priority)
The app and `faro-cli` ship as **separate downloads**, so the CLI silently lags
the app after an app update — and nothing tells the user (this is what produced
the `--timeout-ms` confusion). Close the gap in layers, cheap → full:

- **0a. CLI self-check (near-free early warning).** The app already writes
  `agent-endpoint.json` (bridge URL + token) to its data dir. Add the **app
  version** to that file. When `faro-cli agent …` runs, it compares its own build
  version (clap `#[command(version)]`) to the app version in the endpoint file and
  prints a one-line stderr warning when older — e.g. *"⚠ faro-cli vY is older than
  the running Faro app vX; some commands/flags may be missing — update with
  `faro-cli self-update` or from Faro → Settings."* This alone turns the session's
  cryptic `unexpected argument '--timeout-ms'` into a clear "your CLI is stale."
- **0b. `faro-cli self-update` subcommand.** Downloads the release asset matching
  the app/target from GitHub releases (same source `scripts/install-agentd.sh`
  uses for the daemon), verifies it, and replaces the running binary (on Windows,
  rename-in-place / swap-on-next-launch since a running exe can't overwrite
  itself). Prints before/after version.
- **0c. App-side startup check + update prompt.** On app start — and on demand
  from Settings — the app **locates the installed `faro-cli`** (PATH via
  `where`/`which`, a known install dir, or the bundled sidecar), runs
  `faro-cli --version`, and compares to the app version / a min-compatible
  version. If stale, it surfaces a **non-blocking** prompt: *"A newer faro-cli
  (vX) is available — update now?"* with **Update now · Not now · Always update
  automatically**. If the CLI is **missing**, the prompt instead offers **Install
  the CLI**. Wire the prompt off the status-bar pill (there is no tray), and model
  the check as a small stateful subsystem in the shape of `agent_host.rs`.
- **0d. Preference — "don't ask again / do it automatically."** Persist a
  `cliUpdate: "ask" | "auto" | "off"` setting (default `ask`), modeled on
  `settingsStore.ts` + `Settings.tsx`, exposed via a typed IPC wrapper in
  `ipc.ts`. **"Always update automatically"** flips it to `auto` → future launches
  download+swap silently and never prompt; `off` disables the check entirely. This
  is exactly the "check to not ask again and do it automatically" the maintainer
  asked for.
- **Structural fix (cross-ref [Plan 1](1_faro-agent-pairing-and-distribution.md)
  Phase 3):** bundling `faro-cli` as a Tauri **sidecar** shipped with the app
  makes "update the app" update the CLI too, so drift can't happen for the bundled
  copy. The version-check + `self-update` here is the safety net for a
  **separately installed / on-PATH** CLI — the daily-driver case that bit this
  session — and the two compose: prefer the bundled sidecar, and offer to update a
  stale PATH copy.
- **0e. Align the exec ceilings (small correctness, ship regardless).** The bridge
  clamps exec to **15 min** (`EXEC_TIMEOUT_MS_MAX = 900_000`) but `faro-agentd`'s
  `exec()` independently caps at **10 min** (`timeout_ms.clamp(1, 10*60*1000)` in
  `ops.rs`), so `--timeout-ms 900000` against a *paired agent* is silently capped.
  Lift the daemon cap to match (shared const) so the documented 15 min holds on
  every target.

### Phase 1 — Run a local script / stdin as-is (kills the base64 gymnastics)
- `faro-cli agent script <server> <local-file>` and `faro-cli agent exec
  <server> --file <local>` / `--stdin`. The CLI reads the script **bytes**
  locally and ships them as an opaque payload; the bridge runs them via the
  target's native "read program from stdin" mode (`sh -s` / `bash -s` on POSIX,
  `pwsh -Command -` / `cmd /Q` on Windows agents) instead of splicing the text
  into a command line. Heredocs, nested quotes, and newlines survive because
  nothing re-parses them at a shell boundary.
- Wire-transport can still base64 the payload internally — the point is the
  *user* never sees it. New bridge route `/exec_script` (or `stdin` field on
  `/exec`); same approval gate and 512 KiB output cap as `exec`.

### Phase 2 — Write text to a remote file directly (`agent write`)
- `faro-cli agent write <server> <remote-path> [--from-file <local> | --stdin |
  --content <text>] [--overwrite]`. Drops a debug script or a one-file patch
  without a staging local file and **without going through the mangling-prone
  upload path**. SSH targets stream via SFTP `create`; agent targets via
  `WriteChunk`. New bridge route `/write` gated as a Write.
- This is the direct answer to "a working upload or a dedicated edit command
  would have let me drop a small debug script or patch directly on the server."

### Phase 3 — Guard against shell-mangled remote paths (defensive)
- When a remote-path argument arrives Windows-drive-prefixed
  (`^[A-Za-z]:[\\/]`) **and the target is a POSIX server**, reject with an
  actionable message rather than uploading to a nonsense path: *"that remote
  path looks like Git Bash rewrote it (MSYS path conversion). Re-run with
  `MSYS_NO_PATHCONV=1`, prefix the path with `//`, or use `agent write`."* Apply
  to `agent upload`, `upload-dir`, `download`, and the Phase 2 `write`.
- Document the `MSYS_NO_PATHCONV=1` / leading-`//` escape in
  `docs/remote-agent.md`. Cheap, and it converts a silent wrong-destination into
  a clear error.

### Phase 4 — Background/detached exec with a job id (retires the nohup loop)
- `faro-cli agent exec <server> --detach` returns a **job id** immediately;
  `faro-cli agent job <id>` polls stdout/stderr/exit; `faro-cli agent jobs`
  lists running/finished jobs. Productizes the manual `nohup … & ; tail -f log`
  the session fell back to.
- **SSH-first (pure convention, no protocol change):** the bridge spawns the
  command detached server-side (`setsid`/`nohup`) into a per-job dir
  (`~/.faro/jobs/<id>/{cmd,out,err,exit,pid}`); `job`/`jobs` read those files.
  Cheap and immediately useful.
- **Agent targets (protocol work, second):** add `ExecStart` / `ExecPoll` /
  `ExecKill` to `faro-agent-proto` so `faro-agentd` can spawn, track by job id,
  and stream partial stdout + final exit — the daemon equivalent of the SSH job
  dir. Gated and audited like `Exec`.

### Phase 5 — Read behind HTTP Basic Auth (stretch; reuse Plan 5)
- `faro-cli fetch <url>` (or `agent http-get <server> <path>`) performs an
  authenticated GET reusing a saved **HTTP(S) profile's** stored credentials
  (Plan 5 Phase 4's `HttpFs` already holds Basic-Auth creds), so the agent can
  read rendered pages on an auth-walled staging site instead of injecting debug
  logging. Scope to Basic Auth first; session-cookie/browser proxying is a
  larger, separate effort and explicitly out of scope here.

## Integration points

- `src-tauri/faro-cli/src/main.rs` — Phase 0: `self-update` subcommand + the
  stale-version stderr warning (reads app version from `agent-endpoint.json`).
  Phases 1–5: new `AgentCmd` variants (`Script`, `Write`, `Job`, `Jobs`),
  `--file`/`--stdin` on `Exec`, `--detach`; a `fetch`/`http-get` path for Phase 5.
  Reuses `read_endpoint` / `resolve_server` / `http_post`.
- **App side (Phase 0):** write the app version into `agent-endpoint.json` (the
  bridge's discovery-file writer); a small "CLI updater" subsystem shaped like
  `agent_host.rs` that locates the CLI, compares versions on start, and
  downloads+swaps; the `cliUpdate` setting in `src/stores/settingsStore.ts` +
  `src/components/Settings.tsx`; a typed IPC wrapper in `src/lib/ipc.ts`; the
  prompt + status surface on the status-bar pill in `src/App.tsx`.
- `src-tauri/src/bridge.rs` — new routes `/exec_script`, `/write`, `/job`,
  `/jobs` (+ the detached branch of `/exec`); the ceiling const in Phase 0; the
  matching MCP tool schemas (`faro_write`, `faro_exec_script`, `faro_job`) so the
  in-app AI agent gets the same affordances, gated by the existing approval flow.
- `src-tauri/faro-agentd/src/ops.rs` — daemon cap (Phase 0); the `ExecStart/
  Poll/Kill` handlers (Phase 4).
- `src-tauri/faro-agent-proto/src/msg.rs` — the `ExecStart/ExecPoll/ExecKill`
  request/response set (Phase 4 only). `WriteChunk` already covers Phase 2.
- `src-tauri/src/session/agent.rs` — client side of the new agent requests.
- `docs/remote-agent.md` — document `agent write`/`script`/`job`, the MSYS
  escape, the CLI version, and the aligned 15-min ceiling.

## Risks

- **A "write/script" command is a Write** — it runs through the same per-command
  approval gate as `exec`/`upload`; never auto-approved by the read/safe-exec
  policies (allow-all only), same as `faro_sync`.
- **Background jobs leak server-side files** if not reaped — cap per-job output,
  bound the job dir, and prune finished jobs on a TTL. The daemon arm tracks jobs
  in memory + a small on-disk spill so a restart doesn't orphan them.
- **Windows agents' stdin-program mode** differs (`pwsh -Command -` vs `cmd`) —
  detect shell from `SystemInfo` and pick the right invocation; fall back to a
  temp-file-then-run if a target can't take a program on stdin.
- **Phase 5 credential reuse** must respect the same redaction/consent as
  profiles — never echo the Basic-Auth header into logs or the console.

## Verification

- `cargo check -p faro` clean · `npx tsc --noEmit` exit 0.
- **Phase 0:** with an intentionally old `faro-cli` on PATH, launching Faro shows
  the update prompt; **Update now** swaps the binary and `faro-cli --version` then
  matches the app; **Always update automatically** persists and a subsequent
  launch updates silently with no prompt; a stale `agent …` call prints the
  one-line staleness warning. And `faro-cli agent exec <ssh> --timeout-ms 900000
  "sleep 700; echo ok"` completes on both an SSH server and a paired agent (no
  silent 10-min cap).
- **Phase 1:** a multi-line script with heredocs + nested quotes runs verbatim via
  `agent script` with zero base64 and identical output to running it locally.
- **Phase 2:** `agent write` drops a file with exact bytes (verify via `agent
  read`); `--stdin` and `--from-file` both work; overwrite honored.
- **Phase 3:** an MSYS-mangled `C:/…/var/www` remote path is rejected with the
  escape hint, not uploaded.
- **Phase 4:** a >2-minute backfill via `--detach` returns a job id instantly;
  `agent job <id>` shows progress and the final exit code on SSH and on an agent.
- **Phase 5:** `faro-cli fetch` returns the HTML of a Basic-Auth-protected page
  using a saved HTTP profile.
- Smoke-test the real flow against a live SSH box and a paired phone agent
  (`adb forward tcp:8722 tcp:8722`), per the working agreement's "compiles ≠
  works."
