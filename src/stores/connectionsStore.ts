import { create } from "zustand";
import { ipc } from "@/lib/ipc";
import { toast } from "./toastStore";
import type { ConnectionProfile, SessionId } from "@/lib/types";
import { useTerminals } from "./terminalsStore";

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
    const profile = get().profiles.find((p) => p.id === profileId);
    try {
      const sessionId = await ipc.connect(profileId);
      set({
        activeSessionId: sessionId,
        activeProfileId: profileId,
        connecting: false,
      });
      toast.success(
        "Connected",
        profile
          ? `${profile.name} — ${profile.username}@${profile.host}`
          : undefined
      );
    } catch (e) {
      set({ connecting: false, error: String(e) });
      toast.error("Connection failed", profile ? `${profile.name}: ${e}` : String(e));
      throw e;
    }
  },

  disconnect: async () => {
    const sid = get().activeSessionId;
    if (!sid) return;
    const profile = get().profiles.find((p) => p.id === get().activeProfileId);
    await ipc.disconnect(sid);
    useTerminals.getState().dropSessionTabs(sid);
    set({ activeSessionId: null, activeProfileId: null });
    toast.info("Disconnected", profile?.name);
  },
}));
