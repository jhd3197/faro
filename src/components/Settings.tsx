import { useEffect, useId, useRef } from "react";
import { X, Palette, ArrowDownUp, FolderTree, TerminalSquare, Plug, Radio, Bot } from "lucide-react";
import { useDialog } from "@/hooks/useDialog";
import {
  useSettings,
  APP_THEMES,
  type AppTheme,
  type OverwritePolicy,
  type SortField,
  type SortDirection,
  type PaneViewMode,
  type PaneDensity,
  type BrowserLayout,
  type TerminalTheme,
} from "@/stores/settingsStore";
import { useBridge } from "@/stores/bridgeStore";
import { open } from "@tauri-apps/plugin-dialog";
import { cn } from "@/lib/cn";

interface Props {
  onClose: () => void;
}

export function Settings({ onClose }: Props) {
  const s = useSettings();

  // Agent Bridge approval policy lives in the Rust backend (it enforces it), so
  // these toggles read/write the same source as the Agent Bridge panel.
  const policy = useBridge((b) => b.status.policy);
  const setPolicy = useBridge((b) => b.setPolicy);
  const refreshBridge = useBridge((b) => b.refresh);
  useEffect(() => {
    refreshBridge();
  }, [refreshBridge]);

  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });

  return (
    <div
      className="fixed inset-0 z-modal flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
        className="anim-modal flex max-h-[85vh] w-[34rem] max-w-[92vw] flex-col rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <div className="flex items-center border-b border-border px-5 py-3.5">
          <span id={titleId} className="text-[15px] font-semibold tracking-tight">Settings</span>
          <div className="flex-1" />
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-muted hover:bg-bg-hover hover:text-text"
          >
            <X size={14} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-2">
          <Section title="Appearance" icon={<Palette size={13} />}>
            <Field
              label="App theme"
              help="Recolors the whole interface — accent, surfaces, scrollbars and focus rings all follow the selected palette."
            >
              <ThemeGrid value={s.appTheme} onChange={s.setAppTheme} />
            </Field>
          </Section>

          <Section title="Transfers" icon={<ArrowDownUp size={13} />}>
            <Field label="When the destination already exists">
              <Segmented<OverwritePolicy>
                value={s.overwritePolicy}
                onChange={s.setOverwritePolicy}
                options={[
                  { value: "overwrite", label: "Overwrite" },
                  { value: "skip", label: "Skip" },
                  { value: "rename", label: "Rename" },
                ]}
              />
              <Help>
                {s.overwritePolicy === "overwrite" &&
                  "Replaces the existing file in place."}
                {s.overwritePolicy === "skip" &&
                  "Leaves the existing file alone, marks the transfer as skipped."}
                {s.overwritePolicy === "rename" &&
                  "Appends _1, _2, … to the destination until a free name is found."}
              </Help>
            </Field>
            <ToggleField
              label="Open the transfer panel automatically"
              help="When a new transfer starts, slide the bottom panel into view."
              checked={s.autoOpenTransferPanel}
              onChange={s.setAutoOpenTransferPanel}
            />
            <Field
              label="Download folder"
              help="Where downloads land. Leave blank to use your system Downloads folder."
            >
              <div className="flex items-center gap-1.5">
                <input
                  value={s.defaultDownloadFolder}
                  onChange={(e) => s.setDefaultDownloadFolder(e.target.value)}
                  placeholder="System Downloads"
                  className="min-w-0 flex-1 rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent"
                />
                <button
                  onClick={async () => {
                    const picked = await open({
                      directory: true,
                      title: "Choose a download folder",
                    });
                    if (typeof picked === "string")
                      s.setDefaultDownloadFolder(picked);
                  }}
                  className="shrink-0 rounded-md border border-border px-2.5 py-1.5 text-sm text-text-muted hover:bg-bg-hover hover:text-text"
                >
                  Browse…
                </button>
                {s.defaultDownloadFolder && (
                  <button
                    onClick={() => s.setDefaultDownloadFolder("")}
                    className="shrink-0 rounded-md border border-border px-2 py-1.5 text-xs text-text-dim hover:text-text"
                    title="Reset to system Downloads"
                  >
                    Reset
                  </button>
                )}
              </div>
            </Field>
          </Section>

          <Section title="File panes" icon={<FolderTree size={13} />}>
            <ToggleField
              label="Show hidden files"
              help="Files and directories whose name starts with a dot."
              checked={s.showHiddenFiles}
              onChange={s.setShowHiddenFiles}
            />
            <Field label="Default sort">
              <Select<SortField>
                value={s.sortField}
                onChange={s.setSortField}
                options={[
                  { value: "name", label: "Name" },
                  { value: "size", label: "Size" },
                  { value: "modified", label: "Modified" },
                ]}
              />
              <Segmented<SortDirection>
                value={s.sortDirection}
                onChange={s.setSortDirection}
                options={[
                  { value: "asc", label: "Asc" },
                  { value: "desc", label: "Desc" },
                ]}
              />
            </Field>
            <Field label="View" help="Switchable from the toolbar too.">
              <Segmented<PaneViewMode>
                value={s.paneViewMode}
                onChange={s.setPaneViewMode}
                options={[
                  { value: "details", label: "Details" },
                  { value: "list", label: "List" },
                  { value: "grid", label: "Grid" },
                ]}
              />
              <Segmented<PaneDensity>
                value={s.paneDensity}
                onChange={s.setPaneDensity}
                options={[
                  { value: "comfortable", label: "Comfortable" },
                  { value: "compact", label: "Compact" },
                ]}
              />
            </Field>
            <Field
              label="Browser layout"
              help="Single = one server-focused pane (Upload button). Split = local and remote side by side."
            >
              <Segmented<BrowserLayout>
                value={s.browserLayout}
                onChange={s.setBrowserLayout}
                options={[
                  { value: "single", label: "Single" },
                  { value: "dual", label: "Split" },
                ]}
              />
            </Field>
            <Field
              label="Default editor"
              help="Command used to open files for edit-in-place (e.g. code). Blank = your OS default app."
            >
              <div className="flex items-center gap-1.5">
                <input
                  value={s.defaultEditor}
                  onChange={(e) => s.setDefaultEditor(e.target.value)}
                  placeholder="OS default app"
                  className="min-w-0 flex-1 rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent"
                />
                <button
                  onClick={() => s.setDefaultEditor("code")}
                  className="shrink-0 rounded-md border border-border px-2.5 py-1.5 text-sm text-text-muted hover:bg-bg-hover hover:text-text"
                  title="Use VS Code (the `code` command)"
                >
                  VS Code
                </button>
                {s.defaultEditor && (
                  <button
                    onClick={() => s.setDefaultEditor("")}
                    className="shrink-0 rounded-md border border-border px-2 py-1.5 text-xs text-text-dim hover:text-text"
                    title="Reset to OS default"
                  >
                    Reset
                  </button>
                )}
              </div>
            </Field>
          </Section>

          <Section title="Terminal" icon={<TerminalSquare size={13} />}>
            <Field label="Font size">
              <NumberInput
                min={8}
                max={32}
                value={s.terminalFontSize}
                onChange={s.setTerminalFontSize}
                suffix="px"
              />
            </Field>
            <Field label="Font family">
              <Select<string>
                value={s.terminalFontFamily}
                onChange={s.setTerminalFontFamily}
                options={[
                  {
                    value:
                      '"JetBrains Mono", "Fira Code", "Cascadia Code", Consolas, monospace',
                    label: "JetBrains Mono",
                  },
                  {
                    value: '"Fira Code", Consolas, monospace',
                    label: "Fira Code",
                  },
                  {
                    value: '"Cascadia Code", Consolas, monospace',
                    label: "Cascadia Code",
                  },
                  {
                    value: "Menlo, Consolas, monospace",
                    label: "Menlo / Consolas",
                  },
                  { value: "monospace", label: "System default" },
                ]}
              />
            </Field>
            <Field label="Theme">
              <Select<TerminalTheme>
                value={s.terminalTheme}
                onChange={s.setTerminalTheme}
                options={[
                  { value: "dark", label: "Dark" },
                  { value: "light", label: "Light" },
                  { value: "dracula", label: "Dracula" },
                  { value: "solarized-dark", label: "Solarized Dark" },
                  { value: "gruvbox-dark", label: "Gruvbox Dark" },
                ]}
              />
              <Help>Applies live to open terminals.</Help>
            </Field>
            <Field label="Scrollback">
              <Select<number>
                value={s.terminalScrollback}
                onChange={s.setTerminalScrollback}
                options={[
                  { value: 1000, label: "1,000 lines" },
                  { value: 5000, label: "5,000 lines" },
                  { value: 10000, label: "10,000 lines" },
                  { value: 50000, label: "50,000 lines" },
                ]}
              />
              <Help>Takes effect on the next opened terminal.</Help>
            </Field>
          </Section>

          <Section title="Connections" icon={<Plug size={13} />}>
            <Field label="Default port for new profiles">
              <NumberInput
                min={1}
                max={65535}
                value={s.defaultPort}
                onChange={s.setDefaultPort}
              />
            </Field>
          </Section>

          <Section title="Agent Bridge" icon={<Radio size={13} />}>
            <ToggleField
              label="Allow all — no prompts"
              help="Let connected AI agents run every request (commands, reads, transfers) on enabled sessions without asking. Most permissive."
              checked={policy.allowAll}
              onChange={(v) => setPolicy({ ...policy, allowAll: v })}
            />
            {!policy.allowAll && (
              <>
                <ToggleField
                  label="Auto-approve read-only operations"
                  help="List directories, read files and search run without asking. Downloads & uploads write to disk, so they still prompt unless Allow all is on."
                  checked={policy.autoRead}
                  onChange={(v) => setPolicy({ ...policy, autoRead: v })}
                />
                <ToggleField
                  label="Auto-approve safe shell commands"
                  help="Read-only commands (ls, cat, df, grep…) run without asking; anything that could change the server still prompts. Best-effort heuristic."
                  checked={policy.autoSafeExec}
                  onChange={(v) => setPolicy({ ...policy, autoSafeExec: v })}
                />
              </>
            )}
            <Help>
              Same setting as the Agent Bridge panel — applies to every
              agent-enabled session and persists across restarts.
            </Help>
          </Section>

          <Section title="Agent" icon={<Bot size={13} />}>
            <Field label="Anthropic API key">
              <input
                type="password"
                value={s.anthropicApiKey}
                onChange={(e) => s.setAnthropicApiKey(e.target.value)}
                placeholder="sk-ant-api03-..."
                className="w-full rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent"
              />
            </Field>
            <Help>
              Used only by the built-in Agent chat. Stored locally; never sent
              to Faro's servers.
            </Help>
          </Section>
        </div>

        <div className="flex items-center justify-between border-t border-border px-5 py-3">
          <span className="text-xs text-text-dim">
            Settings persist locally (browser storage).
          </span>
          <button
            onClick={onClose}
            className="btn-accent rounded-md px-3.5 py-1.5 text-sm font-medium text-white"
          >
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

function Section({
  title,
  icon,
  children,
}: {
  title: string;
  icon?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="border-b border-border-subtle py-4 last:border-b-0">
      <div className="mb-3 flex items-center gap-1.5">
        <span className="text-text-muted">{icon}</span>
        <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-text-muted">
          {title}
        </span>
      </div>
      <div className="space-y-3">{children}</div>
    </div>
  );
}

function Field({
  label,
  help,
  children,
}: {
  label: string;
  help?: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="mb-1.5 text-sm">{label}</div>
      {help && <div className="mb-1.5 text-xs text-text-dim">{help}</div>}
      <div className="flex flex-wrap items-center gap-2">{children}</div>
    </div>
  );
}

function Help({ children }: { children: React.ReactNode }) {
  return <div className="mt-1 w-full text-xs text-text-dim">{children}</div>;
}

function ToggleField({
  label,
  help,
  checked,
  onChange,
}: {
  label: string;
  help?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-start gap-3">
      <div className="min-w-0 flex-1">
        <div className="text-sm">{label}</div>
        {help && <div className="text-xs text-text-dim">{help}</div>}
      </div>
      <Toggle checked={checked} onChange={onChange} />
    </div>
  );
}

function Toggle({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-5 w-9 shrink-0 rounded-full border transition-colors",
        checked
          ? "bg-accent border-accent"
          : "bg-bg-subtle border-border hover:border-text-dim"
      )}
    >
      <span
        className={cn(
          "absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white shadow-elev-1 transition-transform",
          checked ? "translate-x-4" : "translate-x-0"
        )}
      />
    </button>
  );
}

