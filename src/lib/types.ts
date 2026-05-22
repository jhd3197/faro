// Shared types mirroring the Rust serde structs in src-tauri/src/.
// Keep these in sync with src-tauri/src/remotefs/mod.rs and profiles/mod.rs.

export type AuthMethod =
  | { kind: "password"; password: string }
  | { kind: "key"; path: string; passphrase?: string }
  | { kind: "agent" };

export type Protocol = "sftp" | "ftp" | "ftps" | "s3" | "azure";

export interface ConnectionProfile {
  id: string;
  name: string;
  protocol: Protocol;
  host: string;
  port: number;
  username: string;
  auth: AuthMethod;
  defaultRemotePath?: string;
  color?: string;
  // Object-store fields (used when protocol === "s3" or "azure").
  bucket?: string; // bucket (S3) or container (Azure)
  region?: string;
  endpoint?: string;
  account?: string; // Azure storage account name
}

export const PROTOCOL_DEFAULT_PORT: Record<Protocol, number> = {
  sftp: 22,
  ftp: 21,
  ftps: 21,
  s3: 443,
  azure: 443,
};

export const PROTOCOL_LABEL: Record<Protocol, string> = {
  sftp: "SFTP",
  ftp: "FTP",
  ftps: "FTPS",
  s3: "S3",
  azure: "Azure",
};

export function isObjectProtocol(p: Protocol): boolean {
  return p === "s3" || p === "azure";
}

// ---- Importers ----

export interface ProfilePreview {
  previewId: string;
  name: string;
  protocol: Protocol;
  host: string;
  port: number;
  username: string;
  identityFile?: string;
  note?: string;
}

export type ImporterKind = "openssh" | "filezilla" | "putty";

export interface ImporterPaths {
  openssh: string | null;
  filezilla: string | null;
  putty: string | null;
}

// ---- Sync ----

export type SyncDirection = "localToRemote" | "remoteToLocal";
export type SyncStrategy = "additive" | "mirror";
export type SyncReason = "missing" | "newer" | "sizeChanged";

export interface SyncFile {
  relative: string;
  sourcePath: string;
  destinationPath: string;
  size: number;
  reason: SyncReason;
}

export interface SyncDelete {
  relative: string;
  path: string;
  kind: FileKind;
  size: number;
}

export interface SyncPlan {
  direction: SyncDirection;
  strategy: SyncStrategy;
  localRoot: string;
  remoteRoot: string;
  copies: SyncFile[];
  deletes: SyncDelete[];
  totalBytes: number;
}

// ---- Edit-in-place ----

export interface EditStartedEvent {
  editId: string;
  sessionId: string;
  remotePath: string;
  localTempPath: string;
}

export interface EditSavedEvent {
  editId: string;
  remotePath: string;
  bytes: number;
}

export interface EditErrorEvent {
  editId: string;
  remotePath: string;
  message: string;
}

export type S3Provider = "aws" | "r2" | "b2";

export interface S3ProviderPreset {
  label: string;
  description: string;
  endpointHint: string; // displayed as placeholder
  defaultRegion: string;
}

export const S3_PROVIDER_PRESETS: Record<S3Provider, S3ProviderPreset> = {
  aws: {
    label: "AWS S3",
    description: "Native Amazon S3; endpoint derived from region.",
    endpointHint: "(leave blank — derived from region)",
    defaultRegion: "us-east-1",
  },
  r2: {
    label: "Cloudflare R2",
    description: "S3-compatible. Region is always 'auto'.",
    endpointHint: "https://<account>.r2.cloudflarestorage.com",
    defaultRegion: "auto",
  },
  b2: {
    label: "Backblaze B2",
    description: "S3-compatible. Endpoint is per-bucket region.",
    endpointHint: "https://s3.us-west-002.backblazeb2.com",
    defaultRegion: "us-west-002",
  },
};

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
