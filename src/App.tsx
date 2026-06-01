import { useState } from "react";
import { ConnectionManager } from "./components/ConnectionManager";
import { DualPaneBrowser } from "./components/DualPaneBrowser";
import { TerminalDock } from "./components/Terminal";
import { TransferQueue } from "./components/TransferQueue";
import { Settings } from "./components/Settings";
import { HostKeyModal } from "./components/HostKeyModal";
import { TitleBar } from "./components/TitleBar";
import { ProfileEditor } from "./components/ProfileEditor";
import { ImportDialog } from "./components/ImportDialog";
import { AboutDialog } from "./components/AboutDialog";
import { useConnections } from "./stores/connectionsStore";
import { useTransfers } from "./stores/transfersStore";
import { useLayout } from "./stores/layoutStore";
import {
  TerminalSquare,
  ArrowDownUp,
  Wifi,
  WifiOff,
  Edit3,
  X,
  Bell,
  CheckCircle2,
  AlertCircle,
  Info,
  AlertTriangle,
  Radio,
} from "lucide-react";
import { useEditor } from "./stores/editorStore";
import { useToasts, type ToastVariant } from "./stores/toastStore";
import { useBridge } from "./stores/bridgeStore";
import { Toaster } from "./components/ui/Toaster";
import { CommandPalette } from "./components/CommandPalette";
import { KeyboardShortcutsDialog } from "./components/KeyboardShortcutsDialog";
import { AgentBridge, AgentBridgeHost } from "./components/AgentBridge";
import { AgentConsole } from "./components/AgentConsole";
import { useShortcuts } from "./hooks/useShortcuts";
import { relTime } from "./lib/format";
import { cn } from "./lib/cn";

export default function App() {
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const profiles = useConnections((s) => s.profiles);
  const activeProfile = profiles.find((p) => p.id === activeProfileId);
  const supportsTerminal = activeProfile?.protocol === "sftp";

  const terminalOpen = useLayout((s) => s.terminalOpen);
  const toggleTerminal = useLayout((s) => s.toggleTerminal);
  const dialog = useLayout((s) => s.dialog);
  const closeDialog = useLayout((s) => s.closeDialog);
  const togglePanel = useTransfers((s) => s.togglePanel);
  const transferPanelOpen = useTransfers((s) => s.panelOpen);
  const activeTransfers = useTransfers((s) =>
    Object.values(s.byId).filter(
      (t) => t.status === "transferring" || t.status === "queued"
    ).length
  );
  const [editsMenuOpen, setEditsMenuOpen] = useState(false);

  useShortcuts();

  return (
    <div className="flex h-screen w-screen flex-col">
      <TitleBar />
      {dialog === "settings" && <Settings onClose={closeDialog} />}
      {dialog === "newConnection" && (
        <ProfileEditor profile={null} onClose={closeDialog} />
      )}
      {dialog === "import" && <ImportDialog onClose={closeDialog} />}
      {dialog === "about" && <AboutDialog onClose={closeDialog} />}
      {dialog === "agentBridge" && <AgentBridge onClose={closeDialog} />}
      {dialog === "agentConsole" && <AgentConsole onClose={closeDialog} />}
      <HostKeyModal />
      <Toaster />
      <AgentBridgeHost />
      <CommandPalette />
      <KeyboardShortcutsDialog />
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

  const history = useToasts((s) => s.history);
  const unread = useToasts((s) => s.unreadCount);
  const markAllRead = useToasts((s) => s.markAllRead);
  const clearHistory = useToasts((s) => s.clearHistory);
  const [notifOpen, setNotifOpen] = useState(false);

  const bridgeRunning = useBridge((s) => s.status.running);
  const openDialog = useLayout((s) => s.openDialog);

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
          <span className="max-w-[18rem] truncate font-medium" title={profile.name}>
            {profile.name}
          </span>
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
      <div className="relative">
        <PillButton
          active={notifOpen}
          onClick={() => {
            const next = !notifOpen;
            setNotifOpen(next);
            if (next) markAllRead();
          }}
          icon={<Bell size={11} />}
          badge={unread > 0 ? unread : undefined}
          title="Notifications"
        />
        {notifOpen && (
          <div
            className="anim-modal absolute bottom-7 right-0 z-30 w-80 overflow-hidden rounded-md border border-border bg-bg-panel shadow-elev-3"
            onMouseLeave={() => setNotifOpen(false)}
          >
            <div className="flex items-center justify-between border-b border-border bg-bg-subtle px-3 py-1.5">
              <span className="text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                Notifications
              </span>
              {history.length > 0 && (
                <button
                  onClick={clearHistory}
                  className="text-[10px] text-text-dim hover:text-text"
                >
                  Clear
                </button>
              )}
            </div>
            {history.length === 0 ? (
              <div className="px-3 py-6 text-center text-[11px] text-text-dim">
                No notifications yet
              </div>
            ) : (
              <div className="max-h-72 overflow-y-auto">
                {history.slice(0, 50).map((n) => (
                  <div
                    key={n.id}
                    className="flex items-start gap-2 border-b border-border-subtle px-3 py-2 last:border-0"
                  >
                    <NotifIcon variant={n.variant} />
                    <div className="min-w-0 flex-1">
                      <div className="text-[11px] font-medium">{n.title}</div>
                      {n.message && (
                        <div className="truncate text-[10px] text-text-dim">
                          {n.message}
                        </div>
                      )}
                    </div>
                    <span className="shrink-0 text-[9px] text-text-dim">
                      {relTime(n.createdAt)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
      <PillButton
        active={bridgeRunning}
        onClick={() => openDialog("agentBridge")}
        icon={
          <Radio
            size={11}
            className={bridgeRunning ? "text-emerald-400" : undefined}
          />
        }
        title="Agent Bridge — let a local AI agent run commands on your server"
      >
        Bridge
      </PillButton>
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
  children?: React.ReactNode;
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

function NotifIcon({ variant }: { variant: ToastVariant }) {
  const map = {
    info: { Icon: Info, cls: "text-accent" },
    success: { Icon: CheckCircle2, cls: "text-emerald-400" },
    error: { Icon: AlertCircle, cls: "text-danger" },
    warning: { Icon: AlertTriangle, cls: "text-amber-400" },
  } as const;
  const { Icon, cls } = map[variant];
  return <Icon size={12} className={cn("mt-0.5 shrink-0", cls)} />;
}
