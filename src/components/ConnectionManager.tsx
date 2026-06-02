import { useEffect, useRef, useState } from "react";
import {
  Plus,
  Trash2,
  TerminalSquare,
  Pencil,
  Search,
  X,
  ChevronDown,
  Unplug,
  Download,
  Plug,
} from "lucide-react";
import { useConnections } from "@/stores/connectionsStore";
import { useLayout } from "@/stores/layoutStore";
import { ProfileEditor } from "./ProfileEditor";
import { ImportDialog } from "./ImportDialog";
import {
  PROTOCOL_LABEL,
  PROTOCOL_DEFAULT_PORT,
  type ConnectionProfile,
  type Protocol,
} from "@/lib/types";
import { cn } from "@/lib/cn";

type RowState = "focused" | "connected" | "connecting" | "error" | "idle";

// Protocol groups render in this fixed order so the index stays learnable.
const GROUP_ORDER: Protocol[] = ["sftp", "ftps", "ftp", "s3", "azure"];

export function ConnectionManager() {
  const {
    profiles,
    activeProfileId,
    activeSessionId,
    sessions,
    connecting,
    error,
    loadProfiles,
    connect,
    disconnect,
    deleteProfile,
  } = useConnections();
  const setTerminalOpen = useLayout((s) => s.setTerminalOpen);

  const [editing, setEditing] = useState<ConnectionProfile | "new" | null>(null);
  const [importing, setImporting] = useState(false);
  const [query, setQuery] = useState("");
  // The profile we last asked to connect — lets us localize the global
  // connecting/error flags onto the owning row without a store change.
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Partial<Record<Protocol, boolean>>>(
    {}
  );
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    loadProfiles();
  }, [loadProfiles]);

  // "/" focuses the filter from anywhere (unless already typing in a field).
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
      searchRef.current?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const connectedIds = new Set(sessions.map((s) => s.profileId));
  const sidFor = (id: string) =>
    sessions.find((s) => s.profileId === id)?.sessionId;

  // connect() doubles as "focus the tab if already open", so clicking any row is
  // safe — it never tears a session down (disconnect is its own control).
  const doConnect = (id: string) => {
    setPendingId(id);
    connect(id)
      .then(() => setPendingId((cur) => (cur === id ? null : cur)))
      .catch(() => {});
  };

  const openShell = async (profileId: string) => {
    try {
      if (!connectedIds.has(profileId)) setPendingId(profileId);
      await connect(profileId);
      setPendingId((cur) => (cur === profileId ? null : cur));
      setTerminalOpen(true);
    } catch {
      /* surfaced on the row */
    }
  };

  const rowState = (p: ConnectionProfile): RowState => {
    const isConnected = connectedIds.has(p.id);
    if (isConnected && p.id === activeProfileId && activeSessionId)
      return "focused";
    if (isConnected) return "connected";
    if (pendingId === p.id && connecting) return "connecting";
    if (pendingId === p.id && error) return "error";
    return "idle";
  };

  const rowProps = (p: ConnectionProfile) => {
    const state = rowState(p);
    return {
      profile: p,
      state,
      errorText: state === "error" ? error : null,
      onConnect: () => doConnect(p.id),
      onDisconnect: () => {
        const sid = sidFor(p.id);
        if (sid) disconnect(sid);
      },
      onOpenShell: () => openShell(p.id),
      onEdit: () => setEditing(p),
      onDelete: () => deleteProfile(p.id),
    };
  };

  const q = query.trim().toLowerCase();
  const searching = q.length > 0;
  const matches = (p: ConnectionProfile) =>
    !searching ||
    [p.name, p.host, p.username, p.bucket, p.account, PROTOCOL_LABEL[p.protocol]]
      .some((s) => !!s && s.toLowerCase().includes(q));
  const filtered = profiles.filter(matches);

  const groups = GROUP_ORDER.map((proto) => ({
    proto,
    items: filtered.filter((p) => p.protocol === proto),
  })).filter((g) => g.items.length > 0);

  const showSearch = profiles.length > 5;

  return (
    <div className="flex h-full w-64 shrink-0 flex-col border-r border-border bg-bg-panel">
      {/* Header */}
      <div className="flex items-center border-b border-border px-3 py-2.5">
        <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-text-muted">
          Connections
        </span>
        {profiles.length > 0 && (
          <span className="ml-1.5 font-mono text-[10px] text-text-dim">
            {profiles.length}
          </span>
        )}
        <div className="flex-1" />
        <button
          className="rounded-md p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          onClick={() => setImporting(true)}
          title="Import from PuTTY, OpenSSH config, FileZilla…"
        >
          <Download size={13} />
        </button>
        <button
          className="rounded-md p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          onClick={() => setEditing("new")}
          title="New connection"
        >
          <Plus size={14} />
        </button>
      </div>

      {/* Filter */}
      {showSearch && (
        <div className="border-b border-border px-2 py-1.5">
          <div className="relative">
            <Search
              size={12}
              className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-text-dim"
            />
            <input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setQuery("");
                  (e.target as HTMLInputElement).blur();
                }
              }}
              placeholder="Filter connections…"
              aria-label="Filter connections"
              className="w-full rounded-md border border-border-subtle bg-bg-subtle py-1 pl-7 pr-6 text-xs outline-none focus:border-accent"
            />
            {query && (
              <button
                onClick={() => setQuery("")}
                title="Clear filter"
                className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-text-dim hover:text-text"
              >
                <X size={11} />
              </button>
            )}
          </div>
        </div>
      )}

      {/* List */}
      <div className="flex-1 overflow-y-auto py-1">
        {profiles.length === 0 ? (
          <EmptyState
            onAdd={() => setEditing("new")}
            onImport={() => setImporting(true)}
          />
        ) : searching ? (
          filtered.length === 0 ? (
            <div className="anim-fade px-4 py-8 text-center text-xs text-text-dim">
              No matches for “{query}”.
              <div className="mt-2">
                <button
                  onClick={() => setQuery("")}
                  className="rounded-md border border-border bg-bg-subtle px-2.5 py-1 text-xs text-text-muted hover:text-text"
                >
                  Clear filter
                </button>
              </div>
            </div>
          ) : (
            [...filtered]
              .sort((a, b) => a.name.localeCompare(b.name))
              .map((p) => <ProfileRow key={p.id} {...rowProps(p)} />)
          )
        ) : (
          groups.map((g) => {
            const isCollapsed = !!collapsed[g.proto];
            const live = g.items.filter((p) => connectedIds.has(p.id)).length;
            return (
              <div key={g.proto}>
                <GroupHeader
                  protocol={g.proto}
                  count={g.items.length}
                  live={live}
                  collapsed={isCollapsed}
                  onToggle={() =>
                    setCollapsed((c) => ({ ...c, [g.proto]: !c[g.proto] }))
                  }
                />
                {!isCollapsed &&
                  g.items.map((p) => <ProfileRow key={p.id} {...rowProps(p)} />)}
              </div>
            );
          })
        )}
      </div>

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

