import { create } from "zustand";

export type OverwritePolicy = "overwrite" | "skip" | "rename";
export type SortField = "name" | "size" | "modified";
export type SortDirection = "asc" | "desc";
export type PaneViewMode = "list" | "details" | "grid";
export type PaneDensity = "comfortable" | "compact";
// "single" = one server-focused pane (default); "dual" = local + remote panes.
export type BrowserLayout = "single" | "dual";
export type AppTheme =
  | "dark"
  | "light"
  | "tokyo"
  | "nord"
  | "dracula"
  | "catppuccin";

export const APP_THEMES: { value: AppTheme; label: string; swatch: string }[] = [
  { value: "dark", label: "Dark", swatch: "rgb(139 127 246)" },
  { value: "light", label: "Light", swatch: "rgb(99 88 220)" },
  { value: "tokyo", label: "Tokyo Night", swatch: "rgb(122 162 247)" },
  { value: "nord", label: "Nord", swatch: "rgb(136 192 208)" },
  { value: "dracula", label: "Dracula", swatch: "rgb(189 147 249)" },
  { value: "catppuccin", label: "Catppuccin", swatch: "rgb(203 166 247)" },
];
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
  /** Where downloads land. Blank = the OS Downloads folder. */
  defaultDownloadFolder: string;
  /** Command/path used to open files for edit-in-place. Blank = OS default app. */
  defaultEditor: string;

  // File panes
  showHiddenFiles: boolean;
  sortField: SortField;
  sortDirection: SortDirection;
  paneViewMode: PaneViewMode;
  paneDensity: PaneDensity;
  browserLayout: BrowserLayout;

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
  setDefaultDownloadFolder: (s: string) => void;
  setDefaultEditor: (s: string) => void;
  setShowHiddenFiles: (v: boolean) => void;
  setSortField: (f: SortField) => void;
  setSortDirection: (d: SortDirection) => void;
  setPaneViewMode: (m: PaneViewMode) => void;
  setPaneDensity: (d: PaneDensity) => void;
  setBrowserLayout: (l: BrowserLayout) => void;
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
  | "setDefaultDownloadFolder"
  | "setDefaultEditor"
  | "setShowHiddenFiles"
  | "setSortField"
  | "setSortDirection"
  | "setPaneViewMode"
  | "setPaneDensity"
  | "setBrowserLayout"
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
  defaultDownloadFolder: "",
  defaultEditor: "",
  showHiddenFiles: false,
  sortField: "name",
  sortDirection: "asc",
  paneViewMode: "details",
  paneDensity: "comfortable",
  browserLayout: "single",
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
    defaultDownloadFolder: s.defaultDownloadFolder,
    defaultEditor: s.defaultEditor,
    showHiddenFiles: s.showHiddenFiles,
    sortField: s.sortField,
    sortDirection: s.sortDirection,
    paneViewMode: s.paneViewMode,
    paneDensity: s.paneDensity,
    browserLayout: s.browserLayout,
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
  setDefaultDownloadFolder: (s) =>
    mutate(set, get, "defaultDownloadFolder", s),
  setDefaultEditor: (s) => mutate(set, get, "defaultEditor", s),
  setShowHiddenFiles: (v) => mutate(set, get, "showHiddenFiles", v),
  setSortField: (f) => mutate(set, get, "sortField", f),
  setSortDirection: (d) => mutate(set, get, "sortDirection", d),
  setPaneViewMode: (m) => mutate(set, get, "paneViewMode", m),
  setPaneDensity: (d) => mutate(set, get, "paneDensity", d),
  setBrowserLayout: (l) => mutate(set, get, "browserLayout", l),
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
