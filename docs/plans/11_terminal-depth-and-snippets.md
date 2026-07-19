# Plan 11 — Terminal depth: persistent instances, split panes, snippets

## Context

Faro's terminal works — multi-tab, shared SSH session, popout windows — but it's
the shallowest surface in the app. Four concrete gaps keep it from being the
centerpiece a "Termius/WinSCP alternative" needs, and users will hit them in
week one:

1. **xterm instances live inside React components.** Remounts (dock toggle,
   tab switch edge cases, HMR) can cost scrollback. The fix is a module-level
   registry: xterm instances and their DOM nodes live outside React, and
   components only attach/detach the cached node — scrollback survives
   everything.
2. **No split panes.** Splits should open a second PTY channel **on the same
   russh connection** (no re-auth, no second TCP handshake), with a layout-tree
   store and Cmd+D / Cmd+Shift+D. Faro's `SessionManager` already pools one
   session per profile shared by browser + terminal — splits are the natural
   extension of that design.
3. **Terminal is SFTP-only** (`App.tsx` — `supportsTerminal = protocol ===
   "sftp"`), even though paired Faro agents can exec. Agent interactive shells
   need an additive protocol op; worth doing once the daemon has background-job
   ops (Plan 10) to model it on.
4. **No command snippets.** Faro has Fleet Skills (powerful, AI-authorable,
   fleet-wide) but nothing at the low-friction everyday end: "save these 5
   commands I type into every WordPress box, insert with Cmd+K, fill
   `{{site}}`." The bar: SQLite-backed, full-text searchable, `{{variable}}`
   templates, one-keystroke palette injection.

## What already exists (don't rebuild)

- `src-tauri/src/terminal.rs` — PTY over russh, terminal output/exit events.
- `SessionManager` (`src-tauri/src/session/`) — one pooled session per profile;
  the russh `Handle` is already shared so extra channels are cheap.
- `src/stores/terminalsStore.ts`, `src/components/Terminal.tsx`,
  `TerminalWindow.tsx` (popout shells), docks that stay CSS-hidden rather than
  unmounted (`App.tsx`) — the "keep alive" instinct exists, just not as a
  registry.
- `src/lib/termSuggest.ts` — ghost-text suggestions; snippet insertion feeds
  the same input path.
- `faro.db` (Plan 3) — SQLite in `AppState`, per-feature tables; snippets are
  one more table, no new subsystem.
- `src/lib/commands.tsx` + `CommandPalette.tsx` — the Cmd+K surface snippets
  plug into.

## Approach

### Phase 1 — Terminal instance registry (decouple xterm from React)
Module-level `Map<sessionId, { term, addons, element, dispose }>` in a new
`src/lib/terminalRegistry.ts` (or a vanilla section of `terminalsStore`):
- Components only call `attach(el)` / `detach()`; the xterm instance and its
  host element are created once and cached. `Terminal.tsx` becomes a thin
  viewport.
- Eager creation on the first output byte so MOTD/early output landing before
  mount isn't dropped (buffer-and-flush is the alternative — registry is
  simpler).
- Disposal is driven by the terminals store (source of truth), not by React
  unmount. HMR cleanup via `import.meta.hot.dispose`.
- Keep popout `TerminalWindow` behavior identical — it just attaches the same
  cached instance in the new window.

### Phase 2 — Split panes on one SSH connection
- Backend: `terminal_split` command opening a second PTY channel on the pooled
  session's russh `Handle`. No new connection, no re-auth. Cancellation via
  `tokio_util::sync::CancellationToken` like the rest.
- Frontend: layout tree in `terminalsStore` (`{type:"leaf",sessionId} |
  {type:"split",dir,ratio,children}`), a `SplitContainer` renderer with drag
  handles, Cmd+D (vertical) / Cmd+Shift+D (horizontal), Cmd+Shift+Enter zoom
  pane, close-pane. Render-all/toggle-visibility so every pane's registry
  instance stays mounted.
- Shortcuts registered as data in `useShortcuts` (predicate-gated so chords
  don't leak into the shell — xterm `attachCustomKeyEventHandler` returns
  `false` for app-level chords).

### Phase 3 — Terminal over the Faro Agent protocol (stretch, additive)
Interactive shell channel for paired agents: additive `faro-agent-proto` op
(PTY/spawn + stream, modeled on Plan 10's background-job ops so older daemons
keep working — capability-gated). `supportsTerminal` becomes
`sftp || agent(supports_pty)`. Falls back cleanly: no button when unsupported.

### Phase 4 — Command snippets
- `snippets` table in `faro.db` (name, body, folder, use-count, timestamps) +
  Tauri commands (`snippet_list/save/delete/run`). Full-text search over
  name+body.
- `{{variable}}` template substitution: on insert, scan for placeholders, show
  one small `VariableDialog` collecting values, then inject the resolved text
  into the focused terminal's input (same path as `termSuggest`).
- UI: a Snippets page/panel (browse, folders, edit), Cmd+K palette section
  ("Snippets: …"), and a quick-insert button in the terminal toolbar. Usage
  count feeds palette ordering.
- Explicitly **not** Fleet Skills: snippets are local, single-session, zero
  ceremony. A "promote to Skill" action can come later; not in this plan.

## Integration points
- `src-tauri/src/terminal.rs` (split command), `src-tauri/src/session/` (channel
  on pooled handle), `src-tauri/src/db.rs` (snippets table + migration),
  `faro-agent-proto` + `faro-agentd` (Phase 3 only).
- `src/lib/terminalRegistry.ts` (new), `src/stores/terminalsStore.ts` (layout
  tree), `src/components/Terminal.tsx`, new `SplitContainer.tsx`,
  `SnippetsPanel.tsx`, `VariableDialog.tsx`, `src/lib/commands.tsx`,
  `src/hooks/useShortcuts.ts`, `App.tsx` (`supportsTerminal`).

## Risks
- **Split-pane focus/IME edge cases** with cached xterm elements — test tab
  switch + popout + split together; the registry makes this deterministic but
  the attach/detach ordering must be exact.
- **Chord capture** — app shortcuts must never reach the shell mid-command;
  predicate-gate on terminal focus.
- **Phase 3 protocol drift** — additive op + capability flag only; never break
  older daemons.
- **Snippet injection safety** — paste into the terminal *input line*, never
  auto-execute; multi-line bodies get a confirm (bracketed-paste style).

## Verification
`cargo check -p faro` + `npx tsc --noEmit` clean. Runtime: open 3 tabs, split
twice, generate scrollback in each, toggle docks/popouts, confirm zero loss;
`kill` the SSH connection and confirm all panes on it show the disconnect
overlay. Snippets: save/insert a `{{var}}` snippet into a live shell, confirm
dialog → resolved text at the prompt, never auto-submitted. Phase 3: pair a
phone agent with a rebuilt daemon and run an interactive `top`/`vi` session.
