import { useMemo, useState } from "react";
import { X, Keyboard } from "lucide-react";
import { useLayout } from "@/stores/layoutStore";
import { useCommands } from "@/lib/commands";
import { formatCombo } from "@/lib/shortcuts";

// Global shortcuts that aren't registry commands (palette + FilePane keys).
const EXTRAS: { group: string; title: string; combo: string }[] = [
  { group: "General", title: "Command Palette", combo: "mod+k" },
  { group: "File pane", title: "Move cursor", combo: "arrows" },
  { group: "File pane", title: "Open / transfer", combo: "enter" },
  { group: "File pane", title: "Up one folder", combo: "backspace" },
  { group: "File pane", title: "Jump to name", combo: "type" },
  { group: "File pane", title: "Select all", combo: "mod+a" },
  { group: "File pane", title: "Delete selection", combo: "delete" },
  { group: "File pane", title: "Clear filter / selection", combo: "escape" },
];

export function KeyboardShortcutsDialog() {
  const open = useLayout((s) => s.shortcutsOpen);
  const setOpen = useLayout((s) => s.setShortcutsOpen);
  const commands = useCommands();
  const [q, setQ] = useState("");

  const groups = useMemo(() => {
    const rows = [
      ...EXTRAS,
      ...commands
        .filter((c) => c.combo)
        .map((c) => ({ group: c.group, title: c.title, combo: c.combo! })),
    ].filter(
      (r) => q === "" || r.title.toLowerCase().includes(q.toLowerCase())
    );
    const byGroup = new Map<string, { title: string; combo: string }[]>();
    for (const r of rows) {
      if (!byGroup.has(r.group)) byGroup.set(r.group, []);
      byGroup.get(r.group)!.push({ title: r.title, combo: r.combo });
    }
    return Array.from(byGroup.entries());
  }, [commands, q]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-[75] flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={() => setOpen(false)}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="anim-modal flex max-h-[80vh] w-[32rem] flex-col rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <div className="flex items-center gap-2 border-b border-border px-5 py-3.5">
          <Keyboard size={14} className="text-text-muted" />
          <span className="text-[15px] font-semibold tracking-tight">
            Keyboard Shortcuts
          </span>
          <div className="flex-1" />
          <button
            onClick={() => setOpen(false)}
            className="rounded-md p-1.5 text-text-muted hover:bg-bg-hover hover:text-text"
          >
            <X size={14} />
          </button>
        </div>
        <div className="border-b border-border px-5 py-2">
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search shortcuts…"
            className="w-full rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent"
          />
        </div>
        <div className="flex-1 overflow-y-auto px-5 py-2">
          {groups.length === 0 ? (
            <div className="py-8 text-center text-xs text-text-dim">
              No shortcuts match.
            </div>
          ) : (
            groups.map(([group, rows]) => (
              <div
                key={group}
                className="border-b border-border-subtle py-3 last:border-b-0"
              >
                <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.08em] text-text-muted">
                  {group}
                </div>
                <div className="space-y-1.5">
                  {rows.map((r) => (
                    <div
                      key={r.title}
                      className="flex items-center justify-between"
                    >
                      <span className="text-sm text-text-muted">{r.title}</span>
                      <Kbd combo={r.combo} />
                    </div>
                  ))}
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

const SPECIAL_KEYS: Record<string, string> = {
  delete: "Del",
  escape: "Esc",
  arrows: "↑ ↓",
  enter: "Enter",
  backspace: "Backspace",
  type: "Type a name",
};

function Kbd({ combo }: { combo: string }) {
  const label = SPECIAL_KEYS[combo] ?? formatCombo(combo);
  return (
    <span className="rounded border border-border bg-bg-subtle px-1.5 py-0.5 font-mono text-[11px] text-text-dim">
      {label}
    </span>
  );
}
