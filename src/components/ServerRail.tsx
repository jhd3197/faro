import { useEffect, useMemo, useRef, useState } from "react";
import {
  Plus,
  Download,
  Search,
  Radio,
  Plug,
  Unplug,
  TerminalSquare,
  Pencil,
  Trash2,
  Power,
  Shield,
  X,
  Cloud,
  Server,
  HardDrive,
} from "lucide-react";
import { useConnections } from "@/stores/connectionsStore";
import { useBridge } from "@/stores/bridgeStore";
import { useLayout } from "@/stores/layoutStore";
import { useSettings } from "@/stores/settingsStore";
import { ProfileEditor } from "./ProfileEditor";
import { ImportDialog } from "./ImportDialog";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { Tooltip } from "./ui/Tooltip";
import { monogram } from "@/lib/format";
import {
  PROTOCOL_DEFAULT_PORT,
  PROTOCOL_LABEL,
  type ConnectionProfile,
  type Protocol,
} from "@/lib/types";
import { cn } from "@/lib/cn";

type RowState = "focused" | "connected" | "connecting" | "error" | "idle";

// Servers sort by protocol (SFTP first — the primary use), then by name, so the
// rail order stays learnable as connections come and go.
const GROUP_ORDER: Protocol[] = ["sftp", "ftps", "ftp", "s3", "azure"];

const fallbackColor = "rgb(var(--accent))";

function profileAddress(p: ConnectionProfile): string {
  if (p.protocol === "s3") return `s3://${p.bucket ?? "?"}`;
  if (p.protocol === "azure") return `az://${p.account ?? "?"}/${p.bucket ?? "?"}`;
  const port =
    p.port !== PROTOCOL_DEFAULT_PORT[p.protocol] ? `:${p.port}` : "";
  return `${p.username}@${p.host}${port}`;
}

