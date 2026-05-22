interface Props {
  title: string;
  message: string;
  confirmLabel?: string;
  destructive?: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

export function ConfirmModal({
  title,
  message,
  confirmLabel = "Confirm",
  destructive,
  onClose,
  onConfirm,
}: Props) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="anim-modal w-[26rem] rounded-xl border border-border bg-bg-panel p-5 shadow-elev-3"
      >
        <div className="mb-2 text-sm font-semibold">{title}</div>
        <div className="mb-4 whitespace-pre-line text-sm text-text-muted">
          {message}
        </div>
        <div className="flex justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-md border border-border px-3.5 py-1.5 text-sm hover:bg-bg-hover"
          >
            Cancel
          </button>
          <button
            onClick={() => {
              onConfirm();
              onClose();
            }}
            className={
              destructive
                ? "rounded-md bg-danger px-3.5 py-1.5 text-sm font-medium text-white hover:opacity-90"
                : "btn-accent rounded-md px-3.5 py-1.5 text-sm font-medium text-white"
            }
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
