import { create } from "zustand";
import { ipc, onTransferEvent } from "@/lib/ipc";
import { useSettings } from "./settingsStore";
import { toast } from "./toastStore";
import { useConflicts, type ConflictDecision } from "./conflictStore";
import { baseName } from "@/lib/format";
import {
  LOCAL_SESSION,
  type SessionId,
  type Transfer,
  type OverwritePolicy,
} from "@/lib/types";

/** A file/folder queued for transfer. Lighter than DirEntry so native-picker
 *  paths (which have no metadata) can be enqueued too. */
export interface TransferItem {
  path: string;
  kind: "file" | "directory";
  size?: number;
  modified?: number;
}

/** Map a DirEntry (kind may be symlink/other) to a TransferItem (file/dir). */
export function toTransferItem(e: {
  path: string;
  kind: string;
  size?: number;
  modified?: number | null;
}): TransferItem {
  return {
    path: e.path,
    kind: e.kind === "directory" ? "directory" : "file",
    size: e.size,
    modified: e.modified ?? undefined,
  };
}

interface TransfersState {
  byId: Record<string, Transfer>;
  panelOpen: boolean;

  togglePanel: () => void;
  setPanelOpen: (open: boolean) => void;

  initListeners: () => Promise<() => void>;
  loadInitial: () => Promise<void>;

  // Low-level starts. `policy` overrides the default overwrite policy for this
  // one transfer (used after the user answers the conflict prompt).
  download: (
    sessionId: SessionId,
    remotePath: string,
    localDir: string,
    policy?: OverwritePolicy
  ) => Promise<string>;
  upload: (
    sessionId: SessionId,
    localPath: string,
    remoteDir: string,
    policy?: OverwritePolicy
  ) => Promise<string>;
  downloadDir: (
    sessionId: SessionId,
    remoteDir: string,
    localDir: string,
    policy?: OverwritePolicy
  ) => Promise<string[]>;
  uploadDir: (
    sessionId: SessionId,
    localDir: string,
    remoteDir: string,
    policy?: OverwritePolicy
  ) => Promise<string[]>;
  // Batch entry points: detect name collisions against the destination and
  // prompt (FileZilla-style) before starting each transfer.
  enqueueDownloads: (
    sessionId: SessionId,
    items: TransferItem[],
    localDir: string
  ) => Promise<void>;
  enqueueUploads: (
    sessionId: SessionId,
    items: TransferItem[],
    remoteDir: string
  ) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  clearFinished: () => void;
}

/** Map a conflict action to the backend overwrite policy. "skip"/"cancel" are
 *  handled by the caller (they don't start a transfer), so this only covers the
 *  two that do. */
function actionToPolicy(action: ConflictDecision["action"]): OverwritePolicy {
  return action === "rename" ? "rename" : "overwrite";
}

/** Shared conflict loop. Lists the destination once, then for each item either
 *  starts it (no collision, or user chose overwrite/rename) or skips it. A
 *  remembered "apply to all" decision short-circuits later prompts; "cancel"
 *  stops the batch. */
async function runBatch(
  destSessionId: SessionId,
  destDir: string,
  side: "local" | "remote",
  items: TransferItem[],
  start: (item: TransferItem, policy: OverwritePolicy) => Promise<unknown>
): Promise<void> {
  const settings = useSettings.getState();
  const defaultPolicy = settings.overwritePolicy;

  // "Don't ask" mode (the user ticked "remember" before, or turned prompts off
  // in Settings): apply the saved default silently — the original behaviour.
  if (!settings.promptOnOverwrite) {
    for (const item of items) await start(item, defaultPolicy).catch(() => {});
    return;
  }

  // Snapshot the destination's names so we can spot collisions. If it can't be
  // listed (e.g. doesn't exist yet), assume no conflicts.
  const existing = new Map<string, { size: number; modified?: number }>();
  try {
    const list = await ipc.listDirectory(destSessionId, destDir);
    for (const e of list) {
      existing.set(e.name, { size: e.size, modified: e.modified ?? undefined });
    }
  } catch {
    // destination unreadable / missing → treat as no conflicts
  }

  let remembered: ConflictDecision | null = null;

  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    const name = baseName(item.path);
    const hit = existing.get(name);
    let policy = defaultPolicy;

    if (hit) {
      const decision: ConflictDecision =
        remembered ??
        (await useConflicts.getState().ask({
          name,
          destDir,
          side,
          kind: item.kind,
          remaining: items.length - i - 1,
          existingSize: item.kind === "file" ? hit.size : undefined,
          existingModified: hit.modified ?? undefined,
          sourceSize: item.kind === "file" ? item.size : undefined,
          sourceModified: item.modified,
        }));

      if (decision.applyToAll) remembered = decision;
      if (decision.remember && decision.action !== "cancel") {
        // Persist as the default AND stop asking from now on.
        settings.setOverwritePolicy(
          decision.action === "skip" ? "skip" : actionToPolicy(decision.action)
        );
        settings.setPromptOnOverwrite(false);
      }
      if (decision.action === "cancel") break;
      if (decision.action === "skip") continue;
      policy = actionToPolicy(decision.action);
    }

    await start(item, policy).catch(() => {});
  }
}

