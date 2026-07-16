# Plan 9 — Fleet Skills (AI-authored, MCP-native automations)

## Context

Reshape of "multi-server command runner." Instead of static saved snippets, the
angle is **reusable Skills the AI can compose, save, and run across the fleet
through MCP** — the same idea as Claude Code / MCP skills, but for server ops.
"Restart the web tier", "rotate logs everywhere", "audit for world-writable
files" become named, parameterized, multi-step Skills an agent can invoke on one
server or many.

This builds directly on primitives that already exist: the Agent Bridge already
has **saved commands** (`bridge_save_command` / `bridge_list_commands` /
`bridge_delete_command`) and MCP registration (`bridge_register_mcp`), plus the
`faro_exec` tool over any connected server. A Skill is the next abstraction up.

## What a Skill is

A named, parameterized workflow: `{ name, description, params[], steps[],
targets (session selector), policy }`. Steps are `faro_exec` / file ops /
conditionals over `${params}`. Crucially it's **MCP-native**: each Skill is
exposed to the AI as a callable tool (`skill:<name>`), so the agent can *author*
a Skill (compose + save it via a `faro_save_skill` tool) and later *invoke* it —
the "MCPs create skills" angle.

## Approach
- **Storage** — extend the bridge's saved-commands store into a Skills store
  (`src-tauri/src/bridge.rs`), JSON under the app data dir. Migrate existing
  saved commands into single-step Skills.
- **Runner** — execute a Skill across a `targets` selector (one server / a group
  / all connected), fanning `faro_exec` out with bounded concurrency and
  aggregating output per target. Reuse the Agent Bridge's exec + approval path.
- **MCP surface** — `faro_list_skills`, `faro_run_skill(name, params, targets)`,
  and `faro_save_skill(def)` bridge tools, so an agent can list, run, and
  **create** Skills. Each saved Skill also surfaces as its own `skill:<name>`
  tool for direct invocation.
- **CLI** — `faro-cli skill run <name> [--target ...] [--param k=v]` and
  `faro-cli skill list`.
- **GUI** — a Skills panel: browse/edit Skills, pick targets, dry-run, run,
  watch aggregated output. Author-by-hand as well as author-by-AI.

## Safety (non-negotiable for fleet exec)
- Every step is policy-gated (the daemon/bridge exec policy already exists);
  a Skill can't exceed a target's allowed actions.
- **Dry-run** mode (show the resolved commands per target without running).
- Explicit confirm before a multi-target run; per-target success/fail summary.
- Full audit log (the bridge already logs activity — extend per Skill run).
- AI-authored Skills are saved as **proposals** requiring one human approval
  before they're runnable, so the agent can't self-grant a destructive workflow.

## Phases
1. Skills store + runner (single target), migrate saved commands.
2. Multi-target fan-out + aggregation + dry-run + audit.
3. MCP tools (`list`/`run`/`save_skill` + per-Skill `skill:<name>` tools).
4. GUI Skills panel; CLI `skill` subcommands.

## Integration points
`src-tauri/src/bridge.rs` (Skills store, runner, MCP tools — builds on the
existing saved-commands + `faro_exec` + approval machinery), `src-tauri/src/lib.rs`
(commands), `faro-cli/src/main.rs` (`skill` subcommand), a GUI Skills panel +
`src/lib/ipc.ts`.

## Risks
- Fleet exec is inherently dangerous → dry-run, approval-gated AI authoring,
  policy caps, and audit are requirements, not extras.
- Partial failure across targets → clear per-target reporting, no silent
  "mostly worked."
- Scope creep toward a full orchestration engine → keep Skills linear + simple;
  defer branching/looping unless needed.

## Verification
`cargo check` + `tsc`; author a Skill by hand and via the AI (save-as-proposal →
approve); dry-run then run across two connected servers with aggregated output;
confirm policy denial on a read-only target; audit entries present; CLI + MCP
tools list/run/save correctly.