function ThemeGrid({
  value,
  onChange,
}: {
  value: AppTheme;
  onChange: (v: AppTheme) => void;
}) {
  return (
    <div className="grid w-full grid-cols-3 gap-2">
      {APP_THEMES.map((t) => {
        const active = value === t.value;
        return (
          <button
            key={t.value}
            onClick={() => onChange(t.value)}
            className={cn(
              "flex items-center gap-2 rounded-lg border px-2.5 py-2 text-left transition-colors",
              active
                ? "border-accent bg-accent/10"
                : "border-border bg-bg-subtle hover:border-text-dim"
            )}
          >
            <span
              className="h-4 w-4 shrink-0 rounded-full border border-white/10"
              style={{ background: t.swatch }}
            />
            <span
              className={cn(
                "truncate text-xs font-medium",
                active ? "text-text" : "text-text-muted"
              )}
            >
              {t.label}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function Segmented<T extends string>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
}) {
  return (
    <div className="inline-flex rounded-md border border-border bg-bg-subtle p-0.5">
      {options.map((o) => {
        const active = value === o.value;
        return (
          <button
            key={o.value}
            onClick={() => onChange(o.value)}
            className={cn(
              "rounded-[5px] px-3 py-1 text-xs font-medium transition-colors",
              active
                ? "bg-bg-panel text-text shadow-elev-1"
                : "text-text-muted hover:text-text"
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

function Select<T extends string | number>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
}) {
  return (
    <select
      value={value as string | number}
      onChange={(e) => {
        const raw = e.target.value;
        const sample = options[0]?.value;
        const parsed =
          typeof sample === "number" ? (Number(raw) as T) : (raw as T);
        onChange(parsed);
      }}
      className="rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none hover:border-text-dim focus:border-accent"
    >
      {options.map((o) => (
        <option key={String(o.value)} value={o.value as string | number}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

function NumberInput({
  min,
  max,
  value,
  onChange,
  suffix,
}: {
  min: number;
  max: number;
  value: number;
  onChange: (n: number) => void;
  suffix?: string;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <input
        type="number"
        min={min}
        max={max}
        value={value}
        onChange={(e) =>
          onChange(
            Math.max(
              min,
              Math.min(max, parseInt(e.target.value) || min)
            )
          )
        }
        className="w-20 rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent"
      />
      {suffix && <span className="text-xs text-text-dim">{suffix}</span>}
    </div>
  );
}
