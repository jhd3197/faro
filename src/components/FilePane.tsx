import { useEffect, useState, useCallback, useRef } from "react";
import {
  ArrowUp,
  RefreshCw,
  Folder,
  FileText,
  Link2,
  ArrowRightLeft,
  FolderPlus,
  Edit3,
  Trash2,
  Copy,
  ShieldCheck,
  ServerOff,
} from "lucide-react";
import { ipc } from "@/lib/ipc";
import type { Capabilities, DirEntry, SessionId } from "@/lib/types";
import { LOCAL_SESSION } from "@/lib/types";
import { useSettings } from "@/stores/settingsStore";
import { cn } from "@/lib/cn";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { PromptModal } from "./PromptModal";
import { ConfirmModal } from "./ConfirmModal";

const DRAG_MIME = "application/x-faro";

interface DragPayload {
  paneId: string;
  entries: DirEntry[];
}

interface Props {
  paneId: string;
  title: string;
  sessionId: SessionId | null;
  path: string;
  onPathChange: (path: string) => void;
  onTransfer?: (entries: DirEntry[]) => void;
  onDrop?: (entries: DirEntry[]) => void;
  transferLabel?: string;
}

type ModalState =
  | { type: "rename"; entry: DirEntry }
  | { type: "mkdir" }
  | { type: "chmod"; entry: DirEntry }
  | { type: "delete"; entries: DirEntry[] }
  | null;