function indexBy(transfers: Transfer[]): Record<string, Transfer> {
  return transfers.reduce<Record<string, Transfer>>((acc, t) => {
    acc[t.id] = t;
    return acc;
  }, {});
}

export const useTransfers = create<TransfersState>((set, get) => ({
  byId: {},
  panelOpen: false,

  togglePanel: () => set((s) => ({ panelOpen: !s.panelOpen })),
  setPanelOpen: (open) => set({ panelOpen: open }),

  initListeners: async () => {
    const unlisten = await onTransferEvent((kind, t) => {
      set((s) => ({ byId: { ...s.byId, [t.id]: t } }));
      if (kind === "added" && !get().panelOpen) {
        if (useSettings.getState().autoOpenTransferPanel) {
          set({ panelOpen: true });
        }
      }
      // Surface terminal outcomes as toasts so background transfers aren't silent.
      if (kind === "done") {
        const verb = t.kind === "upload" ? "Uploaded" : "Downloaded";
        if (t.status === "skipped") {
          toast.info("Transfer skipped", baseName(t.source));
        } else {
          toast.success("Transfer complete", `${verb} ${baseName(t.source)}`);
        }
      } else if (kind === "error") {
        toast.error("Transfer failed", t.error || baseName(t.source));
      }
    });
    return unlisten;
  },

  loadInitial: async () => {
    const list = await ipc.listTransfers();
    set({ byId: indexBy(list) });
  },

  download: async (sessionId, remotePath, localDir, policy) => {
    const p = policy ?? useSettings.getState().overwritePolicy;
    return ipc.startDownload(sessionId, remotePath, localDir, p);
  },

  upload: async (sessionId, localPath, remoteDir, policy) => {
    const p = policy ?? useSettings.getState().overwritePolicy;
    return ipc.startUpload(sessionId, localPath, remoteDir, p);
  },

  downloadDir: async (sessionId, remoteDir, localDir, policy) => {
    const p = policy ?? useSettings.getState().overwritePolicy;
    return ipc.startDirectoryDownload(sessionId, remoteDir, localDir, p);
  },

  uploadDir: async (sessionId, localDir, remoteDir, policy) => {
    const p = policy ?? useSettings.getState().overwritePolicy;
    return ipc.startDirectoryUpload(sessionId, localDir, remoteDir, p);
  },

  enqueueDownloads: async (sessionId, items, localDir) => {
    await runBatch(LOCAL_SESSION, localDir, "local", items, (item, policy) =>
      item.kind === "directory"
        ? get().downloadDir(sessionId, item.path, localDir, policy)
        : get().download(sessionId, item.path, localDir, policy)
    );
  },

  enqueueUploads: async (sessionId, items, remoteDir) => {
    await runBatch(sessionId, remoteDir, "remote", items, (item, policy) =>
      item.kind === "directory"
        ? get().uploadDir(sessionId, item.path, remoteDir, policy)
        : get().upload(sessionId, item.path, remoteDir, policy)
    );
  },

  cancel: async (id) => {
    await ipc.cancelTransfer(id);
  },

  clearFinished: () =>
    set((s) => {
      const next: Record<string, Transfer> = {};
      for (const t of Object.values(s.byId)) {
        if (t.status === "transferring" || t.status === "queued") {
          next[t.id] = t;
        }
      }
      return { byId: next };
    }),
}));
