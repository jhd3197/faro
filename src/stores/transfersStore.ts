import { create } from "zustand";
import { ipc, onTransferEvent } from "@/lib/ipc";
import { useSettings } from "./settingsStore";
import type { SessionId, Transfer } from "@/lib/types";

interface TransfersState {
  byId: Record<string, Transfer>;
  panelOpen: boolean;

  togglePanel: () => void;
  setPanelOpen: (open: boolean) => void;

  initListeners: () => Promise<() => void>;
  loadInitial: () => Promise<void>;

  download: (
    sessionId: SessionId,
    remotePath: string,
    localDir: string
  ) => Promise<string>;
  upload: (
    sessionId: SessionId,
    localPath: string,
    remoteDir: string
  ) => Promise<string>;
  downloadDir: (
    sessionId: SessionId,
    remoteDir: string,
    localDir: string
  ) => Promise<string[]>;
  uploadDir: (
    sessionId: SessionId,
    localDir: string,
    remoteDir: string
  ) => Promise<string[]>;
  cancel: (id: string) => Promise<void>;
  clearFinished: () => void;
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
    });
    return unlisten;
  },

  loadInitial: async () => {
    const list = await ipc.listTransfers();
    set({ byId: indexBy(list) });
  },

  download: async (sessionId, remotePath, localDir) => {
    const policy = useSettings.getState().overwritePolicy;
    return ipc.startDownload(sessionId, remotePath, localDir, policy);
  },

  upload: async (sessionId, localPath, remoteDir) => {
    const policy = useSettings.getState().overwritePolicy;
    return ipc.startUpload(sessionId, localPath, remoteDir, policy);
  },

  downloadDir: async (sessionId, remoteDir, localDir) => {
    const policy = useSettings.getState().overwritePolicy;
    return ipc.startDirectoryDownload(sessionId, remoteDir, localDir, policy);
  },

  uploadDir: async (sessionId, localDir, remoteDir) => {
    const policy = useSettings.getState().overwritePolicy;
    return ipc.startDirectoryUpload(sessionId, localDir, remoteDir, policy);
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
