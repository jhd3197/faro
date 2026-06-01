import { create } from "zustand";
import { ipc, onBridgeApproval, onBridgeActivity } from "@/lib/ipc";
import { toast } from "./toastStore";
import type {
  BridgeStatus,
  BridgeActivity,
  BridgeApproval,
  ApprovalDecision,
} from "@/lib/types";

const EMPTY: BridgeStatus = {
  running: false,
  url: null,
  port: null,
  token: null,
  enabledSessions: [],
};

interface BridgeStoreState {
  status: BridgeStatus;
  activity: BridgeActivity[];
  approvals: BridgeApproval[];
  loaded: boolean;

  init: () => Promise<() => void>;
  refresh: () => Promise<void>;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  setSessionAccess: (sessionId: string, enabled: boolean) => Promise<void>;
  respond: (requestId: string, decision: ApprovalDecision) => Promise<void>;
}

export const useBridge = create<BridgeStoreState>((set, get) => ({
  status: EMPTY,
  activity: [],
  approvals: [],
  loaded: false,

  init: async () => {
    if (!get().loaded) {
      set({ loaded: true });
      try {
        const [status, activity] = await Promise.all([
          ipc.bridgeStatus(),
          ipc.bridgeActivity(),
        ]);
        set({ status, activity: activity.slice().reverse() });
      } catch {
        // backend not ready yet — listeners below still attach
      }
    }
    const unApproval = await onBridgeApproval((a) => {
      set((s) => ({ approvals: [...s.approvals, a] }));
    });
    const unActivity = await onBridgeActivity((e) => {
      set((s) => ({ activity: [e, ...s.activity].slice(0, 200) }));
    });
    return () => {
      unApproval();
      unActivity();
    };
  },

  refresh: async () => {
    try {
      set({ status: await ipc.bridgeStatus() });
    } catch {
      // ignore
    }
  },

  start: async () => {
    try {
      const status = await ipc.bridgeStart();
      set({ status });
      toast.success("Agent Bridge started", status.url ?? undefined);
    } catch (e) {
      toast.error("Couldn't start Agent Bridge", String(e));
    }
  },

  stop: async () => {
    try {
      const status = await ipc.bridgeStop();
      set({ status });
      toast.info("Agent Bridge stopped");
    } catch (e) {
      toast.error("Couldn't stop Agent Bridge", String(e));
    }
  },

  setSessionAccess: async (sessionId, enabled) => {
    try {
      const status = await ipc.bridgeSetSessionAccess(sessionId, enabled);
      set({ status });
    } catch (e) {
      toast.error("Couldn't update agent access", String(e));
    }
  },

  respond: async (requestId, decision) => {
    // Drop it from the local queue immediately for snappy UI.
    set((s) => ({
      approvals: s.approvals.filter((a) => a.requestId !== requestId),
    }));
    try {
      await ipc.respondToBridgeApproval(requestId, decision);
    } catch {
      // the request may have already timed out on the backend
    }
  },
}));
