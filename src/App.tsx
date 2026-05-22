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
} from "lucide-react";
import { cn } from "./lib/cn";

export default function App() {
  const activeSessionId = useConnections((s) => s.activeSessionId);
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
          {activeSessionId && terminalOpen && (
            <div className="h-72 border-t border-border">
              <TerminalDock sessionId={activeSessionId} />
            </div>
          )}
          <TransferQueue />
          <StatusBar
            terminalOpen={terminalOpen}
            onToggleTerminal={toggleTerminal}
            transferPanelOpen={transferPanelOpen}
            onToggleTransfers={togglePanel}
            activeTransfers={activeTransfers}
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
        v0.3
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
  return (
    <div className="flex h-5 w-5 items-center justify-center rounded-md text-white shadow-elev-1 btn-accent">
      <svg
        viewBox="0 0 16 16"
        width="12"
        height="12"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M2 4h7" />
        <path d="M2 12h7" />
        <path d="M11 4l3 4-3 4" />
      </svg>
    </div>
  );
}

function StatusBar({
  terminalOpen,
  onToggleTerminal,
  transferPanelOpen,
  onToggleTransfers,
  activeTransfers,
}: {
  terminalOpen: boolean;
  onToggleTerminal: () => void;
  transferPanelOpen: boolean;
  onToggleTransfers: () => void;
  activeTransfers: number;
}) {
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const profiles = useConnections((s) => s.profiles);
  const profile = profiles.find((p) => p.id === activeProfileId);
  const connected = !!activeSessionId && !!profile;

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
        </div>
      ) : (
        <div className="flex items-center gap-1.5 pl-1 text-text-dim">
          <WifiOff size={11} />
          <span>Not connected</span>
        </div>
      )}
      <div className="flex-1" />
      <PillButton
        active={terminalOpen}
        onClick={onToggleTerminal}
        disabled={!activeSessionId}
        icon={<TerminalSquare size={11} />}
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
}: {
  children: React.ReactNode;
  onClick: () => void;
  icon: React.ReactNode;
  active?: boolean;
  disabled?: boolean;
  badge?: number;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
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
