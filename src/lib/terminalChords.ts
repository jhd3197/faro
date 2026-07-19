// Terminal split/zoom/close chords (Plan 11 Phase 2), in one place so the
// command registry (which binds them, discoverable in the palette + cheat
// sheet) and the terminal itself (which must SWALLOW them so they never reach
// the shell) can't drift apart.
//
// Why `mod+shift+…` and not the plan's bare Cmd+D: "mod" is Ctrl on Windows/
// Linux, and a bare Ctrl+D is EOF in every shell — binding split to it would
// break the terminal. The shift-qualified chords collide with nothing the
// shell needs, and the terminal swallows them anyway (see isTerminalChord).

export const TERMINAL_CHORDS = {
  splitRight: "mod+shift+d", // side-by-side (vertical divider)
  splitDown: "mod+shift+e", // stacked (horizontal divider)
  zoom: "mod+shift+enter",
  closePane: "mod+shift+w",
} as const;

const CHORD_KEYS = new Set(["d", "e", "w", "enter"]);

/** Raw-event matcher mirroring TERMINAL_CHORDS, for xterm's
 *  `attachCustomKeyEventHandler`: returning false there makes xterm ignore the
 *  key (no bytes to the PTY) and lets it bubble to the app shortcut handler. */
export function isTerminalChord(ev: KeyboardEvent): boolean {
  if (ev.type !== "keydown") return false;
  if (!(ev.ctrlKey || ev.metaKey) || !ev.shiftKey || ev.altKey) return false;
  return CHORD_KEYS.has(ev.key.toLowerCase());
}
