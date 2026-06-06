import {
  LOCAL_SESSION,
  isImage,
  imageMime,
  type FileSystemAdapter,
} from "@faro/file-ui";
import { ipc } from "./ipc";
import { useEditor } from "@/stores/editorStore";

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
