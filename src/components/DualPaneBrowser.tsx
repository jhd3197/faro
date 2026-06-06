import { useState } from "react";
import { FilePane } from "@faro/file-ui";
import { SyncDialog } from "./SyncDialog";
import { useConnections } from "@/stores/connectionsStore";
import { useTransfers } from "@/stores/transfersStore";
import { LOCAL_SESSION } from "@/lib/types";
import type { DirEntry } from "@/lib/types";
import { ArrowRightLeft } from "lucide-react";

export function DualPaneBrowser() {
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const profiles = useConnections((s) => s.profiles);
  const profile = profiles.find((p) => p.id === activeProfileId) || null;
  const startDownload = useTransfers((s) => s.download);
  const startUpload = useTransfers((s) => s.upload);
  const startDirDownload = useTransfers((s) => s.downloadDir);
  const startDirUpload = useTransfers((s) => s.uploadDir);

  const [localPath, setLocalPath] = useState(
    navigator.userAgent.includes("Windows") ? "C:\\" : "/"
  );
  // Each live session keeps its own remote location, so switching connection
  // tabs returns you to where you left off on that server.
  const [remotePaths, setRemotePaths] = useState<Record<string, string>>({});
  const [syncOpen, setSyncOpen] = useState(false);

  const remotePath =
    (activeSessionId && remotePaths[activeSessionId]) ||
    profile?.defaultRemotePath ||
    ".";
  const setRemotePath = (path: string) => {
    if (!activeSessionId) return;
    setRemotePaths((m) => ({ ...m, [activeSessionId]: path }));
  };

  const uploadAll = (entries: DirEntry[]) => {
    if (!activeSessionId) return;
    for (const e of entries) {
      if (e.kind === "directory") {
        startDirUpload(activeSessionId, e.path, remotePath).catch(() => {});
      } else if (e.kind === "file") {
        startUpload(activeSessionId, e.path, remotePath).catch(() => {});
      }
    }
  };

  const downloadAll = (entries: DirEntry[]) => {
    if (!activeSessionId) return;
    for (const e of entries) {
      if (e.kind === "directory") {
        startDirDownload(activeSessionId, e.path, localPath).catch(() => {});
      } else if (e.kind === "file") {
        startDownload(activeSessionId, e.path, localPath).catch(() => {});
      }
    }
  };

  return (
    <div className="relative flex h-full flex-1">
      <FilePane
        paneId="local"
        title="Local"
        sessionId={LOCAL_SESSION}
        path={localPath}
        onPathChange={setLocalPath}
        onTransfer={uploadAll}
        onDrop={downloadAll}
        transferLabel="Upload"
      />
      <div className="relative w-px bg-border">
        {activeSessionId && (
          // Centering lives on the wrapper so the button keeps its own
          // transform for the press-scale (button:active scale would otherwise
          // wipe out a -translate centering and make it jump on click).
          <div className="absolute left-1/2 top-1/2 z-raised -translate-x-1/2 -translate-y-1/2">
            <button
              onClick={() => setSyncOpen(true)}
              title="Sync folders"
              aria-label="Sync folders"
              className="flex h-7 w-7 items-center justify-center rounded-full border border-border bg-bg-subtle text-text-muted shadow-elev-1 hover:border-accent hover:bg-accent-soft hover:text-accent hover:shadow-elev-2"
            >
              <ArrowRightLeft size={13} />
            </button>
          </div>
        )}
      </div>
      <FilePane
        paneId="remote"
        title={profile ? `Remote — ${profile.name}` : "Remote"}
        sessionId={activeSessionId}
        path={remotePath}
        onPathChange={setRemotePath}
        onTransfer={downloadAll}
        onDrop={uploadAll}
        transferLabel="Download"
      />

      {syncOpen && activeSessionId && (
        <SyncDialog
          localPath={localPath}
          remotePath={remotePath}
          onClose={() => setSyncOpen(false)}
        />
      )}
    </div>
  );
}
