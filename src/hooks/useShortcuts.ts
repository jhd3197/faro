import { useEffect, useRef } from "react";
import { useCommands } from "@/lib/commands";
import { useLayout } from "@/stores/layoutStore";
import { keyCombo } from "@/lib/shortcuts";

// One global keydown listener that dispatches the command registry. Every
// registered combo uses "mod", so non-modifier keystrokes (typing, FilePane's
// own Esc/Delete/Ctrl+A) pass straight through untouched.
export function useShortcuts() {
  const commands = useCommands();
  const togglePalette = useLayout((s) => s.togglePalette);

  // Keep the latest commands without re-binding the listener every render.
  const ref = useRef(commands);
  ref.current = commands;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const combo = keyCombo(e);
      if (combo === "mod+k") {
        e.preventDefault();
        togglePalette();
        return;
      }
      const cmd = ref.current.find(
        (c) => c.combo === combo && c.enabled !== false
      );
      if (cmd) {
        e.preventDefault();
        cmd.run();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [togglePalette]);
}
