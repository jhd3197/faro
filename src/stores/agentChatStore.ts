import { create } from "zustand";
import { ipc } from "@/lib/ipc";

export type ChatRole = "user" | "assistant" | "tool";

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  toolName?: string;
  toolInput?: string;
  toolOutput?: string;
  error?: boolean;
  at: number;
}

interface AgentChatState {
  messages: ChatMessage[];
  loading: boolean;
  error: string | null;
  sessionId: string | null;

  setSessionId: (id: string | null) => void;
  send: (prompt: string) => Promise<void>;
  clear: () => void;
}

function uid() {
  return Math.random().toString(36).slice(2, 10);
}

export const useAgentChat = create<AgentChatState>((set, get) => ({
  messages: [],
  loading: false,
  error: null,
  sessionId: null,

  setSessionId: (id) => set({ sessionId: id }),

  send: async (prompt) => {
    // The key lives in the OS keychain now (Plan 12 Phase 1) — the frontend
    // only asks whether one is set; the backend reads the value at call time.
    const hasKey = await ipc.apiKeyStatus("anthropic-api-key").catch(() => false);
    if (!hasKey) {
      set({ error: "Add an Anthropic API key in Settings → Agent." });
      return;
    }

    const userMsg: ChatMessage = {
      id: uid(),
      role: "user",
      content: prompt,
      at: Date.now(),
    };
    set((s) => ({
      messages: [...s.messages, userMsg],
      loading: true,
      error: null,
    }));

    try {
      const response = await ipc.agentChat({
        sessionId: get().sessionId,
        prompt,
        history: get()
          .messages.filter(
            (m): m is ChatMessage & { role: "user" | "assistant" } =>
              m.role === "user" || m.role === "assistant"
          )
          .map((m) => ({ role: m.role, content: m.content })),
      });

      const assistantMsg: ChatMessage = {
        id: uid(),
        role: "assistant",
        content: response.content,
        at: Date.now(),
      };
      set((s) => ({
        messages: [...s.messages, assistantMsg],
        loading: false,
      }));
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  clear: () => set({ messages: [], error: null }),
}));
