# Plan 15 — Keyboard shortcuts: remapping, settings UI & file-browser keys

## Context

Every serious desktop app has two things Faro lacks:

1. **Remappable shortcuts in Settings** — "F2 to rename", "I want my own
   bindings". Faro's combos are **hardcoded constants**.
2. **Non-modifier shortcuts** — today the global dispatcher
   (`src/hooks/useShortcuts.ts`) returns early unless `ctrl`/`meta` is held, so
   `F2`, `Delete`, `Enter`, `Space`, `Backspace` can never trigger anything.

What already exists (and this plan builds on, not replaces):

- **A command registry** (`src/lib/commands.tsx`) — every palette action with
  `id`, `title`, `group`, `combo`. Single source of truth; the palette, the
  title-bar menus and the cheat-sheet already read from it.
- **A global dispatcher** (`src/hooks/useShortcuts.ts`) — one keydown listener
  matching `keyCombo(e)` against the registry.
- **A cheat-sheet** (`src/components/KeyboardShortcutsDialog.tsx`) — read-only
  today; becomes the settings surface's sibling.
- **Terminal chords** (`src/lib/terminalChords.ts`) — hardcoded split/zoom/close
  chords, deliberately swallowed by xterm.
- **Settings in `faro.db`** (Plan 12, shipped) — the persistence substrate for
  user overrides, with pre-paint injection so bindings apply before first paint.

So this is **not** "build a shortcut system" — it's "turn the hardcoded registry
into data, add a settings UI to edit it, and teach the dispatcher about
non-modifier keys and contexts".

## Scope

**In:**
- A **bindings layer**: default combos live in the registry as today; user
  overrides are stored in `faro.db` (`settings` table, `shortcut.<commandId>`
  keys) and win over defaults. Effective binding = override ?? default.
- A **Settings → Keyboard tab**: searchable, grouped list of every command with
  its current binding, a click-to-record capture field, conflict detection,
  per-command reset, and reset-all.
- **Non-modifier & bare-key support** in the dispatcher, with input-focus guards
  (never fire while typing in an input/textarea/contenteditable or a terminal),
  so `F2`/`Enter`/`Delete`/`Space`/`Backspace` are usable.
- **File-browser shortcuts**: `F2` rename, `Enter` open, `Delete` delete,
  `Backspace` go up, `Ctrl/Cmd+A` select all (exists), `Space` quick info —
  registered as real commands so they're remappable too.
- **Remappable terminal chords** — `TERMINAL_CHORDS` becomes defaults, same
  override mechanism (still xterm-swallowed, never reaching the shell).

**Explicitly out:**
- Multi-key sequences / leader keys (`Ctrl+K Ctrl+S` style). Single combos only.
- Recording *macros* (that's Fleet Skills / Snippets territory).
- Syncing bindings across machines (Plan 12's encrypted backup already carries
  `faro.db`, so backups include them — nothing more to do).

## Approach

### Phase 1 — Bindings layer
- New `src/lib/keybindings.ts`: `DEFAULTS` derived from the command registry,
  `effectiveCombo(commandId)` = override ?? default, override map seeded from
  the settings injection and kept in a small store (`bindingsStore`).
- `useShortcuts` matches against **effective** combos instead of `c.combo`;
  the palette, menus and cheat-sheet display effective combos too (one resolver,
  no duplicated maps).
- Backend: reuse Plan 12's `settings` table + commands — no new schema, just
  `shortcut.*` keys; set/delete on change, included in backup for free.

### Phase 2 — Settings → Keyboard tab
- New section in `src/components/Settings.tsx`: grouped, searchable command list
  (reuse the cheat-sheet's grouping) with a record button per row:
  click → "press keys…" captures the next keydown via `keyCombo`, `Esc` cancels,
  `Backspace` clears the binding (restores default).
- **Conflict detection**: if the captured combo is already bound in the same
  context, show the collision and offer swap/overwrite/cancel.
- Reset per row + "Reset all to defaults".

### Phase 3 — Non-modifier keys, contexts & file-browser keys
- Dispatcher: drop the blanket mod-gate; instead skip when the event target is
  editable (`input`, `textarea`, `select`, `contenteditable`, the xterm helper
  textarea) **unless** the combo still has `mod` (mod-combos stay global).
- Lightweight **contexts**: `global` (default), `file-browser`, `terminal`.
  A command declares its context; the dispatcher only fires it when that
  surface is focused/active. Keeps `Delete` in the browser from colliding with
  anything global.
- Promote FilePane's existing ad-hoc key handling (Esc/Delete/Ctrl+A) into
  registered commands and add: `F2` rename, `Enter` open, `Backspace` parent
  dir, `Space` properties/quick info, `Ctrl/Cmd+Shift+N` new folder,
  `F5`/`mod+r`-in-context refresh.

### Phase 4 — Remappable terminal chords
- `terminalChords.ts` exports defaults; effective chords come from the same
  bindings layer; xterm keeps swallowing them. The cheat-sheet and palette show
  the effective chord.

## Integration points
- `src/lib/keybindings.ts` (new), `src/lib/commands.tsx` (defaults + context
  field), `src/hooks/useShortcuts.ts` (resolver + guards + contexts),
  `src/lib/shortcuts.ts` (combo helpers, unchanged),
  `src/stores/` (small `bindingsStore` seeded from the Plan 12 injection),
  `src/components/Settings.tsx` (Keyboard tab),
  `src/components/KeyboardShortcutsDialog.tsx` (show effective combos),
  `src/lib/terminalChords.ts` (defaults only),
  FilePane key handling → registered commands.
- Backend: **none new** — `settings` table + Plan 12 commands only.

## Risks
- **Stealing keys from typing** — the editable-target guard is the critical
  correctness piece; xterm's hidden textarea must count as editable.
- **Swallowed chords** — xterm and the webview reserve some combos; record UI
  should warn on known-unusable combos (e.g. bare `F5` if we keep it for
  refresh in-context only).
- **Drift between surfaces** — palette/menu/cheat-sheet/settings must all read
  the one resolver, never their own copy of a combo.
- **Migration** — existing users keep today's hardcoded combos as defaults;
  overrides are additive, so no breaking change.

## Verification
`tsc` clean + `vite build`; headless mock-harness script (like
`scripts/verify-terminal.mjs`): rebind "Toggle Terminal" to `mod+shift+`` and
confirm it fires and the cheat-sheet/palette show the new combo; confirm `F2`
starts a rename in the file browser but does nothing while typing in an input;
confirm a conflicting binding is rejected in the settings UI; confirm overrides
survive an app restart (seeded from `faro.db` injection).
