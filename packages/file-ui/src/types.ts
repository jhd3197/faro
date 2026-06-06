// Canonical file-system types for the file browser. The host app re-exports
// these (Faro does so from src/lib/types.ts) so there is a single source of
// truth shared between the package and whatever transport backs it.

export type FileKind = "file" | "directory" | "symlink" | "other";

export interface DirEntry {
  name: string;
  path: string;
  kind: FileKind;
  size: number;
  modified?: number; // unix seconds
  mode?: number; // posix mode bits when applicable
}

export interface Capabilities {
  canChmod: boolean;
  canSymlink: boolean;
  canRename: boolean;
  hasDirectories: boolean; // false for object stores (S3/Azure)
}

// A session handle is opaque to the UI — it's whatever your adapter understands
// (an SSH session UUID, an HTTP base, a mount id…). The sentinel below marks the
// host's own local filesystem, which the pane treats with native path rules
// (Windows drive letters + backslashes).
export type SessionId = string;
export const LOCAL_SESSION: SessionId = "local";

// ---- View-state types (mirrored by the host's settings store) ----

export type SortField = "name" | "size" | "modified";
export type SortDirection = "asc" | "desc";
export type PaneViewMode = "list" | "details" | "grid";
export type PaneDensity = "comfortable" | "compact";
