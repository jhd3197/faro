import { useEffect, useState } from "react";
import { FilePane } from "./FilePane";
import { useConnections } from "@/stores/connectionsStore";
import { useTransfers } from "@/stores/transfersStore";
import { LOCAL_SESSION } from "@/lib/types";
import type { DirEntry } from "@/lib/types";

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
    <div className="flex h-full flex-1">
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
      <div className="w-px bg-border" />
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
    </div>
  );
}
