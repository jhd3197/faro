import { useEffect } from "react";
import { ArrowDownToLine, ArrowUpFromLine, X, ChevronDown, Trash2, ArrowDownUp } from "lucide-react";
import { useTransfers } from "@/stores/transfersStore";
import type { Transfer } from "@/lib/types";
import { cn } from "@/lib/cn";

export function TransferQueue() {
  const { byId, panelOpen, setPanelOpen, cancel, clearFinished, initListeners, loadInitial } =
    useTransfers();

  useEffect(() => {
    let unsub: (() => void) | undefined;
    (async () => {
      await loadInitial();
      unsub = await initListeners();
    })();
    return () => {
      unsub?.();
    };
  }, [initListeners, loadInitial]);

  const transfers = Object.values(byId).sort((a, b) => b.startedAt - a.startedAt);
  if (!panelOpen) return null;

  const active = transfers.filter(
    (t) => t.status === "transferring" || t.status === "queued"
  ).length;

  return (
    <div className="anim-slide-up flex max-h-64 flex-col border-t border-border bg-bg-panel">
      <div className="flex items-center gap-2 border-b border-border bg-bg-subtle px-3 py-1.5">
        <span className="text-xs font-semibold uppercase tracking-wider text-text-muted">
          Transfers
        </span>
        {active > 0 && (
          <span className="rounded bg-accent/20 px-1.5 py-0.5 text-[10px] font-medium text-accent">
            {active} active
          </span>
        )}
        <div className="flex-1" />
        <button
          onClick={clearFinished}
          className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          title="Clear finished"
        >
          <Trash2 size={12} />
        </button>
        <button
          onClick={() => setPanelOpen(false)}
          className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          title="Hide"
        >
          <ChevronDown size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto">
        {transfers.length === 0 && (
          <div className="flex flex-col items-center justify-center px-3 py-8 text-center anim-fade">
            <div className="mb-2 flex h-10 w-10 items-center justify-center rounded-xl bg-bg-subtle text-text-dim">
              <ArrowDownUp size={16} />
            </div>
            <div className="text-xs text-text-dim">
              No transfers yet. Drag files between panes to begin.
            </div>
          </div>
        )}
        {transfers.map((t) => (
          <Row key={t.id} t={t} onCancel={() => cancel(t.id)} />
        ))}
      </div>
    </div>
  );
}

function Row({ t, onCancel }: { t: Transfer; onCancel: () => void }) {
  const Icon = t.kind === "download" ? ArrowDownToLine : ArrowUpFromLine;
  const pct = t.size > 0 ? Math.min(100, (t.transferred / t.size) * 100) : 0;

  return (
    <div className="flex items-center gap-3 border-b border-border-subtle px-3 py-2 text-sm">
      <Icon
        size={14}
        className={cn(
          t.kind === "download" ? "text-blue-400" : "text-emerald-400"
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="truncate font-mono text-xs">{t.source}</div>
        <div className="truncate text-[11px] text-text-dim">
          → {t.destination}
        </div>
        {t.status === "transferring" && (
          <div className="mt-1 h-1 w-full overflow-hidden rounded bg-bg-subtle">
            <div
              className="h-full bg-accent transition-[width] duration-150"
              style={{ width: `${pct}%` }}
            />
          </div>
        )}
        {t.error && (
          <div className="mt-1 text-[11px] text-danger">{t.error}</div>
        )}
      </div>
      <div className="w-20 shrink-0 text-right text-[11px] text-text-muted">
        <div>{fmtSize(t.transferred)}</div>
        <div className="text-text-dim">
          {t.status === "transferring"
            ? `${pct.toFixed(0)}%`
            : t.status === "done"
              ? "done"
              : t.status === "error"
                ? "error"
                : t.status === "canceled"
                  ? "canceled"
                  : t.status}
        </div>
      </div>
      {(t.status === "transferring" || t.status === "queued") && (
        <button
          onClick={onCancel}
          className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          title="Cancel"
        >
          <X size={12} />
        </button>
      )}
    </div>
  );
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
