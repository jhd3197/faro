import { useEffect, useState } from "react";
import { FilePane } from "./FilePane";
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
  const [remotePath, setRemotePath] = useState(
    profile?.defaultRemotePath || "."
  );
  const [syncOpen, setSyncOpen] = useState(false);

  useEffect(() => {
    setRemotePath(profile?.defaultRemotePath || ".");
  }, [profile?.id]);

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
          <button
            onClick={() => setSyncOpen(true)}
            title="Sync this folder with the remote pane"
            className="btn-accent absolute left-1/2 top-3 z-20 flex h-6 -translate-x-1/2 items-center gap-1 rounded-full px-2 text-[10px] font-medium uppercase tracking-wider text-white shadow-elev-2"
          >
            <ArrowRightLeft size={10} />
            Sync
          </button>
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
