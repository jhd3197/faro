import { useId, useRef } from "react";
import type { DirEntry } from "../types";
import { fmtSize, fmtMtime, formatMode } from "../lib/format";
import { useDialog } from "../hooks/useDialog";

interface Props {
  entry: DirEntry;
  onClose: () => void;
}

const KIND_LABEL: Record<DirEntry["kind"], string> = {
  file: "File",
  directory: "Folder",
  symlink: "Symlink",
  other: "Special",
};

// Read-only "Properties" sheet. Everything shown is already on the DirEntry the
// pane listed, so no backend round-trip is needed — it's a nicer presentation of
// what the row's columns hint at.
export function PropertiesModal({ entry, onClose }: Props) {
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });

  const rows: Array<{ label: string; value: React.ReactNode }> = [
    { label: "Type", value: KIND_LABEL[entry.kind] },
  ];
  if (entry.kind === "file") {
    rows.push({
      label: "Size",
      value: (
        <span>
          {fmtSize(entry.size)}
          <span className="ml-1.5 text-text-dim">
            ({entry.size.toLocaleString()} bytes)
          </span>
        </span>
      ),
    });
  }
  rows.push({ label: "Modified", value: fmtMtime(entry.modified) || "—" });
  if (entry.mode != null) {
    rows.push({
      label: "Permissions",
      value: (
        <span className="font-mono">
          {formatMode(entry.mode)}
          <span className="ml-1.5 text-text-dim">
            {(entry.mode & 0o777).toString(8)}
          </span>
        </span>
      ),
    });
  }

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
        className="anim-modal w-[28rem] max-w-[92vw] rounded-xl border border-border bg-bg-panel p-5 shadow-elev-3"
      >
        <div
          id={titleId}
          className="mb-3 truncate text-sm font-semibold"
          title={entry.name}
        >
          {entry.name}
        </div>
        <dl className="space-y-2 text-sm">
          {rows.map((r) => (
            <div key={r.label} className="flex gap-3">
              <dt className="w-28 shrink-0 text-text-muted">{r.label}</dt>
              <dd className="min-w-0 flex-1 break-words">{r.value}</dd>
            </div>
          ))}
          <div className="flex gap-3">
            <dt className="w-28 shrink-0 text-text-muted">Path</dt>
            <dd className="min-w-0 flex-1 break-all font-mono text-xs text-text-muted">
              {entry.path}
            </dd>
          </div>
        </dl>
        <div className="mt-5 flex justify-end">
          <button
            onClick={onClose}
            className="rounded-md border border-border px-3.5 py-1.5 text-sm hover:bg-bg-hover"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
