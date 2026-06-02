import { useEffect, useId, useRef } from "react";
import {
  X,
  Trash2,
  TerminalSquare,
  FileText,
  Download,
  Upload,
  Search,
  Ban,
  CircleAlert,
  CheckCircle2,
  Loader2,
  Radio,
} from "lucide-react";
import { useBridge } from "@/stores/bridgeStore";
import { useDialog } from "@/hooks/useDialog";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/format";
import type { AgentConsoleEntry } from "@/lib/types";

// A live, read-only view of everything the agent does through the bridge —
// commands stream their output in real time; file ops show as a feed. This is
// the human's window into the agent; it doesn't accept input.
export function AgentConsole({ onClose }: { onClose: () => void }) {
  const entries = useBridge((s) => s.console);
  const clear = useBridge((s) => s.clearConsole);
  const running = entries.filter((e) => e.status === "running").length;

  const scrollRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries]);

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
        className="anim-modal flex max-h-[85vh] w-[44rem] max-w-[95vw] flex-col rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <div className="flex items-center gap-2 border-b border-border px-5 py-3.5">
          <Radio size={15} className="text-accent" />
          <span id={titleId} className="text-[15px] font-semibold tracking-tight">
            Agent console
          </span>
          {running > 0 && (
            <span className="flex items-center gap-1 rounded-full bg-accent/15 px-1.5 py-0.5 text-[10px] font-medium text-accent">
              <Loader2 size={10} className="animate-spin" /> {running} running
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={clear}
            disabled={entries.length === 0}
            className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-40"
          >
            <Trash2 size={11} /> Clear
          </button>
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-muted hover:bg-bg-hover hover:text-text"
          >
            <X size={14} />
          </button>
        </div>

        <div ref={scrollRef} className="flex-1 space-y-2 overflow-y-auto px-4 py-3">
          {entries.length === 0 ? (
            <div className="flex h-40 flex-col items-center justify-center gap-2 text-center text-xs text-text-dim">
              <TerminalSquare size={20} className="opacity-50" />
              <div>
                Nothing yet. When the agent runs commands or transfers files
                through the bridge, they stream here live.
              </div>
            </div>
          ) : (
            entries.map((e) => <ConsoleEntry key={e.id} entry={e} />)
          )}
        </div>

        <div className="border-t border-border px-5 py-3 text-[11px] text-text-dim">
          <Radio size={11} className="mr-1 inline" />
          Live, read-only — this is your view of what the agent is doing over the
          Agent Bridge.
        </div>
      </div>
    </div>
  );
}

function KindIcon({ entry }: { entry: AgentConsoleEntry }) {
  if (entry.status === "running")
    return <Loader2 size={12} className="mt-0.5 shrink-0 animate-spin text-accent" />;
  if (entry.kind === "denied")
    return <Ban size={12} className="mt-0.5 shrink-0 text-text-dim" />;
  if (entry.kind === "error" || entry.ok === false)
    return <CircleAlert size={12} className="mt-0.5 shrink-0 text-danger" />;
  switch (entry.kind) {
    case "exec":
      return <TerminalSquare size={12} className="mt-0.5 shrink-0 text-accent" />;
    case "read":
      return <FileText size={12} className="mt-0.5 shrink-0 text-text-muted" />;
    case "search":
      return <Search size={12} className="mt-0.5 shrink-0 text-text-muted" />;
    case "download":
      return <Download size={12} className="mt-0.5 shrink-0 text-text-muted" />;
    case "upload":
      return <Upload size={12} className="mt-0.5 shrink-0 text-text-muted" />;
    default:
      return <CheckCircle2 size={12} className="mt-0.5 shrink-0 text-success" />;
  }
}

function ConsoleEntry({ entry }: { entry: AgentConsoleEntry }) {
  const failed = entry.kind === "error" || entry.ok === false;
  return (
    <div className="rounded-md border border-border-subtle bg-bg-subtle/40 px-3 py-2">
      <div className="flex items-start gap-2">
        <KindIcon entry={entry} />
        <span
          className={cn(
            "min-w-0 flex-1 break-words font-mono text-[12px]",
            failed ? "text-danger" : "text-text"
          )}
        >
          {entry.command}
        </span>
        {entry.sessionName && (
          <span className="shrink-0 truncate text-[10px] text-text-dim" title={entry.sessionName}>
            {entry.sessionName}
          </span>
        )}
        <span className="shrink-0 text-[10px] text-text-dim">{relTime(entry.at)}</span>
      </div>
      {entry.output && (
        <pre className="selectable mt-1.5 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded bg-bg-panel px-2.5 py-1.5 font-mono text-[11px] leading-relaxed text-text-muted">
          {entry.output}
        </pre>
      )}
    </div>
  );
}