export function FilePane({
  paneId,
  title,
  sessionId,
  path,
  onPathChange,
  onTransfer,
  onDrop,
  transferLabel = "Transfer",
}: Props) {
  const { showHiddenFiles, sortField, sortDirection } = useSettings();

  const [entries, setEntries] = useState<DirEntry[]>([]);
  const [draftPath, setDraftPath] = useState(path);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [isDropTarget, setIsDropTarget] = useState(false);
  const [caps, setCaps] = useState<Capabilities | null>(null);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);
  const [modal, setModal] = useState<ModalState>(null);
  const dragCounter = useRef(0);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setDraftPath(path);
  }, [path]);

  const load = useCallback(
    async (p: string) => {
      if (!sessionId) {
        setEntries([]);
        return;
      }
      setLoading(true);
      setError(null);
      try {
        const list = await ipc.listDirectory(sessionId, p);
        setEntries(list);
        setSelected(new Set());
        setAnchor(null);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    },
    [sessionId]
  );

  useEffect(() => {
    load(path);
  }, [sessionId, path, load]);

  // Pull capabilities once per session so chmod / mkdir / etc. can be hidden
  // for backends that don't support them (notably S3).
  useEffect(() => {
    if (!sessionId) {
      setCaps(null);
      return;
    }
    let cancelled = false;
    ipc.capabilities(sessionId).then(
      (c) => {
        if (!cancelled) setCaps(c);
      },
      () => {
        if (!cancelled) setCaps(null);
      }
    );
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  // Apply filter + sort from settings.
  const visible = entries
    .filter((e) => showHiddenFiles || !e.name.startsWith("."))
    .slice()
    .sort((a, b) => {
      // Directories always come before files regardless of sort.
      if (a.kind === "directory" && b.kind !== "directory") return -1;
      if (a.kind !== "directory" && b.kind === "directory") return 1;
      let cmp = 0;
      if (sortField === "size") {
        cmp = a.size - b.size;
      } else if (sortField === "modified") {
        cmp = (a.modified ?? 0) - (b.modified ?? 0);
      } else {
        cmp = a.name.localeCompare(b.name);
      }
      return sortDirection === "asc" ? cmp : -cmp;
    });

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setSelected(new Set());
        setAnchor(null);
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        setSelected(new Set(visible.map((x) => x.path)));
      } else if (e.key === "Delete" && selected.size > 0) {
        const items = visible.filter((v) => selected.has(v.path));
        if (items.length > 0) setModal({ type: "delete", entries: items });
      }
    };
    el.addEventListener("keydown", handler);
    return () => el.removeEventListener("keydown", handler);
  }, [visible, selected]);

  const goUp = () => {
    const isLocal = sessionId === LOCAL_SESSION;
    const parts = path.split(/[/\\]/).filter(Boolean);
    if (parts.length <= 1) {
      onPathChange(isLocal ? path : "/");
      return;
    }
    parts.pop();
    const next =
      isLocal && /^[a-zA-Z]:/.test(path)
        ? parts.join("\\")
        : "/" + parts.join("/");
    onPathChange(next || "/");
  };

  const joinPath = (parent: string, name: string) => {
    const isLocal = sessionId === LOCAL_SESSION;
    const sep = isLocal && /^[a-zA-Z]:/.test(parent) ? "\\" : "/";
    if (parent.endsWith(sep) || parent.endsWith("/")) return `${parent}${name}`;
    return `${parent}${sep}${name}`;
  };

  const onRowClick = (entry: DirEntry, e: React.MouseEvent) => {
    if (e.shiftKey && anchor) {
      const idxA = visible.findIndex((x) => x.path === anchor);
      const idxB = visible.findIndex((x) => x.path === entry.path);
      if (idxA >= 0 && idxB >= 0) {
        const [lo, hi] = idxA < idxB ? [idxA, idxB] : [idxB, idxA];
        const next = new Set<string>();
        for (let i = lo; i <= hi; i++) next.add(visible[i].path);
        setSelected(next);
      }
    } else if (e.ctrlKey || e.metaKey) {
      setSelected((prev) => {
        const next = new Set(prev);
        if (next.has(entry.path)) next.delete(entry.path);
        else next.add(entry.path);
        return next;
      });
      setAnchor(entry.path);
    } else {
      setSelected(new Set([entry.path]));
      setAnchor(entry.path);
    }
  };

  const onRowActivate = (entry: DirEntry) => {
    if (entry.kind === "directory") {
      onPathChange(entry.path);
      return;
    }
    onTransfer?.([entry]);
  };

  const transferSelection = () => {
    const items = visible.filter((e) => selected.has(e.path));
    if (items.length === 0) return;
    onTransfer?.(items);
  };

  // --- Drag & drop ---

  const onRowDragStart = (e: React.DragEvent, entry: DirEntry) => {
    const dragSet = selected.has(entry.path)
      ? visible.filter((x) => selected.has(x.path))
      : [entry];
    if (!selected.has(entry.path)) {
      setSelected(new Set([entry.path]));
      setAnchor(entry.path);
    }
    const payload: DragPayload = { paneId, entries: dragSet };
    e.dataTransfer.setData(DRAG_MIME, JSON.stringify(payload));
    e.dataTransfer.effectAllowed = "copy";
  };

  const isOurDrag = (e: React.DragEvent) =>
    Array.from(e.dataTransfer.types).includes(DRAG_MIME);

  const onPaneDragEnter = (e: React.DragEvent) => {
    if (!isOurDrag(e)) return;
    e.preventDefault();
    dragCounter.current++;
    setIsDropTarget(true);
  };
  const onPaneDragOver = (e: React.DragEvent) => {
    if (!isOurDrag(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  };
  const onPaneDragLeave = (e: React.DragEvent) => {
    if (!isOurDrag(e)) return;
    dragCounter.current = Math.max(0, dragCounter.current - 1);
    if (dragCounter.current === 0) setIsDropTarget(false);
  };
  const onPaneDrop = (e: React.DragEvent) => {
    if (!isOurDrag(e)) return;
    e.preventDefault();
    dragCounter.current = 0;
    setIsDropTarget(false);
    const raw = e.dataTransfer.getData(DRAG_MIME);
    if (!raw) return;
    let payload: DragPayload;
    try {
      payload = JSON.parse(raw);
    } catch {
      return;
    }
    if (payload.paneId === paneId) return;
    if (!payload.entries.length) return;
    onDrop?.(payload.entries);
  };

  // --- Context menu ---

  const copyPaths = (items: DirEntry[]) => {
    navigator.clipboard.writeText(items.map((e) => e.path).join("\n"));
  };
  const copyNames = (items: DirEntry[]) => {
    navigator.clipboard.writeText(items.map((e) => e.name).join("\n"));
  };

  const openRowMenu = (e: React.MouseEvent, entry: DirEntry) => {
    e.preventDefault();
    e.stopPropagation();
    if (!selected.has(entry.path)) {
      setSelected(new Set([entry.path]));
      setAnchor(entry.path);
    }
    const selectedItems = selected.has(entry.path)
      ? visible.filter((v) => selected.has(v.path))
      : [entry];
    const single = selectedItems.length === 1 ? selectedItems[0] : null;

    const items: MenuItem[] = [];
    if (single) {
      items.push({
        label: single.kind === "directory" ? "Open" : "Transfer to other pane",
        onClick: () => onRowActivate(single),
      });
      items.push({
        label: "Rename",
        icon: <Edit3 size={12} />,
        onClick: () => setModal({ type: "rename", entry: single }),
        separatorAfter: true,
      });
    } else {
      items.push({
        label: `Transfer ${selectedItems.length} items to other pane`,
        onClick: () => onTransfer?.(selectedItems),
        separatorAfter: true,
      });
    }
    items.push({
      label: `Delete${selectedItems.length > 1 ? ` (${selectedItems.length})` : ""}`,
      icon: <Trash2 size={12} />,
      destructive: true,
      onClick: () => setModal({ type: "delete", entries: selectedItems }),
      separatorAfter: true,
    });
    items.push({
      label: `Copy path${selectedItems.length > 1 ? `s (${selectedItems.length})` : ""}`,
      icon: <Copy size={12} />,
      onClick: () => copyPaths(selectedItems),
    });
    items.push({
      label: `Copy name${selectedItems.length > 1 ? `s (${selectedItems.length})` : ""}`,
      onClick: () => copyNames(selectedItems),
      separatorAfter: single?.kind === "file",
    });
    if (
      single?.kind === "file" &&
      sessionId !== LOCAL_SESSION &&
      caps?.canChmod !== false
    ) {
      items.push({
        label: "Change permissions (chmod)…",
        icon: <ShieldCheck size={12} />,
        onClick: () => setModal({ type: "chmod", entry: single }),
      });
    }
    setMenu({ x: e.clientX, y: e.clientY, items });
  };

  const openPaneMenu = (e: React.MouseEvent) => {
    if (!sessionId) return;
    e.preventDefault();
    const items: MenuItem[] = [];
    if (caps?.hasDirectories !== false) {
      items.push({
        label: "New folder…",
        icon: <FolderPlus size={12} />,
        onClick: () => setModal({ type: "mkdir" }),
        separatorAfter: true,
      });
    }
    items.push({
      label: "Refresh",
      icon: <RefreshCw size={12} />,
      onClick: () => load(path),
    });
    setMenu({ x: e.clientX, y: e.clientY, items });
  };

  // --- File op handlers ---

  const doRename = async (entry: DirEntry, newName: string) => {
    if (!sessionId) return;
    const parentEnd = Math.max(
      entry.path.lastIndexOf("/"),
      entry.path.lastIndexOf("\\")
    );
    const parent = entry.path.slice(0, parentEnd);
    const to = joinPath(parent, newName);
    try {
      await ipc.renamePath(sessionId, entry.path, to);
      await load(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const doDelete = async (items: DirEntry[]) => {
    if (!sessionId) return;
    try {
      for (const it of items) {
        await ipc.deletePath(sessionId, it.path, it.kind === "directory");
      }
      await load(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const doMkdir = async (name: string) => {
    if (!sessionId) return;
    try {
      await ipc.createDirectory(sessionId, joinPath(path, name));
      await load(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const doChmod = async (entry: DirEntry, modeText: string) => {
    if (!sessionId) return;
    const mode = parseInt(modeText, 8);
    if (Number.isNaN(mode)) {
      setError(`invalid mode "${modeText}" — use octal like 755`);
      return;
    }
    try {
      await ipc.chmodPath(sessionId, entry.path, mode);
      await load(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const selectionCount = selected.size;

  return (
    <div
      ref={containerRef}
      tabIndex={0}
      onDragEnter={onPaneDragEnter}
      onDragOver={onPaneDragOver}
      onDragLeave={onPaneDragLeave}
      onDrop={onPaneDrop}
      onContextMenu={openPaneMenu}
      className={cn(
        "flex h-full flex-1 flex-col bg-bg-panel outline-none transition-colors",
        isDropTarget && "ring-2 ring-inset ring-accent/60 bg-accent/5"
      )}
    >
      <div className="flex items-center gap-1 border-b border-border bg-bg-subtle px-2 py-1.5">
        <span className="text-xs font-semibold uppercase tracking-wider text-text-muted">
          {title}
        </span>
        {selectionCount > 0 && (
          <span className="rounded bg-accent/20 px-1.5 py-0.5 text-[10px] font-medium text-accent">
            {selectionCount} selected
          </span>
        )}
        {isDropTarget && (
          <span className="rounded bg-accent px-1.5 py-0.5 text-[10px] font-medium text-white">
            drop to {transferLabel.toLowerCase()}
          </span>
        )}
        <div className="flex-1" />
        {selectionCount > 0 && onTransfer && (
          <button
            onClick={transferSelection}
            className="flex items-center gap-1 rounded bg-accent px-2 py-0.5 text-[11px] font-medium text-white hover:bg-accent-hover"
            title={`${transferLabel} ${selectionCount} item(s) to the other pane`}
          >
            <ArrowRightLeft size={11} />
            {transferLabel} {selectionCount}
          </button>
        )}
        {caps?.hasDirectories !== false && (
          <button
            onClick={() => setModal({ type: "mkdir" })}
            disabled={!sessionId}
            className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-40"
            title="New folder"
          >
            <FolderPlus size={13} />
          </button>
        )}
        <button
          onClick={goUp}
          disabled={!sessionId}
          className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-40"
          title="Up"
        >
          <ArrowUp size={13} />
        </button>
        <button
          onClick={() => load(path)}
          disabled={!sessionId}
          className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-40"
          title="Refresh"
        >
          <RefreshCw size={13} />
        </button>
      </div>

      <div className="border-b border-border px-2 py-1">
        <input
          value={draftPath}
          onChange={(e) => setDraftPath(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onPathChange(draftPath);
          }}
          disabled={!sessionId}
          className="w-full rounded border border-border bg-bg-subtle px-2 py-1 font-mono text-xs outline-none focus:border-accent disabled:opacity-50"
          placeholder="/"
        />
      </div>

      <div className="flex-1 overflow-y-auto">
        {!sessionId && (
          <div className="flex h-full flex-col items-center justify-center px-3 py-12 text-center anim-fade">
            <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-xl bg-bg-subtle text-text-dim">
              <ServerOff size={20} />
            </div>
            <div className="text-xs text-text-dim">
              No connection.
              <br />
              Click a profile in the sidebar to connect.
            </div>
          </div>
        )}
        {loading && (
          <div className="px-3 py-2 text-xs text-text-muted">Loading…</div>
        )}
        {error && (
          <div className="px-3 py-2 text-xs text-danger">
            {error}{" "}
            <button
              onClick={() => setError(null)}
              className="ml-1 underline opacity-60 hover:opacity-100"
            >
              dismiss
            </button>
          </div>
        )}
        {visible.map((entry) => (
          <Row
            key={entry.path}
            entry={entry}
            selected={selected.has(entry.path)}
            onClick={(e) => onRowClick(entry, e)}
            onActivate={() => onRowActivate(entry)}
            onDragStart={(e) => onRowDragStart(e, entry)}
            onContextMenu={(e) => openRowMenu(e, entry)}
          />
        ))}
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menu.items}
          onClose={() => setMenu(null)}
        />
      )}

      {modal?.type === "rename" && (
        <PromptModal
          title={`Rename ${modal.entry.name}`}
          label="New name"
          initialValue={modal.entry.name}
          okLabel="Rename"
          onClose={() => setModal(null)}
          onSubmit={(v) => doRename(modal.entry, v)}
        />
      )}
      {modal?.type === "mkdir" && (
        <PromptModal
          title="New folder"
          label={`In ${path}`}
          initialValue=""
          okLabel="Create"
          onClose={() => setModal(null)}
          onSubmit={(v) => doMkdir(v)}
        />
      )}
      {modal?.type === "chmod" && (
        <PromptModal
          title={`Permissions for ${modal.entry.name}`}
          label="Octal mode (e.g. 755, 644)"
          initialValue={
            modal.entry.mode
              ? (modal.entry.mode & 0o777).toString(8)
              : "644"
          }
          okLabel="Apply"
          onClose={() => setModal(null)}
          onSubmit={(v) => doChmod(modal.entry, v)}
        />
      )}
      {modal?.type === "delete" && (
        <ConfirmModal
          title={
            modal.entries.length === 1
              ? `Delete ${modal.entries[0].name}?`
              : `Delete ${modal.entries.length} items?`
          }
          message={
            modal.entries.length === 1
              ? `Path: ${modal.entries[0].path}${
                  modal.entries[0].kind === "directory"
                    ? "\nThis directory and all its contents will be removed."
                    : ""
                }`
              : modal.entries
                  .slice(0, 5)
                  .map((e) => e.path)
                  .join("\n") +
                (modal.entries.length > 5
                  ? `\n…and ${modal.entries.length - 5} more`
                  : "")
          }
          destructive
          confirmLabel="Delete"
          onClose={() => setModal(null)}
          onConfirm={() => doDelete(modal.entries)}
        />
      )}
    </div>
  );
}

function Row({
  entry,
  selected,
  onClick,
  onActivate,
  onDragStart,
  onContextMenu,
}: {
  entry: DirEntry;
  selected: boolean;
  onClick: (e: React.MouseEvent) => void;
  onActivate: () => void;
  onDragStart: (e: React.DragEvent) => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const Icon =
    entry.kind === "directory"
      ? Folder
      : entry.kind === "symlink"
        ? Link2
        : FileText;
  return (
    <div
      onClick={onClick}
      onDoubleClick={onActivate}
      onContextMenu={onContextMenu}
      draggable
      onDragStart={onDragStart}
      className={cn(
        "flex cursor-default select-none items-center gap-2 border-b border-border-subtle px-3 py-1 text-sm",
        selected ? "bg-accent/15 hover:bg-accent/20" : "hover:bg-bg-hover"
      )}
      title={
        entry.kind === "file"
          ? "Drag to other pane • Double-click to transfer • Right-click for more"
          : "Drag, double-click to open, right-click for more"
      }
    >
      <Icon
        size={13}
        className={
          entry.kind === "directory" ? "text-accent" : "text-text-muted"
        }
      />
      <span className="flex-1 truncate">{entry.name}</span>
      <span className="text-xs text-text-dim">
        {entry.kind === "file" ? fmtSize(entry.size) : ""}
      </span>
    </div>
  );
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
