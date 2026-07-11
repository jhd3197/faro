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
  | "catppuccin"
  | "gruvbox"
  | "solarized"
  | "onedark"
  | "rosepine"
  | "everforest"
  | "github-light"
  | "monokai"
  | "ayu"
  | "palenight"
  | "github-dark";

// `neutral: true` marks the two plain light/dark palettes — they're surfaced as
// the Light/Dark "mode" toggle in Settings, not as named colors. `dark` drives
// which mode tile lights up. Everything else is a named color palette.
export interface ThemeMeta {
  value: AppTheme;
  label: string;
  swatch: string;
  dark: boolean;
  neutral?: boolean;
}

export const APP_THEMES: ThemeMeta[] = [
  { value: "dark", label: "Dark", swatch: "rgb(139 127 246)", dark: true, neutral: true },
  { value: "light", label: "Light", swatch: "rgb(99 88 220)", dark: false, neutral: true },
  { value: "tokyo", label: "Tokyo Night", swatch: "rgb(122 162 247)", dark: true },
  { value: "nord", label: "Nord", swatch: "rgb(136 192 208)", dark: true },
  { value: "dracula", label: "Dracula", swatch: "rgb(189 147 249)", dark: true },
  { value: "catppuccin", label: "Catppuccin", swatch: "rgb(203 166 247)", dark: true },
  { value: "gruvbox", label: "Gruvbox", swatch: "rgb(254 128 25)", dark: true },
  { value: "solarized", label: "Solarized", swatch: "rgb(38 139 210)", dark: true },
  { value: "onedark", label: "One Dark", swatch: "rgb(97 175 239)", dark: true },
  { value: "rosepine", label: "Rosé Pine", swatch: "rgb(196 167 231)", dark: true },
  { value: "everforest", label: "Everforest", swatch: "rgb(167 192 128)", dark: true },
  { value: "monokai", label: "Monokai", swatch: "rgb(249 38 114)", dark: true },
  { value: "ayu", label: "Ayu Mirage", swatch: "rgb(255 167 89)", dark: true },
  { value: "palenight", label: "Palenight", swatch: "rgb(130 170 255)", dark: true },
  { value: "github-dark", label: "GitHub Dark", swatch: "rgb(47 129 247)", dark: true },
  { value: "github-light", label: "GitHub Light", swatch: "rgb(9 105 218)", dark: false },
];

/** Named color palettes (everything except the plain Light/Dark neutrals). */
export const COLOR_THEMES = APP_THEMES.filter((t) => !t.neutral);
export type TerminalTheme =
  | "dark"
  | "light"
  | "dracula"
  | "solarized-dark"
  | "gruvbox-dark"
  | "onedark"
  | "rosepine"
  | "everforest";

interface SettingsState {
  // Appearance
  appTheme: AppTheme;
  /** Optional accent override (hex). "" = use the theme's own accent. */
  accentColor: string;

  // Transfers
  overwritePolicy: OverwritePolicy;
  /** Show a per-file conflict prompt before overwriting. When false, apply
   *  `overwritePolicy` silently (the pre-prompt behaviour). */
  promptOnOverwrite: boolean;
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
  /** Expand the connection rail into a labeled list (names + addresses) instead
   *  of the compact Discord-style bubble strip. */
  railExpanded: boolean;

  // Terminal
  terminalFontSize: number;
  terminalFontFamily: string;
  terminalTheme: TerminalTheme;
  terminalScrollback: number;
  /** Copy the terminal selection to the clipboard as soon as it's made
   *  (PuTTY-style). On by default. */
  terminalCopyOnSelect: boolean;
  /** Inline ghost-text history suggestions while typing a command (fish/VS
   *  Code style, → to accept). On by default. */
  terminalSuggestions: boolean;

  // Connections
  defaultPort: number;

  // Agent
  /** Anthropic API key for the built-in Agent chat. Stored locally only. */
  anthropicApiKey: string;

  setAppTheme: (t: AppTheme) => void;
  setAccentColor: (hex: string) => void;
  setOverwritePolicy: (p: OverwritePolicy) => void;
  setPromptOnOverwrite: (v: boolean) => void;
  setAutoOpenTransferPanel: (v: boolean) => void;
  setDefaultDownloadFolder: (s: string) => void;
  setDefaultEditor: (s: string) => void;
  setShowHiddenFiles: (v: boolean) => void;
  setSortField: (f: SortField) => void;
  setSortDirection: (d: SortDirection) => void;
  setPaneViewMode: (m: PaneViewMode) => void;
  setPaneDensity: (d: PaneDensity) => void;
  setBrowserLayout: (l: BrowserLayout) => void;
  setRailExpanded: (v: boolean) => void;
  setTerminalFontSize: (n: number) => void;
  setTerminalFontFamily: (s: string) => void;
  setTerminalTheme: (t: TerminalTheme) => void;
  setTerminalScrollback: (n: number) => void;
  setTerminalCopyOnSelect: (v: boolean) => void;
  setTerminalSuggestions: (v: boolean) => void;
  setDefaultPort: (n: number) => void;
  setAnthropicApiKey: (s: string) => void;
}

