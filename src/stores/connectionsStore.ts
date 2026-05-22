import { create } from "zustand";
import { ipc } from "@/lib/ipc";
import type { ConnectionProfile, SessionId } from "@/lib/types";

interface ConnectionsState {
  profiles: ConnectionProfile[];
  activeSessionId: SessionId | null;
  activeProfileId: string | null;
  connecting: boolean;
  error: string | null;

  loadProfiles: () => Promise<void>;
  saveProfile: (p: ConnectionProfile) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  connect: (profileId: string) => Promise<void>;
  disconnect: () => Promise<void>;
}

export const useConnections = create<ConnectionsState>((set, get) => ({
  profiles: [],
  activeSessionId: null,
  activeProfileId: null,
  connecting: false,
  error: null,

  loadProfiles: async () => {
    const profiles = await ipc.listProfiles();
    set({ profiles });
  },

  saveProfile: async (p) => {
    await ipc.saveProfile(p);
    await get().loadProfiles();
  },

  deleteProfile: async (id) => {
    await ipc.deleteProfile(id);
    await get().loadProfiles();
  },

  connect: async (profileId) => {
    set({ connecting: true, error: null });
    try {
      const sessionId = await ipc.connect(profileId);
      set({
        activeSessionId: sessionId,
        activeProfileId: profileId,
        connecting: false,
      });
    } catch (e) {
      set({ connecting: false, error: String(e) });
      throw e;
    }
  },

  disconnect: async () => {
    const sid = get().activeSessionId;
    if (!sid) return;
    await ipc.disconnect(sid);
    set({ activeSessionId: null, activeProfileId: null });
  },
}));
