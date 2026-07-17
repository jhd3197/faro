import {
  LOCAL_SESSION,
  isImage,
  imageMime,
  type FileSystemAdapter,
} from "@faro/file-ui";
import { ipc } from "./ipc";
import { useEditor } from "@/stores/editorStore";
import { useTerminals } from "@/stores/terminalsStore";
import { useLayout } from "@/stores/layoutStore";
import { useDiskScan } from "@/stores/diskScanStore";
import { useDiff } from "@/stores/diffStore";
import { useSearch } from "@/stores/searchStore";

// POSIX single-quote a path so a `cd` survives spaces / shell metacharacters.
function shQuote(p: string): string {
  return `'${p.replace(/'/g, `'\\''`)}'`;
}

// The concrete @faro/file-ui adapter for Faro: it maps the package's
// transport-agnostic operations onto Faro's Tauri command surface. This is the
// ONE place the open-source file UI meets Faro's Rust backend — swap this object
// and the same components would drive any other backend (HTTP, mock, …).
export const tauriFileSystem: FileSystemAdapter = {
  listDirectory: (sessionId, path) => ipc.listDirectory(sessionId, path),
  capabilities: (sessionId) => ipc.capabilities(sessionId),
  rename: (sessionId, from, to) => ipc.renamePath(sessionId, from, to),
  remove: (sessionId, path, recursive) =>
    ipc.deletePath(sessionId, path, recursive),
  mkdir: (sessionId, path) => ipc.createDirectory(sessionId, path),
  chmod: (sessionId, path, mode) => ipc.chmodPath(sessionId, path, mode),
  // Edit-in-place runs through the editor store, which installs the
  // save/error listeners and tracks the live-edit pill in the status bar.
  editFile: (sessionId, path) =>
    useEditor.getState().startEditing(sessionId, path),

  // Copy a file/folder next to the original. The Rust side resolves a free
  // name and does the copy server-side (cp over SSH) or on the local disk.
  duplicate: (sessionId, path) => ipc.duplicatePath(sessionId, path),

  // Server-side archive then download. Returns once the transfer is queued; the
  // transfers panel tracks the actual byte download, and the backend removes the
  // temp archive on the server when it finishes.
  archive: async (sessionId, path, format) => {
    await ipc.startArchiveDownload(sessionId, path, format);
  },

  // Reveal the terminal dock and open a fresh shell already cd'd into `path`.
  openTerminal: async (sessionId, path) => {
    useLayout.getState().setTerminalOpen(true);
    useTerminals.getState().openTab(sessionId, `cd ${shQuote(path)}\n`);
  },

  // Open the Disk Usage explorer and kick off a scan of `path`. The store owns
  // the scan lifecycle (progress events, cancel, drill-down); this just triggers it.
  analyzeDiskUsage: async (sessionId, path) => {
    await useDiskScan.getState().openFor(sessionId, path);
  },

  // Open the Directory Diff view with this folder as side A. The user picks side
  // B (a connection + path) in the overlay, then runs the comparison; the diff
  // store owns that lifecycle.
  compareDirectory: async (sessionId, path) => {
    useDiff.getState().openFor(sessionId, path);
  },

  // Open the Fleet Search panel rooted at this folder. The user types a name or
  // content query and runs it; the search store owns the streaming lifecycle.
  searchDirectory: async (sessionId, path) => {
    useSearch.getState().openFor(sessionId, path);
  },

  // Image previews for the grid view. Local files only for now — the backend
  // `read_file_preview` command reads bytes off the local disk and we wrap them
  // in a Blob for the package to downscale. Remote sessions return null, so the
  // pane falls back to the type icon. Any failure → null → icon.
  thumbnail: async (sessionId, entry) => {
    if (sessionId !== LOCAL_SESSION || !isImage(entry)) return null;
    try {
      const b64 = await ipc.readFilePreview(sessionId, entry.path);
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const type = imageMime(entry.name) ?? "application/octet-stream";
      return new Blob([bytes], { type });
    } catch {
      return null;
    }
  },
};
