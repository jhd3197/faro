import { useState } from "react";
import { ConnectionManager } from "./components/ConnectionManager";
import { DualPaneBrowser } from "./components/DualPaneBrowser";
import { TerminalDock } from "./components/Terminal";
import { TransferQueue } from "./components/TransferQueue";
import { Settings } from "./components/Settings";
import { HostKeyModal } from "./components/HostKeyModal";
import { useConnections } from "./stores/connectionsStore";
import { useTransfers } from "./stores/transfersStore";
import { useLayout } from "./stores/layoutStore";
import { useSettings } from "./stores/settingsStore";
import {
  TerminalSquare,
  ArrowDownUp,
  Settings as SettingsIcon,
  Sun,
  Moon,
  Wifi,
  WifiOff,
  Edit3,
  X,
} from "lucide-react";
import { useEditor } from "./stores/editorStore";
import { cn } from "./lib/cn";

export default function App() {
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const profiles = useConnections((s) => s.profiles);
  const activeProfile = profiles.find((p) => p.id === activeProfileId);
  const supportsTerminal = activeProfile?.protocol === "sftp";

  const terminalOpen = useLayout((s) => s.terminalOpen);
  const toggleTerminal = useLayout((s) => s.toggleTerminal);
  const togglePanel = useTransfers((s) => s.togglePanel);
  const transferPanelOpen = useTransfers((s) => s.panelOpen);
  const activeTransfers = useTransfers((s) =>
    Object.values(s.byId).filter(
      (t) => t.status === "transferring" || t.status === "queued"
    ).length
  );
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [editsMenuOpen, setEditsMenuOpen] = useState(false);

  return (
    <div className="flex h-screen w-screen flex-col">
      <TitleBar onOpenSettings={() => setSettingsOpen(true)} />
      {settingsOpen && <Settings onClose={() => setSettingsOpen(false)} />}
      <HostKeyModal />
      <div className="flex flex-1 overflow-hidden">
        <ConnectionManager />
        <div className="flex flex-1 flex-col">
          <div className="flex-1 overflow-hidden">
            <DualPaneBrowser />
          </div>
          {activeSessionId && terminalOpen && supportsTerminal && (
            <div className="h-72 border-t border-border">
              <TerminalDock sessionId={activeSessionId} />
            </div>
          )}
          <TransferQueue />
          <StatusBar
            terminalOpen={terminalOpen && supportsTerminal}
            onToggleTerminal={toggleTerminal}
            terminalAvailable={supportsTerminal}
            transferPanelOpen={transferPanelOpen}
            onToggleTransfers={togglePanel}
            activeTransfers={activeTransfers}
            editsMenuOpen={editsMenuOpen}
            onToggleEditsMenu={() => setEditsMenuOpen((v) => !v)}
            onCloseEditsMenu={() => setEditsMenuOpen(false)}
          />
        </div>
      </div>
    </div>
  );
}

function TitleBar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const appTheme = useSettings((s) => s.appTheme);
  const setAppTheme = useSettings((s) => s.setAppTheme);
  return (
    <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-bg-panel px-3">
      <Logo />
      <span className="text-[13px] font-semibold tracking-tight">Faro</span>
      <span className="hidden text-[11px] text-text-dim sm:inline">
        servers · storage · sessions
      </span>
      <span className="rounded-full bg-accent-soft px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider text-accent">
        v1.2
      </span>
      <div className="flex-1" />
      <button
        onClick={() => setAppTheme(appTheme === "dark" ? "light" : "dark")}
        className="rounded-md p-1.5 text-text-muted hover:bg-bg-hover hover:text-text"
        title={`Switch to ${appTheme === "dark" ? "light" : "dark"} theme`}
      >
        {appTheme === "dark" ? <Sun size={14} /> : <Moon size={14} />}
      </button>
      <button
        onClick={onOpenSettings}
        className="rounded-md p-1.5 text-text-muted hover:bg-bg-hover hover:text-text"
        title="Settings"
      >
        <SettingsIcon size={14} />
      </button>
    </div>
  );
}

function Logo() {
  // Stylised lighthouse silhouette + beam — matches the app icon. The beam
  // uses the accent colour so the logo picks up theme changes naturally.
  return (
    <div className="flex h-5 w-5 items-center justify-center rounded-md text-white shadow-elev-1 btn-accent">
      <svg
        viewBox="0 0 16 16"
        width="13"
        height="13"
        fill="currentColor"
      >
        {/* Beam */}
        <path
          d="M9 5.5 L15 4 L15 8 L9 6.5 Z"
          fill="rgb(252 211 77)"
          opacity="0.85"
        />
        {/* Lantern roof */}
        <path d="M5 4.2 L7 2.5 L9 4.2 Z" />
        {/* Lantern body */}
        <rect x="5.4" y="4.2" width="3.2" height="2" rx="0.2" />
        {/* Tower */}
        <path d="M5 6.2 L9 6.2 L9.4 13.5 L4.6 13.5 Z" />
        {/* Tower stripe */}
        <rect x="4.6" y="9" width="4.8" height="0.7" fill="rgb(20 22 36)" />
      </svg>
    </div>
  );
}

