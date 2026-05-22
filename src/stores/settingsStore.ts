import { create } from "zustand";

export type OverwritePolicy = "overwrite" | "skip" | "rename";
export type SortField = "name" | "size" | "modified";
export type SortDirection = "asc" | "desc";
export type AppTheme = "dark" | "light";
export type TerminalTheme =
  | "dark"
  | "light"
  | "dracula"
  | "solarized-dark"
  | "gruvbox-dark";

interface SettingsState {
  // Appearance
  appTheme: AppTheme;

  // Transfers
  overwritePolicy: OverwritePolicy;
  autoOpenTransferPanel: boolean;

  // File panes
  showHiddenFiles: boolean;
  sortField: SortField;
  sortDirection: SortDirection;

  // Terminal
  terminalFontSize: number;
  terminalFontFamily: string;
  terminalTheme: TerminalTheme;
  terminalScrollback: number;

  // Connections
  defaultPort: number;

  setAppTheme: (t: AppTheme) => void;
  setOverwritePolicy: (p: OverwritePolicy) => void;
  setAutoOpenTransferPanel: (v: boolean) => void;
  setShowHiddenFiles: (v: boolean) => void;
  setSortField: (f: SortField) => void;
  setSortDirection: (d: SortDirection) => void;
  setTerminalFontSize: (n: number) => void;
  setTerminalFontFamily: (s: string) => void;
  setTerminalTheme: (t: TerminalTheme) => void;
  setTerminalScrollback: (n: number) => void;
  setDefaultPort: (n: number) => void;
}

const STORAGE_KEY = "faro.settings.v1";

type Persisted = Omit<
  SettingsState,
  | "setAppTheme"
  | "setOverwritePolicy"
  | "setAutoOpenTransferPanel"
  | "setShowHiddenFiles"
  | "setSortField"
  | "setSortDirection"
  | "setTerminalFontSize"
  | "setTerminalFontFamily"
  | "setTerminalTheme"
  | "setTerminalScrollback"
  | "setDefaultPort"
>;

const DEFAULTS: Persisted = {
  appTheme: "dark",
  overwritePolicy: "overwrite",
  autoOpenTransferPanel: true,
  showHiddenFiles: false,
  sortField: "name",
  sortDirection: "asc",
  terminalFontSize: 13,
  terminalFontFamily:
    '"JetBrains Mono", "Fira Code", "Cascadia Code", Consolas, monospace',
  terminalTheme: "dark",
  terminalScrollback: 5000,
  defaultPort: 22,
};

function load(): Persisted {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULTS;
    const parsed = JSON.parse(raw) as Partial<Persisted>;
    return { ...DEFAULTS, ...parsed };
  } catch {
    return DEFAULTS;
  }
}

function persist(s: Persisted) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(s));
  } catch {
    // ignore
  }
}

const initial = load();

function mutate<K extends keyof Persisted>(
  set: (fn: (s: SettingsState) => Partial<SettingsState>) => void,
  get: () => SettingsState,
  key: K,
  value: Persisted[K]
) {
  set(() => ({ [key]: value }) as Partial<SettingsState>);
  const s = get();
  persist({
    appTheme: s.appTheme,
    overwritePolicy: s.overwritePolicy,
    autoOpenTransferPanel: s.autoOpenTransferPanel,
    showHiddenFiles: s.showHiddenFiles,
    sortField: s.sortField,
    sortDirection: s.sortDirection,
    terminalFontSize: s.terminalFontSize,
    terminalFontFamily: s.terminalFontFamily,
    terminalTheme: s.terminalTheme,
    terminalScrollback: s.terminalScrollback,
    defaultPort: s.defaultPort,
  });
}

export const useSettings = create<SettingsState>((set, get) => ({
  ...initial,

  setAppTheme: (t) => mutate(set, get, "appTheme", t),
  setOverwritePolicy: (p) => mutate(set, get, "overwritePolicy", p),
  setAutoOpenTransferPanel: (v) =>
    mutate(set, get, "autoOpenTransferPanel", v),
  setShowHiddenFiles: (v) => mutate(set, get, "showHiddenFiles", v),
  setSortField: (f) => mutate(set, get, "sortField", f),
  setSortDirection: (d) => mutate(set, get, "sortDirection", d),
  setTerminalFontSize: (n) => mutate(set, get, "terminalFontSize", n),
  setTerminalFontFamily: (s) => mutate(set, get, "terminalFontFamily", s),
  setTerminalTheme: (t) => mutate(set, get, "terminalTheme", t),
  setTerminalScrollback: (n) => mutate(set, get, "terminalScrollback", n),
  setDefaultPort: (n) => mutate(set, get, "defaultPort", n),
}));

export const TERMINAL_THEMES: Record<
  TerminalTheme,
  { background: string; foreground: string; cursor: string }
> = {
  dark: { background: "#0b0d10", foreground: "#e5e7eb", cursor: "#3b82f6" },
  light: { background: "#fafafa", foreground: "#1f2937", cursor: "#3b82f6" },
  dracula: {
    background: "#282a36",
    foreground: "#f8f8f2",
    cursor: "#ff79c6",
  },
  "solarized-dark": {
    background: "#002b36",
    foreground: "#839496",
    cursor: "#93a1a1",
  },
  "gruvbox-dark": {
    background: "#282828",
    foreground: "#ebdbb2",
    cursor: "#fe8019",
  },
};
