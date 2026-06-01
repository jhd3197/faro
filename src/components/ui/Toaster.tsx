import { CheckCircle2, AlertCircle, Info, AlertTriangle, X } from "lucide-react";
import {
  useToasts,
  type Toast as ToastT,
  type ToastVariant,
} from "@/stores/toastStore";
import { cn } from "@/lib/cn";

const ICON = {
  info: Info,
  success: CheckCircle2,
  error: AlertCircle,
  warning: AlertTriangle,
} as const;

const ACCENT: Record<ToastVariant, string> = {
  info: "text-accent",
  success: "text-emerald-400",
  error: "text-danger",
  warning: "text-amber-400",
};

// Stacked transient toasts, bottom-right, clear of the 28px status bar.
export function Toaster() {
  const toasts = useToasts((s) => s.toasts);
  const dismiss = useToasts((s) => s.dismiss);
  if (toasts.length === 0) return null;
  return (
    <div className="pointer-events-none fixed bottom-10 right-3 z-[60] flex w-80 flex-col gap-2">
      {toasts.map((t) => (
        <ToastCard key={t.id} toast={t} onDismiss={() => dismiss(t.id)} />
      ))}
    </div>
  );
}

function ToastCard({
  toast,
  onDismiss,
}: {
  toast: ToastT;
  onDismiss: () => void;
}) {
  const Icon = ICON[toast.variant];
  return (
    <div className="anim-slide-up pointer-events-auto flex items-start gap-2.5 rounded-lg border border-border bg-bg-panel px-3 py-2.5 shadow-elev-3">
      <Icon size={15} className={cn("mt-0.5 shrink-0", ACCENT[toast.variant])} />
      <div className="min-w-0 flex-1">
        <div className="text-xs font-medium">{toast.title}</div>
        {toast.message && (
          <div className="mt-0.5 break-words text-[11px] text-text-muted">
            {toast.message}
          </div>
        )}
      </div>
      <button
        onClick={onDismiss}
        className="rounded p-0.5 text-text-dim hover:bg-bg-hover hover:text-text"
        title="Dismiss"
      >
        <X size={12} />
      </button>
    </div>
  );
}