function StatusBar({
  terminalOpen,
  onToggleTerminal,
  terminalAvailable,
  transferPanelOpen,
  onToggleTransfers,
  activeTransfers,
  editsMenuOpen,
  onToggleEditsMenu,
  onCloseEditsMenu,
}: {
  terminalOpen: boolean;
  onToggleTerminal: () => void;
  terminalAvailable: boolean;
  transferPanelOpen: boolean;
  onToggleTransfers: () => void;
  activeTransfers: number;
  editsMenuOpen: boolean;
  onToggleEditsMenu: () => void;
  onCloseEditsMenu: () => void;
}) {
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const profiles = useConnections((s) => s.profiles);
  const profile = profiles.find((p) => p.id === activeProfileId);
  const connected = !!activeSessionId && !!profile;
  const edits = useEditor((s) => s.edits);
  const stopEditing = useEditor((s) => s.stopEditing);
  const editList = Object.values(edits);

  return (
    <div className="flex h-7 shrink-0 items-center gap-2 border-t border-border bg-bg-panel px-2 text-[11px]">
      {connected ? (
        <div className="flex items-center gap-1.5 pl-1">
          <div className="relative">
            <div
              className="h-1.5 w-1.5 rounded-full"
              style={{ background: profile.color || "rgb(var(--accent))" }}
            />
            <div
              className="absolute inset-0 h-1.5 w-1.5 animate-ping rounded-full opacity-50"
              style={{ background: profile.color || "rgb(var(--accent))" }}
            />
          </div>
          <span className="font-medium">{profile.name}</span>
          <span className="font-mono text-text-dim">
            {profile.username}@{profile.host}:{profile.port}
          </span>
          <span className="rounded-sm bg-bg-subtle px-1 text-[9px] font-medium uppercase tracking-wider text-text-dim">
            {profile.protocol}
          </span>
        </div>
      ) : (
        <div className="flex items-center gap-1.5 pl-1 text-text-dim">
          <WifiOff size={11} />
          <span>Not connected</span>
        </div>
      )}
      <div className="flex-1" />
      {editList.length > 0 && (
        <div className="relative">
          <PillButton
            active={editsMenuOpen}
            onClick={onToggleEditsMenu}
            icon={<Edit3 size={11} />}
            badge={editList.length}
          >
            Editing
          </PillButton>
          {editsMenuOpen && (
            <div
              className="anim-modal absolute bottom-7 right-0 z-30 w-80 overflow-hidden rounded-md border border-border bg-bg-panel shadow-elev-3"
              onMouseLeave={onCloseEditsMenu}
            >
              <div className="border-b border-border bg-bg-subtle px-3 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                Live edits — saves upload automatically
              </div>
              {editList.map((e) => {
                const since = e.lastSavedAt
                  ? `${Math.max(1, Math.round((Date.now() - e.lastSavedAt) / 1000))}s ago`
                  : "no saves yet";
                return (
                  <div
                    key={e.editId}
                    className="flex items-start gap-2 border-b border-border-subtle px-3 py-2 last:border-0"
                  >
                    <Edit3
                      size={11}
                      className="mt-0.5 shrink-0 text-accent"
                    />
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-mono text-[11px]">
                        {e.remotePath}
                      </div>
                      {e.lastError ? (
                        <div className="text-[10px] text-danger">
                          {e.lastError}
                        </div>
                      ) : (
                        <div className="text-[10px] text-text-dim">
                          saved {since}
                        </div>
                      )}
                    </div>
                    <button
                      onClick={() => stopEditing(e.editId)}
                      className="rounded p-0.5 text-text-muted hover:bg-bg-hover hover:text-danger"
                      title="Stop editing (closes the watcher)"
                    >
                      <X size={11} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}
      <PillButton
        active={terminalOpen}
        onClick={onToggleTerminal}
        disabled={!activeSessionId || !terminalAvailable}
        icon={<TerminalSquare size={11} />}
        title={
          !terminalAvailable && activeSessionId
            ? "Terminal is only available for SFTP sessions"
            : undefined
        }
      >
        Terminal
      </PillButton>
      <PillButton
        active={transferPanelOpen}
        onClick={onToggleTransfers}
        icon={<ArrowDownUp size={11} />}
        badge={activeTransfers > 0 ? activeTransfers : undefined}
      >
        Transfers
      </PillButton>
      {connected && (
        <div className="flex items-center gap-1 pl-2 pr-1 text-emerald-400">
          <Wifi size={11} />
        </div>
      )}
    </div>
  );
}

function PillButton({
  children,
  onClick,
  icon,
  active,
  disabled,
  badge,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  icon: React.ReactNode;
  active?: boolean;
  disabled?: boolean;
  badge?: number;
  title?: string;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={cn(
        "flex items-center gap-1.5 rounded-md px-2 py-0.5 transition-colors disabled:opacity-40 disabled:cursor-not-allowed",
        active
          ? "bg-accent-soft text-accent"
          : "text-text-muted hover:bg-bg-hover hover:text-text"
      )}
    >
      {icon}
      {children}
      {badge !== undefined && (
        <span className="ml-0.5 rounded-full bg-accent px-1.5 py-0 text-[9px] font-semibold text-white">
          {badge}
        </span>
      )}
    </button>
  );
}
