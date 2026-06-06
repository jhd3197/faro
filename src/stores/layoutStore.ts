import { create } from "zustand";

// Which app-level modal is open (mutually exclusive). Centralized here so the
// command palette, keyboard shortcuts and title-bar menus can all open them
// without prop-threading through App.
export type AppDialog =
  | "settings"
  | "newConnection"
  | "import"
  | "about"
  | "agentBridge";

interface LayoutState {
  terminalOpen: boolean;
  setTerminalOpen: (open: boolean) => void;
  toggleTerminal: () => void;

  // The Agent console is a dockable bottom panel (not a modal) so it can sit
  // open alongside the file browser and the Bridge control panel.
  consoleOpen: boolean;
  setConsoleOpen: (open: boolean) => void;
  toggleConsole: () => void;

  dialog: AppDialog | null;
  openDialog: (d: AppDialog) => void;
  closeDialog: () => void;

  // When true, the file browser shows the LOCAL filesystem instead of the
  // active server. Toggled by the rail's "Local" home bubble; cleared when a
  // server bubble is selected.
  browseLocal: boolean;
  setBrowseLocal: (v: boolean) => void;

  paletteOpen: boolean;
  setPaletteOpen: (v: boolean) => void;
  togglePalette: () => void;

  shortcutsOpen: boolean;
  setShortcutsOpen: (v: boolean) => void;
}

export const useLayout = create<LayoutState>((set) => ({
  terminalOpen: false,
  setTerminalOpen: (open) => set({ terminalOpen: open }),
  toggleTerminal: () => set((s) => ({ terminalOpen: !s.terminalOpen })),

  consoleOpen: false,
  setConsoleOpen: (open) => set({ consoleOpen: open }),
  toggleConsole: () => set((s) => ({ consoleOpen: !s.consoleOpen })),

  dialog: null,
  openDialog: (d) => set({ dialog: d }),
  closeDialog: () => set({ dialog: null }),

  browseLocal: false,
  setBrowseLocal: (v) => set({ browseLocal: v }),

  paletteOpen: false,
  setPaletteOpen: (v) => set({ paletteOpen: v }),
  togglePalette: () => set((s) => ({ paletteOpen: !s.paletteOpen })),

  shortcutsOpen: false,
  setShortcutsOpen: (v) => set({ shortcutsOpen: v }),
}));
