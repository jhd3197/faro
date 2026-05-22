import { create } from "zustand";
import type { SessionId } from "@/lib/types";

// One TerminalTab per open xterm. Multiple tabs may belong to the same
// session — they share the SSH connection (separate PTY channels). When the
// active SSH session changes, tabs from prior sessions are kept in state but
// the panel only shows tabs that belong to the current session.
export interface TerminalTab {
  id: string; // client-side id, used as React key
  sessionId: SessionId;
  title: string; // editable; defaults to `Shell N`
}

interface TerminalsState {
  tabs: TerminalTab[];
  activeId: string | null;

  openTab: (sessionId: SessionId) => TerminalTab;
  closeTab: (id: string) => void;
  setActive: (id: string) => void;
  renameTab: (id: string, title: string) => void;
  /** Drop all tabs for a session (called on disconnect). */
  dropSessionTabs: (sessionId: SessionId) => void;
}

function genId() {
  return crypto.randomUUID();
}

export const useTerminals = create<TerminalsState>((set, get) => ({
  tabs: [],
  activeId: null,

  openTab: (sessionId) => {
    const sessionCount = get().tabs.filter((t) => t.sessionId === sessionId).length;
    const tab: TerminalTab = {
      id: genId(),
      sessionId,
      title: `Shell ${sessionCount + 1}`,
    };
    set((s) => ({ tabs: [...s.tabs, tab], activeId: tab.id }));
    return tab;
  },

  closeTab: (id) => {
    set((s) => {
      const idx = s.tabs.findIndex((t) => t.id === id);
      if (idx === -1) return s;
      const tabs = s.tabs.filter((t) => t.id !== id);
      let activeId = s.activeId;
      if (s.activeId === id) {
        // Pick a sibling tab in the same session if possible, else any tab.
        const sessionId = s.tabs[idx].sessionId;
        const sibling =
          tabs.filter((t) => t.sessionId === sessionId).pop() ??
          tabs[tabs.length - 1] ??
          null;
        activeId = sibling ? sibling.id : null;
      }
      return { tabs, activeId };
    });
  },

  setActive: (id) => set({ activeId: id }),

  renameTab: (id, title) =>
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === id ? { ...t, title } : t)),
    })),

  dropSessionTabs: (sessionId) =>
    set((s) => {
      const tabs = s.tabs.filter((t) => t.sessionId !== sessionId);
      const activeStillThere = tabs.some((t) => t.id === s.activeId);
      return { tabs, activeId: activeStillThere ? s.activeId : tabs[0]?.id ?? null };
    }),
}));