// The single connection navigator — a Discord-style vertical rail of round
// "server" bubbles. Replaces the old ConnectionManager sidebar and the
// ConnectionTabs strip: one bubble per saved server, status dot, hover name,
// click-to-act menu, plus a dedicated Agent Bridge section below the divider.
export function ServerRail() {
  const {
    profiles,
    sessions,
    activeProfileId,
    activeSessionId,
    connecting,
    error,
    loadProfiles,
    connect,
    disconnect,
    deleteProfile,
    setActiveSession,
    saveProfile,
  } = useConnections();
  const setTerminalOpen = useLayout((s) => s.setTerminalOpen);
  const openDialog = useLayout((s) => s.openDialog);
  const browseLocal = useLayout((s) => s.browseLocal);
  const setBrowseLocal = useLayout((s) => s.setBrowseLocal);
  const browserLayout = useSettings((s) => s.browserLayout);

  const enabledSessions = useBridge((s) => s.status.enabledSessions);
  const bridgeRunning = useBridge((s) => s.status.running);
  const setSessionAccess = useBridge((s) => s.setSessionAccess);
  const startBridge = useBridge((s) => s.start);
  const stopBridge = useBridge((s) => s.stop);

  const [editing, setEditing] = useState<ConnectionProfile | "new" | null>(null);
  const [importing, setImporting] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  // Which profile we last asked to connect — localizes the global
  // connecting/error flags onto the owning bubble (mirrors ConnectionManager).
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);

  // Load profiles once, then auto-connect any flagged servers in sequence so
  // host-key prompts queue through the always-mounted HostKeyModal rather than
  // racing. Runs a single time per launch.
  const autoRan = useRef(false);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      await loadProfiles();
      if (cancelled || autoRan.current) return;
      autoRan.current = true;
      const st = useConnections.getState();
      for (const p of st.profiles) {
        if (!p.autoConnect) continue;
        if (st.sessions.some((s) => s.profileId === p.id)) continue;
        try {
          await st.connect(p.id);
        } catch {
          // connect() already surfaces failures as a toast.
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loadProfiles]);

  // "/" opens the search popover from anywhere (unless already typing).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.isContentEditable)
      )
        return;
      e.preventDefault();
      setSearchOpen(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const connectedIds = useMemo(
    () => new Set(sessions.map((s) => s.profileId)),
    [sessions]
  );
  const sidFor = (id: string) =>
    sessions.find((s) => s.profileId === id)?.sessionId;

  // ProfileIds that currently have a bridge-enabled live session. Keyed via
  // sessionId → profileId so a reconnect (new session UUID) resolves correctly.
  const bridgedIds = useMemo(() => {
    const set = new Set<string>();
    for (const sid of enabledSessions) {
      const sess = sessions.find((s) => s.sessionId === sid);
      if (sess) set.add(sess.profileId);
    }
    return set;
  }, [enabledSessions, sessions]);

  const ordered = useMemo(
    () =>
      [...profiles].sort((a, b) => {
        const ga = GROUP_ORDER.indexOf(a.protocol);
        const gb = GROUP_ORDER.indexOf(b.protocol);
        if (ga !== gb) return ga - gb;
        return a.name.localeCompare(b.name);
      }),
    [profiles]
  );

  const rowState = (p: ConnectionProfile): RowState => {
    const isConnected = connectedIds.has(p.id);
    if (isConnected && p.id === activeProfileId && activeSessionId)
      return "focused";
    if (isConnected) return "connected";
    if (pendingId === p.id && connecting) return "connecting";
    if (pendingId === p.id && error) return "error";
    return "idle";
  };

  const doConnect = (id: string) => {
    setPendingId(id);
    connect(id)
      .then(() => setPendingId((cur) => (cur === id ? null : cur)))
      .catch(() => {});
  };

  const openShell = async (id: string) => {
    if (!connectedIds.has(id)) setPendingId(id);
    try {
      await connect(id);
      setPendingId((cur) => (cur === id ? null : cur));
      setTerminalOpen(true);
    } catch {
      /* surfaced on the bubble */
    }
  };

  const bubbleMenuItems = (p: ConnectionProfile): MenuItem[] => {
    const sid = sidFor(p.id);
    const connected = connectedIds.has(p.id);
    const sessionBridged = sid ? enabledSessions.includes(sid) : false;
    const items: MenuItem[] = [
      {
        label: connected ? "Switch to" : "Connect",
        icon: <Plug size={14} />,
        onClick: () =>
          connected && sid ? setActiveSession(sid) : doConnect(p.id),
      },
    ];
    if (p.protocol === "sftp") {
      items.push({
        label: connected ? "Open shell" : "Connect & open shell",
        icon: <TerminalSquare size={14} />,
        onClick: () => openShell(p.id),
      });
    }
    items.push({
      label: "Properties / Edit…",
      icon: <Pencil size={14} />,
      onClick: () => setEditing(p),
      separatorAfter: true,
    });
    if (sid) {
      items.push({
        label: sessionBridged ? "Disable Agent Bridge" : "Enable Agent Bridge",
        icon: <Radio size={14} />,
        onClick: () => setSessionAccess(sid, !sessionBridged),
      });
    }
    items.push({
      label: p.autoConnect ? "Disable auto-connect" : "Auto-connect on startup",
      icon: <Power size={14} />,
      onClick: () => saveProfile({ ...p, autoConnect: !p.autoConnect }),
    });
    if (connected && sid) {
      items.push({
        label: "Disconnect",
        icon: <Unplug size={14} />,
        onClick: () => disconnect(sid),
        separatorAfter: true,
      });
    }
    items.push({
      label: "Delete",
      icon: <Trash2 size={14} />,
      destructive: true,
      onClick: () => deleteProfile(p.id),
    });
    return items;
  };

  // The reconciled click rule: connected → switch; auto-connect → connect;
  // otherwise open the bubble menu (Connect is the first item).
  const onBubbleClick = (e: React.MouseEvent, p: ConnectionProfile) => {
    setBrowseLocal(false);
    const sid = sidFor(p.id);
    if (sid) {
      setActiveSession(sid);
      return;
    }
    if (p.autoConnect) {
      doConnect(p.id);
      return;
    }
    openMenuAt(e, bubbleMenuItems(p));
  };

  const openMenuAt = (e: React.MouseEvent, items: MenuItem[]) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setMenu({ x: r.right + 6, y: r.top, items });
  };

  const pickFromSearch = (p: ConnectionProfile) => {
    const sid = sidFor(p.id);
    if (sid) setActiveSession(sid);
    else doConnect(p.id);
    setSearchOpen(false);
  };

  // ---- Agent Bridge section ----
  const bridgeSessions = useMemo(
    () =>
      enabledSessions
        .map((sid) => {
          const sess = sessions.find((s) => s.sessionId === sid);
          const profile = sess
            ? profiles.find((p) => p.id === sess.profileId)
            : undefined;
          return profile ? { sid, profile } : null;
        })
        .filter((x): x is { sid: string; profile: ConnectionProfile } => !!x),
    [enabledSessions, sessions, profiles]
  );

  const masterBridgeMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    openMenuAt(e, [
      {
        label: "Open bridge panel",
        icon: <Shield size={14} />,
        onClick: () => openDialog("agentBridge"),
        separatorAfter: true,
      },
      bridgeRunning
        ? { label: "Stop bridge", icon: <Power size={14} />, onClick: stopBridge }
        : { label: "Start bridge", icon: <Power size={14} />, onClick: startBridge },
    ]);
  };

  const bridgeSessionMenu = (
    e: React.MouseEvent,
    sid: string
  ) => {
    e.preventDefault();
    openMenuAt(e, [
      {
        label: "Open bridge panel",
        icon: <Shield size={14} />,
        onClick: () => openDialog("agentBridge"),
        separatorAfter: true,
      },
      {
        label: "Disable agent access",
        icon: <X size={14} />,
        destructive: true,
        onClick: () => setSessionAccess(sid, false),
      },
    ]);
  };

  return (
    <div className="relative flex h-full w-[68px] shrink-0 flex-col border-r border-border bg-bg">
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-2 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        {/* Local files (single-pane layout only) */}
        {browserLayout === "single" && (
          <div className="mb-2 flex flex-col items-center gap-1.5">
            <div className="group relative flex w-full items-center justify-center">
              <span
                aria-hidden
                className={cn(
                  "absolute left-0 top-1/2 w-1 -translate-y-1/2 rounded-r-full transition-all duration-150 motion-reduce:transition-none",
                  browseLocal
                    ? "h-7 bg-accent"
                    : "h-0 bg-text group-hover:h-3"
                )}
              />
              <Tooltip portal side="right" label="Local files">
                <button
                  onClick={() => setBrowseLocal(true)}
                  aria-label="Local files"
                  className={cn(
                    "flex h-11 w-11 items-center justify-center rounded-2xl transition-colors",
                    browseLocal
                      ? "glow-accent bg-accent-soft text-accent"
                      : "bg-bg-subtle text-text-muted hover:bg-bg-hover hover:text-text"
                  )}
                >
                  <HardDrive size={19} />
                </button>
              </Tooltip>
            </div>
            <span className="h-px w-7 rounded-full bg-border" />
          </div>
        )}
        {/* Connections */}
        <div className="flex flex-col items-center gap-1.5">
          {ordered.length === 0 ? (
            <Tooltip portal
          side="right" label="Add your first server">
              <button
                onClick={() => setEditing("new")}
                className="flex h-11 w-11 items-center justify-center rounded-2xl border border-dashed border-border text-text-dim transition-colors hover:border-accent hover:text-accent"
                aria-label="Add a server"
              >
                <Plus size={20} />
              </button>
            </Tooltip>
          ) : (
            ordered.map((p) => (
              <RailBubble
                key={p.id}
                profile={p}
                state={rowState(p)}
                bridged={bridgedIds.has(p.id)}
                onClick={(e) => onBubbleClick(e, p)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  openMenuAt(e, bubbleMenuItems(p));
                }}
              />
            ))
          )}
        </div>

        {/* Divider + Agent Bridge */}
        <div className="mt-3 flex flex-col items-center gap-1.5">
          <span className="mb-1 h-px w-7 rounded-full bg-border" />
          <Tooltip
            portal
          side="right"
            label={`Agent Bridge — ${bridgeRunning ? "running" : "stopped"}`}
          >
            <button
              onClick={() => openDialog("agentBridge")}
              onContextMenu={masterBridgeMenu}
              aria-label="Agent Bridge"
              className={cn(
                "relative flex h-11 w-11 items-center justify-center rounded-2xl bg-bg-subtle transition-colors hover:bg-bg-hover",
                bridgeRunning ? "text-success" : "text-text-dim hover:text-text"
              )}
            >
              <Radio size={19} />
              <StatusDot
                kind={bridgeRunning ? "connected" : "idle"}
              />
            </button>
          </Tooltip>

          {bridgeSessions.map(({ sid, profile }) => (
            <BridgeBubble
              key={sid}
              profile={profile}
              focused={sid === activeSessionId}
              onClick={() => setActiveSession(sid)}
              onContextMenu={(e) => bridgeSessionMenu(e, sid)}
            />
          ))}
        </div>
      </div>

      {/* Pinned controls */}
      <div className="relative flex flex-col items-center gap-1 border-t border-border py-2">
        <RailIconButton
          label="Search servers  ·  /"
          active={searchOpen}
          onClick={() => setSearchOpen((v) => !v)}
        >
          <Search size={17} />
        </RailIconButton>
        <RailIconButton label="New connection" onClick={() => setEditing("new")}>
          <Plus size={19} />
        </RailIconButton>
        <RailIconButton
          label="Import from PuTTY, OpenSSH, FileZilla…"
          onClick={() => setImporting(true)}
        >
          <Download size={16} />
        </RailIconButton>

        {searchOpen && (
          <RailSearch
            profiles={ordered}
            connectedIds={connectedIds}
            onPick={pickFromSearch}
            onClose={() => setSearchOpen(false)}
          />
        )}
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menu.items}
          onClose={() => setMenu(null)}
        />
      )}
      {editing && (
        <ProfileEditor
          profile={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
        />
      )}
      {importing && <ImportDialog onClose={() => setImporting(false)} />}
    </div>
  );
}

