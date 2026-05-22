import { useEffect, useState } from "react";
import {
  Plus,
  Trash2,
  Plug,
  PlugZap,
  TerminalSquare,
  Pencil,
} from "lucide-react";
import { useConnections } from "@/stores/connectionsStore";
import { useLayout } from "@/stores/layoutStore";
import { ProfileEditor } from "./ProfileEditor";
import type { ConnectionProfile } from "@/lib/types";
import { cn } from "@/lib/cn";

export function ConnectionManager() {
  const {
    profiles,
    activeProfileId,
    activeSessionId,
    connecting,
    error,
    loadProfiles,
    connect,
    disconnect,
    deleteProfile,
  } = useConnections();
  const setTerminalOpen = useLayout((s) => s.setTerminalOpen);

  const [editing, setEditing] = useState<ConnectionProfile | "new" | null>(
    null
  );

  useEffect(() => {
    loadProfiles();
  }, [loadProfiles]);

  const openShell = async (profileId: string) => {
    try {
      if (activeProfileId !== profileId || !activeSessionId) {
        await connect(profileId);
      }
      setTerminalOpen(true);
    } catch {
      /* error captured on store */
    }
  };

  return (
    <div className="flex h-full w-64 shrink-0 flex-col border-r border-border bg-bg-panel">
      <div className="flex items-center border-b border-border px-3 py-2.5">
        <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-text-muted">
          Connections
        </span>
        <div className="flex-1" />
        <button
          className="rounded-md p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          onClick={() => setEditing("new")}
          title="New connection"
        >
          <Plus size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto py-1">
        {profiles.length === 0 && (
          <div className="px-4 py-10 text-center anim-fade">
            <div className="mx-auto mb-3 flex h-14 w-14 items-center justify-center rounded-2xl bg-accent-soft text-accent">
              <Plug size={24} />
            </div>
            <div className="mb-1 text-[13px] font-medium">
              No connections yet
            </div>
            <div className="mb-4 text-xs text-text-dim">
              Add your first SSH or SFTP server to get started.
            </div>
            <button
              onClick={() => setEditing("new")}
              className="btn-accent inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium text-white"
            >
              <Plus size={11} />
              Add connection
            </button>
          </div>
        )}
        {profiles.map((p) => {
          const isActive = p.id === activeProfileId && !!activeSessionId;
          return (
            <ProfileRow
              key={p.id}
              profile={p}
              active={isActive}
              onConnect={() => {
                if (isActive) disconnect();
                else connect(p.id).catch(() => {});
              }}
              onOpenShell={() => openShell(p.id)}
              onEdit={() => setEditing(p)}
              onDelete={() => deleteProfile(p.id)}
            />
          );
        })}
      </div>

      {connecting && (
        <div className="flex items-center gap-2 border-t border-border px-3 py-2 text-xs text-accent">
          <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent" />
          Connecting…
        </div>
      )}
      {error && (
        <div className="border-t border-border bg-danger-soft px-3 py-2 text-xs text-danger">
          {error}
        </div>
      )}

      {editing && (
        <ProfileEditor
          profile={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}

function ProfileRow({
  profile: p,
  active,
  onConnect,
  onOpenShell,
  onEdit,
  onDelete,
}: {
  profile: ConnectionProfile;
  active: boolean;
  onConnect: () => void;
  onOpenShell: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      className={cn(
        "group mx-1.5 mb-0.5 flex items-center gap-2.5 rounded-md px-2 py-2 transition-colors",
        active
          ? "bg-accent-soft ring-1 ring-inset ring-accent/30"
          : "hover:bg-bg-hover"
      )}
    >
      <div
        className="h-2.5 w-2.5 shrink-0 rounded-full ring-2 ring-bg-panel"
        style={{ background: p.color || "rgb(var(--accent))" }}
        title={p.color ? "tag" : "default"}
      />
      <button
        className="min-w-0 flex-1 text-left"
        onClick={onConnect}
        title={`${p.username}@${p.host}:${p.port}`}
      >
        <div className="truncate text-[13px] font-medium">{p.name}</div>
        <div className="truncate font-mono text-[11px] text-text-dim">
          {p.username}@{p.host}
        </div>
      </button>
      <div className="flex shrink-0 items-center gap-0.5">
        {p.protocol === "sftp" && (
          <IconButton
            onClick={onOpenShell}
            title="Open shell"
            activeColor="accent"
          >
            <TerminalSquare size={13} />
          </IconButton>
        )}
        <IconButton
          onClick={onEdit}
          title="Edit"
          className="opacity-0 group-hover:opacity-100"
        >
          <Pencil size={12} />
        </IconButton>
        <IconButton
          onClick={onDelete}
          title="Delete"
          className="opacity-0 group-hover:opacity-100 hover:text-danger"
        >
          <Trash2 size={12} />
        </IconButton>
        <div className="ml-1">
          {active ? (
            <PlugZap size={14} className="text-emerald-400" />
          ) : (
            <Plug size={13} className="text-text-dim" />
          )}
        </div>
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
  activeColor?: "accent";
}) {
  return (
    <button
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
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
