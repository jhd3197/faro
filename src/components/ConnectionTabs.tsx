import { useEffect, useRef, useState } from "react";
import { Plus, X, Cloud, Server, Loader2 } from "lucide-react";
import { useConnections } from "@/stores/connectionsStore";
import { PROTOCOL_LABEL } from "@/lib/types";
import type { ConnectionProfile } from "@/lib/types";
import { cn } from "@/lib/cn";

// A tab per live connection, above the dual-pane browser. Switching tabs swaps
// the focused session (browser, terminal, status bar all follow it). The +
// button connects another saved server without leaving the strip — this is
// what lets the agent bridge several servers at once.
export function ConnectionTabs() {
  const sessions = useConnections((s) => s.sessions);
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const profiles = useConnections((s) => s.profiles);
  const setActiveSession = useConnections((s) => s.setActiveSession);
  const disconnect = useConnections((s) => s.disconnect);

  // Nothing connected yet — the browser shows its own connect prompt.
  if (sessions.length === 0) return null;

  return (
    <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-bg-subtle pl-2 pr-1">
      <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
        {sessions.map((s) => {
          const profile = profiles.find((p) => p.id === s.profileId);
          if (!profile) return null;
          return (
            <ConnectionTab
              key={s.sessionId}
              profile={profile}
              active={s.sessionId === activeSessionId}
              onClick={() => setActiveSession(s.sessionId)}
              onClose={() => disconnect(s.sessionId)}
            />
          );
        })}
      </div>
      <ConnectButton />
    </div>
  );
}

function ConnectionTab({
  profile,
  active,
  onClick,
  onClose,
}: {
  profile: ConnectionProfile;
  active: boolean;
  onClick: () => void;
  onClose: () => void;
}) {
  const isObject = profile.protocol === "s3" || profile.protocol === "azure";
  const ProtoIcon = isObject ? Cloud : Server;
  return (
    <button
      onClick={onClick}
      title={
        isObject
          ? `${profile.name} — ${profile.protocol}://${profile.bucket ?? "?"}`
          : `${profile.name} — ${profile.username}@${profile.host}:${profile.port}`
      }
      className={cn(
        "group/tab flex h-7 shrink-0 items-center gap-2 rounded-md pl-2 pr-1.5 text-[12px] transition-colors",
        active
          ? "bg-bg-panel text-text ring-1 ring-inset ring-accent/40"
          : "text-text-muted hover:bg-bg-hover hover:text-text"
      )}
    >
      <span className="relative flex h-2 w-2 shrink-0 items-center justify-center">
        <span
          className="h-2 w-2 rounded-full"
          style={{ background: profile.color || "rgb(var(--accent))" }}
        />
        {active && (
          <span
            className="absolute inset-0 h-2 w-2 animate-ping rounded-full opacity-40"
            style={{ background: profile.color || "rgb(var(--accent))" }}
          />
        )}
      </span>
      <ProtoIcon size={11} className="shrink-0 text-text-dim" />
      <span className="max-w-[14rem] truncate font-medium">{profile.name}</span>
      <span className="shrink-0 rounded-sm bg-bg-subtle px-1 text-[9px] font-medium uppercase tracking-wider text-text-dim">
        {PROTOCOL_LABEL[profile.protocol]}
      </span>
      <span
        role="button"
        tabIndex={-1}
        aria-label={`Disconnect ${profile.name}`}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        className={cn(
          "ml-0.5 flex h-4 w-4 items-center justify-center rounded text-text-dim hover:bg-bg-hover hover:text-danger",
          active ? "opacity-70" : "opacity-0 group-hover/tab:opacity-100"
        )}
      >
        <X size={11} />
      </span>
    </button>
  );
}

// "+" opens a popover of saved servers that aren't already connected.
function ConnectButton() {
  const profiles = useConnections((s) => s.profiles);
  const sessions = useConnections((s) => s.sessions);
  const connect = useConnections((s) => s.connect);
  const connecting = useConnections((s) => s.connecting);
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  const connectedIds = new Set(sessions.map((s) => s.profileId));
  const available = profiles.filter((p) => !connectedIds.has(p.id));

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open]);

  return (
    <div className="relative shrink-0" ref={wrapRef}>
      <button
        onClick={() => setOpen((v) => !v)}
        disabled={available.length === 0}
        title={
          available.length === 0
            ? "Every saved server is already connected"
            : "Connect another server"
        }
        className={cn(
          "flex h-7 w-7 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-bg-hover hover:text-text disabled:opacity-40 disabled:hover:bg-transparent",
          open && "bg-bg-hover text-text"
        )}
      >
        {connecting ? (
          <Loader2 size={13} className="animate-spin" />
        ) : (
          <Plus size={14} />
        )}
      </button>
      {open && available.length > 0 && (
        <div className="anim-modal absolute right-0 top-9 z-menu w-64 overflow-hidden rounded-md border border-border bg-bg-panel shadow-elev-3">
          <div className="border-b border-border bg-bg-subtle px-3 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
            Connect another server
          </div>
          <div className="max-h-72 overflow-y-auto py-1">
            {available.map((p) => {
              const isObject = p.protocol === "s3" || p.protocol === "azure";
              const ProtoIcon = isObject ? Cloud : Server;
              return (
                <button
                  key={p.id}
                  onClick={() => {
                    setOpen(false);
                    connect(p.id).catch(() => {});
                  }}
                  className="flex w-full items-center gap-2.5 px-3 py-1.5 text-left hover:bg-bg-hover"
                >
                  <span
                    className="h-2 w-2 shrink-0 rounded-full"
                    style={{ background: p.color || "rgb(var(--accent))" }}
                  />
                  <ProtoIcon size={11} className="shrink-0 text-text-dim" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[12px] font-medium">{p.name}</div>
                    <div className="truncate font-mono text-[10px] text-text-dim">
                      {isObject
                        ? `${p.protocol}://${p.bucket ?? "?"}`
                        : `${p.username}@${p.host}`}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
