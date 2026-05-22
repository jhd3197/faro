// Shared types mirroring the Rust serde structs in src-tauri/src/.
// Keep these in sync with src-tauri/src/remotefs/mod.rs and profiles/mod.rs.

export type AuthMethod =
  | { kind: "password"; password: string }
  | { kind: "key"; path: string; passphrase?: string }
  | { kind: "agent" };

export interface ConnectionProfile {
  id: string;
  name: string;
  protocol: "sftp"; // v0.1 only sftp; v0.2 adds ftp, v0.3 adds s3 etc.
  host: string;
  port: number;
  username: string;
  auth: AuthMethod;
  defaultRemotePath?: string;
  color?: string;
}

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
  hasDirectories: boolean; // false for object stores once we add them
}

// A SessionId can be either a real SSH session UUID or the sentinel "local".
export type SessionId = string;
export const LOCAL_SESSION: SessionId = "local";

export interface TerminalDataEvent {
  terminalId: string;
  data: string;
}

export interface TerminalExitEvent {
  terminalId: string;
  code: number | null;
}

// Server-side asks the user whether to trust a host fingerprint we've never
// seen, or whose stored key has changed (potential MITM). The user replies via
// `respondToHostPrompt`. Mirrors src-tauri/src/session/mod.rs.
export type HostPromptKind = "unknown" | "mismatch";
export type HostDecision = "accept" | "trust" | "reject";

export interface HostPromptEvent {
  requestId: string;
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
  storedFingerprint?: string | null;
  kind: HostPromptKind;
}

export type TransferKind = "download" | "upload";
export type TransferStatus =
  | "queued"
  | "transferring"
  | "done"
  | "skipped"
  | "error"
  | "canceled";
export type OverwritePolicy = "overwrite" | "skip" | "rename";

export interface Transfer {
  id: string;
  kind: TransferKind;
  source: string;
  destination: string;
  size: number;
  transferred: number;
  status: TransferStatus;
  error?: string;
  startedAt: number;
}