function RailBubble({
  profile: p,
  state,
  bridged,
  onClick,
  onContextMenu,
}: {
  profile: ConnectionProfile;
  state: RowState;
  bridged: boolean;
  onClick: (e: React.MouseEvent) => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const focused = state === "focused";
  const connected = state === "focused" || state === "connected";
  const connecting = state === "connecting";
  const isError = state === "error";
  const plaintext = p.protocol === "ftp";
  const color = p.color || fallbackColor;
  const addr = profileAddress(p);

  return (
    <div className="group relative flex w-full items-center justify-center">
      {/* Active / connected pill on the rail's left edge. */}
      <span
        aria-hidden
        className={cn(
          "absolute left-0 top-1/2 w-1 -translate-y-1/2 rounded-r-full transition-all duration-150 motion-reduce:transition-none",
          focused
            ? "h-7 bg-accent"
            : connected
              ? "h-3 bg-text group-hover:h-5"
              : "h-0 bg-text group-hover:h-3"
        )}
      />
      <Tooltip
        portal
          side="right"
        label={
          <span className="flex flex-col gap-0.5 text-left">
            <span className="font-semibold text-text">{p.name}</span>
            <span className="font-mono text-[10px] text-text-dim">{addr}</span>
            <span className="text-[10px] text-text-dim">
              {PROTOCOL_LABEL[p.protocol]}
              {bridged ? " · bridged" : ""}
            </span>
          </span>
        }
      >
        <button
          onClick={onClick}
          onContextMenu={onContextMenu}
          aria-label={p.name}
          title=""
          className={cn(
            "relative flex h-11 w-11 items-center justify-center rounded-full bg-bg-subtle text-[13px] font-bold leading-none transition-[border-radius,box-shadow] duration-150 hover:rounded-2xl motion-reduce:transition-none",
            (focused || connected) && "rounded-2xl",
            focused && "glow-accent",
            isError && "ring-1 ring-danger",
            plaintext && !isError && "ring-1 ring-warning/50"
          )}
        >
          <span style={{ color }}>{monogram(p.name)}</span>
          <StatusDot
            kind={
              connected
                ? "connected"
                : connecting
                  ? "connecting"
                  : isError
                    ? "error"
                    : "idle"
            }
          />
          {bridged && (
            <span className="absolute -right-0.5 -top-0.5 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-bg text-success">
              <Shield size={9} />
            </span>
          )}
        </button>
      </Tooltip>
    </div>
  );
}

// A bridged session, shown in the Agent Bridge section. Deliberately distinct
// from a connection bubble (squarer, green ring, shield) so it never reads as a
// plain server.
function BridgeBubble({
  profile: p,
  focused,
  onClick,
  onContextMenu,
}: {
  profile: ConnectionProfile;
  focused: boolean;
  onClick: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  return (
    <div className="group relative flex w-full items-center justify-center">
      <span
        aria-hidden
        className={cn(
          "absolute left-0 top-1/2 w-1 -translate-y-1/2 rounded-r-full bg-success transition-all duration-150 motion-reduce:transition-none",
          focused ? "h-6" : "h-2 group-hover:h-4"
        )}
      />
      <Tooltip
        portal
          side="right"
        label={
          <span className="flex flex-col gap-0.5 text-left">
            <span className="font-semibold text-text">{p.name}</span>
            <span className="text-[10px] text-success">· bridged</span>
          </span>
        }
      >
        <button
          onClick={onClick}
          onContextMenu={onContextMenu}
          aria-label={`${p.name} (bridged)`}
          className="relative flex h-10 w-10 items-center justify-center rounded-lg bg-bg-subtle text-[12px] font-bold leading-none ring-1 ring-success transition-colors hover:bg-bg-hover"
        >
          <span className="text-success">{monogram(p.name)}</span>
          <span className="absolute -right-0.5 -top-0.5 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-bg text-success">
            <Shield size={9} />
          </span>
        </button>
      </Tooltip>
    </div>
  );
}

// The one place color means connection state — mirrors ConnectionManager's Lamp,
// rendered as a corner badge on the bubble.
function StatusDot({
  kind,
}: {
  kind: "connected" | "connecting" | "error" | "idle";
}) {
  return (
    <span className="absolute -bottom-0.5 -right-0.5 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-bg">
      <span
        className={cn(
          "h-2 w-2 rounded-full",
          kind === "connected"
            ? "bg-success ring-2 ring-success/25"
            : kind === "connecting"
              ? "bg-accent animate-pulse motion-reduce:animate-none"
              : kind === "error"
                ? "bg-danger"
                : "border border-text-dim"
        )}
      />
    </span>
  );
}

function RailIconButton({
  children,
  label,
  active,
  onClick,
}: {
  children: React.ReactNode;
  label: string;
  active?: boolean;
  onClick: () => void;
}) {
  return (
    <Tooltip portal
          side="right" label={label}>
      <button
        onClick={onClick}
        aria-label={label}
        className={cn(
          "flex h-9 w-9 items-center justify-center rounded-xl transition-colors",
          active
            ? "bg-accent-soft text-accent"
            : "text-text-muted hover:bg-bg-hover hover:text-text"
        )}
      >
        {children}
      </button>
    </Tooltip>
  );
}

// Searchable popover over every saved server — the rail's replacement for the
// old sidebar filter. Click a result to focus it (if open) or connect.
function RailSearch({
  profiles,
  connectedIds,
  onPick,
  onClose,
}: {
  profiles: ConnectionProfile[];
  connectedIds: Set<string>;
  onPick: (p: ConnectionProfile) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const wrapRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [onClose]);

  const q = query.trim().toLowerCase();
  const results = q
    ? profiles.filter((p) =>
        [p.name, p.host, p.username, p.bucket, p.account, PROTOCOL_LABEL[p.protocol]]
          .some((s) => !!s && s.toLowerCase().includes(q))
      )
    : profiles;

  return (
    <div
      ref={wrapRef}
      className="anim-modal absolute bottom-2 left-full z-menu ml-2 w-72 overflow-hidden rounded-lg border border-border bg-bg-panel shadow-elev-3"
    >
      <div className="border-b border-border bg-bg-subtle px-2 py-1.5">
        <div className="relative">
          <Search
            size={12}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-text-dim"
          />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && results[0]) onPick(results[0]);
            }}
            placeholder="Search servers…"
            aria-label="Search servers"
            className="w-full rounded-md border border-border-subtle bg-bg px-7 py-1 text-xs outline-none focus:border-accent"
          />
          {query && (
            <button
              onClick={() => setQuery("")}
              className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-text-dim hover:text-text"
              aria-label="Clear search"
            >
              <X size={11} />
            </button>
          )}
        </div>
      </div>
      <div className="max-h-80 overflow-y-auto py-1">
        {results.length === 0 ? (
          <div className="px-3 py-6 text-center text-xs text-text-dim">
            No servers match “{query}”.
          </div>
        ) : (
          results.map((p) => {
            const isObject = p.protocol === "s3" || p.protocol === "azure";
            const ProtoIcon = isObject ? Cloud : Server;
            const online = connectedIds.has(p.id);
            return (
              <button
                key={p.id}
                onClick={() => onPick(p)}
                className="flex w-full items-center gap-2.5 px-3 py-1.5 text-left hover:bg-bg-hover"
              >
                <span
                  className="h-2 w-2 shrink-0 rounded-full"
                  style={{ background: p.color || fallbackColor }}
                />
                <ProtoIcon size={12} className="shrink-0 text-text-dim" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[12px] font-medium">{p.name}</div>
                  <div className="truncate font-mono text-[10px] text-text-dim">
                    {profileAddress(p)}
                  </div>
                </div>
                {online && (
                  <span
                    className="h-1.5 w-1.5 shrink-0 rounded-full bg-success"
                    title="Connected"
                  />
                )}
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}