function GroupHeader({
  protocol,
  count,
  live,
  collapsed,
  onToggle,
}: {
  protocol: Protocol;
  count: number;
  live: number;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      onClick={onToggle}
      aria-expanded={!collapsed}
      className="sticky top-0 z-sticky flex w-full items-center gap-1.5 bg-bg-panel px-3 py-1 text-text-muted transition-colors hover:text-text"
    >
      <ChevronDown
        size={11}
        className={cn("shrink-0 transition-transform", collapsed && "-rotate-90")}
      />
      <span className="text-[10px] font-semibold uppercase tracking-[0.08em]">
        {PROTOCOL_LABEL[protocol]}
      </span>
      <span className="font-mono text-[10px] text-text-dim">·{count}</span>
      {live > 0 && (
        <span
          className="ml-auto h-1.5 w-1.5 rounded-full bg-success"
          title={`${live} connected`}
        />
      )}
    </button>
  );
}

function ProfileRow({
  profile: p,
  state,
  errorText,
  onConnect,
  onDisconnect,
  onOpenShell,
  onEdit,
  onDelete,
}: {
  profile: ConnectionProfile;
  state: RowState;
  errorText: string | null;
  onConnect: () => void;
  onDisconnect: () => void;
  onOpenShell: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const focused = state === "focused";
  const connected = state === "focused" || state === "connected";
  const connecting = state === "connecting";
  const isError = state === "error";
  const isObject = p.protocol === "s3" || p.protocol === "azure";

  const addr = isObject
    ? p.protocol === "s3"
      ? `s3://${p.bucket ?? "?"}`
      : `az://${p.account ?? "?"}/${p.bucket ?? "?"}`
    : `${p.username}@${p.host}${
        p.port !== PROTOCOL_DEFAULT_PORT[p.protocol] ? `:${p.port}` : ""
      }`;

  const subline = connecting
    ? "connecting…"
    : isError
      ? errorText ?? "connection failed"
      : addr;

  const bodyTitle = focused
    ? `${p.name} — focused`
    : connected
      ? `${p.name} — connected (click to focus)`
      : connecting
        ? "Connecting…"
        : isError
          ? `${errorText ?? "Failed"} — click to retry`
          : `Connect to ${p.name}`;

  return (
    <div
      className={cn(
        "group relative mx-1.5 mb-px flex items-center rounded-md transition-colors",
        focused
          ? "bg-accent-soft ring-1 ring-inset ring-accent/50"
          : isError
            ? "bg-danger/10"
            : connected
              ? "bg-bg-subtle/50 hover:bg-bg-hover"
              : "hover:bg-bg-hover"
      )}
    >
      <button
        onClick={() => {
          if (!connecting) onConnect();
        }}
        disabled={connecting}
        title={bodyTitle}
        aria-busy={connecting}
        className="flex min-w-0 flex-1 items-center gap-2 px-1.5 py-1.5 text-left disabled:cursor-default"
      >
        <Lamp state={state} />
        <ProtoTag protocol={p.protocol} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-medium text-text">
            {p.name}
          </span>
          <span
            className={cn(
              "block truncate font-mono text-[11px]",
              connecting
                ? "text-accent"
                : isError
                  ? "text-danger"
                  : "text-text-dim"
            )}
          >
            {subline}
          </span>
        </span>
      </button>

      {/* Actions overlay on hover/focus so the row never reflows. */}
      <div className="pointer-events-none absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5 rounded-md bg-bg-hover pl-3 opacity-0 transition-opacity group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100">
        {connected && (
          <IconButton
            onClick={onDisconnect}
            title="Disconnect"
            className="hover:text-danger"
          >
            <Unplug size={13} />
          </IconButton>
        )}
        {p.protocol === "sftp" && (
          <IconButton
            onClick={onOpenShell}
            title={connected ? "Open shell" : "Connect and open shell"}
          >
            <TerminalSquare size={13} />
          </IconButton>
        )}
        <IconButton onClick={onEdit} title="Edit">
          <Pencil size={12} />
        </IconButton>
        <IconButton
          onClick={onDelete}
          title="Delete"
          className="hover:text-danger"
        >
          <Trash2 size={12} />
        </IconButton>
      </div>
    </div>
  );
}

// Status lamp — the one place color means connection state. Hollow=idle,
// success=connected, accent-pulse=connecting, danger=error.
function Lamp({ state }: { state: RowState }) {
  return (
    <span className="flex w-2 shrink-0 items-center justify-center">
      <span
        className={cn(
          "h-1.5 w-1.5 rounded-full",
          state === "focused" || state === "connected"
            ? "bg-success ring-2 ring-success/25"
            : state === "connecting"
              ? "bg-accent animate-pulse"
              : state === "error"
                ? "bg-danger"
                : "border border-text-dim"
        )}
      />
    </span>
  );
}

// Fixed-width mono protocol tag — replaces the old default-purple dot and makes
// the backend legible at a glance. Plaintext FTP is flagged so risk is obvious.
function ProtoTag({ protocol }: { protocol: Protocol }) {
  const plaintext = protocol === "ftp";
  return (
    <span
      className={cn(
        "w-10 shrink-0 rounded-sm py-0.5 text-center font-mono text-[9px] font-semibold uppercase tracking-wide",
        plaintext ? "bg-warning/15 text-warning" : "bg-bg text-text-muted"
      )}
      title={
        plaintext
          ? "FTP transmits credentials in clear text"
          : PROTOCOL_LABEL[protocol]
      }
    >
      {PROTOCOL_LABEL[protocol]}
    </span>
  );
}

function EmptyState({
  onAdd,
  onImport,
}: {
  onAdd: () => void;
  onImport: () => void;
}) {
  return (
    <div className="anim-fade px-4 py-10 text-center">
      <div className="mx-auto mb-3 flex h-11 w-11 items-center justify-center rounded-lg bg-bg-subtle text-text-dim">
        <Plug size={20} />
      </div>
      <div className="mb-1 text-[13px] font-medium">No connections</div>
      <div className="mb-4 text-xs text-text-dim">
        Add a server, or import from OpenSSH, PuTTY, or FileZilla.
      </div>
      <div className="flex flex-col items-center gap-1.5">
        <button
          onClick={onAdd}
          className="btn-accent inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium text-white"
        >
          <Plus size={11} /> New connection
        </button>
        <button
          onClick={onImport}
          className="inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-xs text-text-muted hover:text-text"
        >
          <Download size={11} /> Import
        </button>
      </div>
    </div>
  );
}

function IconButton({
  children,
  onClick,
  title,
  className,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  className?: string;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className={cn(
        "rounded p-1 text-text-muted hover:bg-bg-subtle hover:text-text",
        className
      )}
    >
      {children}
    </button>
  );
}
