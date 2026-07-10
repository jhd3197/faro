import { useEffect, useId, useRef, useState } from "react";
import { X, Bot, Send, Trash2, CircleAlert, Loader2 } from "lucide-react";
import { useAgentChat } from "@/stores/agentChatStore";
import { useConnections } from "@/stores/connectionsStore";
import { useLayout } from "@/stores/layoutStore";
import { useDialog } from "@/hooks/useDialog";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/format";

export function AgentChatDock() {
  const messages = useAgentChat((s) => s.messages);
  const loading = useAgentChat((s) => s.loading);
  const error = useAgentChat((s) => s.error);
  const send = useAgentChat((s) => s.send);
  const clear = useAgentChat((s) => s.clear);
  const setSessionId = useAgentChat((s) => s.setSessionId);
  const setChatOpen = useLayout((s) => s.setChatOpen);

  const sessions = useConnections((s) => s.sessions);
  const profiles = useConnections((s) => s.profiles);
  const activeSessionId = useConnections((s) => s.activeSessionId);

  const [input, setInput] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  // Default the agent context to the currently focused session.
  useEffect(() => {
    setSessionId(activeSessionId);
  }, [activeSessionId, setSessionId]);

  // Keep the view pinned to the latest message.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, loading]);

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const text = input.trim();
    if (!text || loading) return;
    setInput("");
    void send(text);
  };

  return (
    <div className="flex h-full flex-col bg-bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-bg-subtle px-2.5">
        <Bot size={13} className="text-accent" />
        <span id={titleId} className="text-[12px] font-semibold tracking-tight">
          Chat
        </span>
        <div className="flex-1" />
        <button
          onClick={clear}
          disabled={messages.length === 0}
          title="Clear chat"
          className="flex h-6 w-6 items-center justify-center rounded-md text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-40"
        >
          <Trash2 size={12} />
        </button>
        <button
          onClick={() => setChatOpen(false)}
          title="Close chat"
          className="flex h-6 w-6 items-center justify-center rounded-md text-text-muted hover:bg-bg-hover hover:text-text"
        >
          <X size={14} />
        </button>
      </div>

      <div className="flex items-center gap-2 border-b border-border px-3 py-1.5">
        <span className="text-[11px] text-text-dim">Context:</span>
        <select
          value={useAgentChat.getState().sessionId ?? ""}
          onChange={(e) => setSessionId(e.target.value || null)}
          className="min-w-0 flex-1 rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-text outline-none focus:border-accent"
        >
          <option value="">Whole bridge</option>
          {sessions.map((s) => {
            const p = profiles.find((x) => x.id === s.profileId);
            return (
              <option key={s.sessionId} value={s.sessionId}>
                {p?.name ?? s.sessionId}
              </option>
            );
          })}
        </select>
      </div>

      <div
        ref={scrollRef}
        className="min-w-0 flex-1 space-y-3 overflow-y-auto px-3 py-3"
      >
        {messages.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-xs text-text-dim">
            <Bot size={24} className="opacity-50" />
            <div className="max-w-xs">
              Ask the AI to investigate or fix something on your connected
              servers. It can use the same tools as Claude Code over the Agent
              Bridge.
            </div>
          </div>
        )}
        {messages.map((m) => (
          <MessageBubble key={m.id} message={m} />
        ))}
        {loading && (
          <div className="flex items-center gap-2 text-[11px] text-text-dim">
            <Loader2 size={12} className="animate-spin" /> thinking…
          </div>
        )}
        {error && (
          <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-2.5 py-1.5 text-[11px] text-danger">
            <CircleAlert size={12} className="mt-0.5 shrink-0" />
            {error}
          </div>
        )}
      </div>

      <form
        onSubmit={onSubmit}
        className="flex items-center gap-2 border-t border-border px-3 py-2"
      >
        <input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="Ask the AI…"
          className="min-w-0 flex-1 rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent"
        />
        <button
          type="submit"
          disabled={!input.trim() || loading}
          className="btn-accent flex h-8 w-8 items-center justify-center rounded-md text-white disabled:opacity-40"
        >
          <Send size={14} />
        </button>
      </form>
    </div>
  );
}

function MessageBubble({ message }: { message: import("@/stores/agentChatStore").ChatMessage }) {
  const isUser = message.role === "user";
  return (
    <div className={cn("flex flex-col gap-0.5", isUser ? "items-end" : "items-start")}>
      <div
        className={cn(
          "max-w-[90%] rounded-lg px-2.5 py-1.5 text-[12px] leading-relaxed",
          isUser
            ? "bg-accent text-white"
            : message.error
              ? "border border-danger/30 bg-danger/10 text-danger"
              : "border border-border bg-bg-subtle text-text"
        )}
      >
        {message.content}
        {message.toolName && (
          <div className="mt-1 text-[10px] opacity-80">
            tool: <span className="font-mono">{message.toolName}</span>
          </div>
        )}
      </div>
      <span className="text-[9px] text-text-dim">{relTime(message.at)}</span>
    </div>
  );
}

export function AgentChatModal({ onClose }: { onClose: () => void }) {
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });
  return (
    <div
      className="fixed inset-0 z-modal flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
        className="anim-modal flex h-[80vh] w-[32rem] max-w-[94vw] flex-col rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <AgentChatDock />
      </div>
    </div>
  );
}