const STORAGE_KEY = "faro.settings.v1";

type Persisted = Omit<
  SettingsState,
  | "setAppTheme"
  | "setAccentColor"
  | "setOverwritePolicy"
  | "setPromptOnOverwrite"
  | "setAutoOpenTransferPanel"
  | "setDefaultDownloadFolder"
  | "setDefaultEditor"
  | "setShowHiddenFiles"
  | "setSortField"
  | "setSortDirection"
  | "setPaneViewMode"
  | "setPaneDensity"
  | "setBrowserLayout"
  | "setRailExpanded"
  | "setTerminalFontSize"
  | "setTerminalFontFamily"
  | "setTerminalTheme"
  | "setTerminalScrollback"
  | "setTerminalCopyOnSelect"
  | "setTerminalSuggestions"
  | "setDefaultPort"
  | "setAnthropicApiKey"
>;

const DEFAULTS: Persisted = {
  appTheme: "dark",
  accentColor: "",
  overwritePolicy: "overwrite",
  promptOnOverwrite: true,
  autoOpenTransferPanel: true,
  defaultDownloadFolder: "",
  defaultEditor: "",
  showHiddenFiles: false,
  sortField: "name",
  sortDirection: "asc",
  paneViewMode: "details",
  paneDensity: "comfortable",
  browserLayout: "single",
  railExpanded: false,
  terminalFontSize: 13,
  terminalFontFamily:
    '"JetBrains Mono", "Fira Code", "Cascadia Code", Consolas, monospace',
  terminalTheme: "dark",
  terminalScrollback: 5000,
  terminalCopyOnSelect: true,
  terminalSuggestions: true,
  defaultPort: 22,
  anthropicApiKey: "",
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
    accentColor: s.accentColor,
    overwritePolicy: s.overwritePolicy,
    promptOnOverwrite: s.promptOnOverwrite,
    autoOpenTransferPanel: s.autoOpenTransferPanel,
    defaultDownloadFolder: s.defaultDownloadFolder,
    defaultEditor: s.defaultEditor,
    showHiddenFiles: s.showHiddenFiles,
    sortField: s.sortField,
    sortDirection: s.sortDirection,
    paneViewMode: s.paneViewMode,
    paneDensity: s.paneDensity,
    browserLayout: s.browserLayout,
    railExpanded: s.railExpanded,
    terminalFontSize: s.terminalFontSize,
    terminalFontFamily: s.terminalFontFamily,
    terminalTheme: s.terminalTheme,
    terminalScrollback: s.terminalScrollback,
    terminalCopyOnSelect: s.terminalCopyOnSelect,
    terminalSuggestions: s.terminalSuggestions,
    defaultPort: s.defaultPort,
    anthropicApiKey: s.anthropicApiKey,
  });
}

export const useSettings = create<SettingsState>((set, get) => ({
  ...initial,

  setAppTheme: (t) => mutate(set, get, "appTheme", t),
  setAccentColor: (hex) => mutate(set, get, "accentColor", hex),
  setOverwritePolicy: (p) => mutate(set, get, "overwritePolicy", p),
  setPromptOnOverwrite: (v) => mutate(set, get, "promptOnOverwrite", v),
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
  setRailExpanded: (v) => mutate(set, get, "railExpanded", v),
  setTerminalFontSize: (n) => mutate(set, get, "terminalFontSize", n),
  setTerminalFontFamily: (s) => mutate(set, get, "terminalFontFamily", s),
  setTerminalTheme: (t) => mutate(set, get, "terminalTheme", t),
  setTerminalScrollback: (n) => mutate(set, get, "terminalScrollback", n),
  setTerminalCopyOnSelect: (v) =>
    mutate(set, get, "terminalCopyOnSelect", v),
  setTerminalSuggestions: (v) =>
    mutate(set, get, "terminalSuggestions", v),
  setDefaultPort: (n) => mutate(set, get, "defaultPort", n),
  setAnthropicApiKey: (s) => mutate(set, get, "anthropicApiKey", s),
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
  onedark: {
    background: "#282c34",
    foreground: "#abb2bf",
    cursor: "#61afef",
  },
  rosepine: {
    background: "#191724",
    foreground: "#e0def4",
    cursor: "#c4a7e7",
  },
  everforest: {
    background: "#2d353b",
    foreground: "#d3c6aa",
    cursor: "#a7c080",
  },
};
