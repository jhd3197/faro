import { createContext, useContext, type ReactNode } from "react";
import type {
  ArchiveFormat,
  Capabilities,
  DirEntry,
  FileKind,
  PaneDensity,
  PaneViewMode,
  SessionId,
  SortDirection,
  SortField,
} from "./types";
import { fileIcon, type FileIconSpec } from "./lib/fileIcons";

// ---------------------------------------------------------------------------
// The transport contract. This is the whole point of the package: the UI never
// imports a Tauri command, an HTTP client, or anything else — it talks to this
// interface, and the host supplies a concrete implementation. Swap the adapter
// and the same components drive SFTP, S3, an in-memory mock, or a REST API.
// ---------------------------------------------------------------------------
export interface FileSystemAdapter {
  /** List a directory. `sessionId` is opaque; LOCAL_SESSION means the host FS. */
  listDirectory(sessionId: SessionId, path: string): Promise<DirEntry[]>;
  /** Which operations this backend supports (hides chmod/mkdir for object stores). */
  capabilities(sessionId: SessionId): Promise<Capabilities>;
  rename(sessionId: SessionId, from: string, to: string): Promise<void>;
  remove(sessionId: SessionId, path: string, recursive: boolean): Promise<void>;
  mkdir(sessionId: SessionId, path: string): Promise<void>;
  chmod(sessionId: SessionId, path: string, mode: number): Promise<void>;
  /**
   * Optional: open a file in an external editor / edit-in-place flow. Omit it
   * and the pane simply doesn't offer the "Edit…" action.
   */
  editFile?(sessionId: SessionId, path: string): Promise<void>;
  /**
   * Optional: fetch raw bytes for an image so the grid can show a real preview
   * instead of a generic icon. Return the file as a Blob, or null if a preview
   * isn't available (too large, remote backend without read, not an image…).
   * The package handles laziness, downscaling, and object-URL cleanup — the
   * adapter only has to hand back bytes. Omit it entirely to keep icons only.
   */
  thumbnail?(sessionId: SessionId, entry: DirEntry): Promise<Blob | null>;
  /**
   * Optional: make a copy of a file/folder alongside the original (the host
   * picks a non-colliding name like "foo copy"). Omit it and the pane doesn't
   * offer "Duplicate". May reject for backends that can't copy server-side.
   */
  duplicate?(sessionId: SessionId, path: string, kind: FileKind): Promise<void>;
  /**
   * Optional: archive a remote directory on the server (one `tar`/`zip`
   * command) and download the single resulting file — far cheaper than walking
   * the tree. Omit it and the pane doesn't offer "Download as …". Remote
   * (shell-capable) backends only.
   */
  archive?(
    sessionId: SessionId,
    path: string,
    format: ArchiveFormat
  ): Promise<void>;
  /**
   * Optional: open an interactive terminal already changed into `path`. Omit it
   * and the pane doesn't offer "Open terminal here". Remote sessions only.
   */
  openTerminal?(sessionId: SessionId, path: string): Promise<void>;
}

// ---------------------------------------------------------------------------
// View state. The pane is fully controlled for sort / view-mode / density so a
// host can persist these however it likes (Faro wires them to its zustand
// settings store). Pass plain values + setters.
// ---------------------------------------------------------------------------
export interface PaneSettings {
  showHiddenFiles: boolean;
  sortField: SortField;
  sortDirection: SortDirection;
  paneViewMode: PaneViewMode;
  paneDensity: PaneDensity;
  /** Display name for the external-editor action, e.g. "VS Code". */
  editorLabel?: string;
  setSortField(field: SortField): void;
  setSortDirection(dir: SortDirection): void;
  setPaneViewMode(mode: PaneViewMode): void;
  setPaneDensity(density: PaneDensity): void;
}

export interface FileUiValue {
  fs: FileSystemAdapter;
  settings: PaneSettings;
  /** Resolve the icon + colour for an entry. Defaults to the built-in map. */
  iconFor: (entry: DirEntry) => FileIconSpec;
}

const FileUiContext = createContext<FileUiValue | null>(null);

export function FileUiProvider({
  fs,
  settings,
  iconFor = fileIcon,
  children,
}: {
  fs: FileSystemAdapter;
  settings: PaneSettings;
  /**
   * Optional: override icon selection (e.g. a host's own brand icon set). Falls
   * back to the package's extension-based map.
   */
  iconFor?: (entry: DirEntry) => FileIconSpec;
  children: ReactNode;
}) {
  // A new object identity each render is fine — settings are small and the only
  // consumers are file panes, which re-render on settings changes anyway.
  return (
    <FileUiContext.Provider value={{ fs, settings, iconFor }}>
      {children}
    </FileUiContext.Provider>
  );
}

export function useFileUi(): FileUiValue {
  const ctx = useContext(FileUiContext);
  if (!ctx) {
    throw new Error(
      "useFileUi must be used within a <FileUiProvider>. Wrap your file browser " +
        "with it and pass a FileSystemAdapter + PaneSettings."
    );
  }
  return ctx;
}
