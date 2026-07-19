import { useEffect, useState } from "react";
import {
  Info,
  Loader2,
  CheckCircle2,
  TerminalSquare,
  Plus,
  Trash2,
} from "lucide-react";
import { ipc } from "@/lib/ipc";
import { toast } from "@/stores/toastStore";
import type { PathStatus } from "@/lib/types";
import { getVersion } from "@tauri-apps/api/app";

/// Settings → About. The app's identity/version, plus the trust/UX fundamentals
/// from Plan 16: the one-click "Add faro-cli to PATH" row (Phase 4). The updater
/// card (Phase 2) and notification toggles (Phase 3) join this section as they land.
export function AboutSettings() {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const v = await getVersion();
        if (!cancelled) setVersion(v);
      } catch {
        /* not in a Tauri context (mock/browser) — just omit the version */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h3 className="mb-1 flex items-center gap-2 text-sm font-medium text-text">
          <Info size={15} className="text-accent" />
          Faro{version ? ` ${version}` : ""}
        </h3>
        <p className="text-xs text-text-muted">
          A modern client for SFTP · FTP · S3 · Azure — plus the Agent Bridge.
        </p>
      </div>

      <PathRow />
    </div>
  );
}

/// One-click "Add faro-cli to PATH" (Plan 16 Phase 4). Per-user only, so it never
/// needs admin. `faro-cli` ships as a separate binary from the app; once it's on
/// PATH it works in any terminal. On Windows this writes the per-user
/// `HKCU\Environment\Path` (new terminals pick it up immediately); on macOS/Linux
/// it symlinks into `~/.local/bin` and, if needed, adds one guarded line to the
/// shell profile.
function PathRow() {
  const [status, setStatus] = useState<PathStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      setStatus(await ipc.pathStatus());
    } catch (e) {
      toast.error(`Couldn't read PATH status: ${e}`);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const add = async () => {
    setBusy(true);
    try {
      const next = await ipc.pathAdd();
      setStatus(next);
      toast.success(next.detail ?? "Added faro-cli to your PATH.");
    } catch (e) {
      toast.error(`Add to PATH failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      const next = await ipc.pathRemove();
      setStatus(next);
      toast.success(next.detail ?? "Removed faro-cli from your PATH.");
    } catch (e) {
      toast.error(`Remove from PATH failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  // Prefer the managed flag for the primary action: right after an add, our
  // registry/symlink entry is present even though this already-running process's
  // own PATH won't reflect it until a new terminal (or app restart).
  const managed = status?.managed ?? false;
  const onPath = status?.onPath ?? false;
  const canInstallFirst = status ? !status.binHasCli && !onPath && !managed : false;

  return (
    <div>
      <div className="mb-1.5 flex items-center gap-2 text-sm font-medium text-text">
        <TerminalSquare size={15} className="text-accent" />
        faro-cli on PATH
      </div>
      <p className="mb-2 text-xs text-text-muted">
        Put the <code className="text-text">faro-cli</code> command on your PATH so
        it works in any terminal. Per-user only — no admin required.
      </p>

      <div className="rounded-lg border border-border bg-bg-subtle/50 p-3 text-sm">
        {!status ? (
          <div className="flex items-center gap-2 text-text-muted">
            <Loader2 size={15} className="animate-spin" /> Checking…
          </div>
        ) : managed ? (
          <div className="flex items-start gap-2">
            <CheckCircle2 size={15} className="mt-0.5 shrink-0 text-emerald-500" />
            <div className="min-w-0">
              <div className="text-text">faro-cli is on your PATH (managed by Faro).</div>
              <p className="mt-0.5 truncate text-xs text-text-muted">{status.binDir}</p>
            </div>
          </div>
        ) : onPath ? (
          <div className="flex items-start gap-2">
            <CheckCircle2 size={15} className="mt-0.5 shrink-0 text-emerald-500" />
            <div className="min-w-0">
              <div className="text-text">faro-cli is already on your PATH.</div>
              {status.cliLocation && (
                <p className="mt-0.5 truncate text-xs text-text-muted">
                  {status.cliLocation}
                </p>
              )}
            </div>
          </div>
        ) : canInstallFirst ? (
          <div className="min-w-0">
            <div className="text-text">faro-cli isn't installed yet.</div>
            <p className="mt-0.5 text-xs text-text-muted">
              Install it from the <span className="font-medium">faro-cli</span> tab
              first, then add it to your PATH here.
            </p>
          </div>
        ) : (
          <div className="min-w-0">
            <div className="text-text">faro-cli isn't on your PATH.</div>
            <p className="mt-0.5 truncate text-xs text-text-muted">{status.binDir}</p>
          </div>
        )}

        <div className="mt-3 flex gap-2">
          {managed ? (
            <button
              onClick={() => void remove()}
              disabled={busy}
              className="inline-flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-xs text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-60"
            >
              {busy ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <Trash2 size={13} />
              )}
              Remove from PATH
            </button>
          ) : (
            <button
              onClick={() => void add()}
              disabled={busy || onPath || canInstallFirst}
              className="inline-flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:bg-accent/90 disabled:opacity-60"
            >
              {busy ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <Plus size={13} />
              )}
              Add to PATH
            </button>
          )}
          <button
            onClick={() => void load()}
            disabled={busy}
            className="rounded-md border border-border px-3 py-1.5 text-xs text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-60"
          >
            Refresh
          </button>
        </div>
      </div>
    </div>
  );
}
