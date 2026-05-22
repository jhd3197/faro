import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConnectionProfile,
  DirEntry,
  Capabilities,
  HostDecision,
  HostPromptEvent,
  SessionId,
  TerminalDataEvent,
  TerminalExitEvent,
  Transfer,
  OverwritePolicy,
} from "./types";

// Typed wrappers around the Tauri command surface. The string names must match
// the #[tauri::command] handlers registered in src-tauri/src/commands.rs.

export const ipc = {
  listProfiles: () => invoke<ConnectionProfile[]>("list_profiles"),

  saveProfile: (profile: ConnectionProfile) =>
    invoke<void>("save_profile", { profile }),

  deleteProfile: (id: string) => invoke<void>("delete_profile", { id }),

  connect: (profileId: string) =>
    invoke<SessionId>("connect", { profileId }),

  disconnect: (sessionId: SessionId) =>
    invoke<void>("disconnect", { sessionId }),

  listDirectory: (sessionId: SessionId, path: string) =>
    invoke<DirEntry[]>("list_directory", { sessionId, path }),

  capabilities: (sessionId: SessionId) =>
    invoke<Capabilities>("capabilities", { sessionId }),

  openTerminal: (sessionId: SessionId, cols: number, rows: number) =>
    invoke<string>("open_terminal", { sessionId, cols, rows }),

  terminalWrite: (terminalId: string, data: string) =>
    invoke<void>("terminal_write", { terminalId, data }),

  terminalResize: (terminalId: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { terminalId, cols, rows }),

  closeTerminal: (terminalId: string) =>
    invoke<void>("close_terminal", { terminalId }),

  startDownload: (
    sessionId: SessionId,
    remotePath: string,
    localDir: string,
    overwritePolicy?: OverwritePolicy
  ) =>
    invoke<string>("start_download", {
      sessionId,
      remotePath,
      localDir,
      overwritePolicy,
    }),

  startUpload: (
    sessionId: SessionId,
    localPath: string,
    remoteDir: string,
    overwritePolicy?: OverwritePolicy
  ) =>
    invoke<string>("start_upload", {
      sessionId,
      localPath,
      remoteDir,
      overwritePolicy,
    }),

  cancelTransfer: (transferId: string) =>
    invoke<void>("cancel_transfer", { transferId }),

  listTransfers: () => invoke<Transfer[]>("list_transfers"),

  startDirectoryDownload: (
    sessionId: SessionId,
    remoteDir: string,
    localDir: string,
    overwritePolicy?: OverwritePolicy
  ) =>
    invoke<string[]>("start_directory_download", {
      sessionId,
      remoteDir,
      localDir,
      overwritePolicy,
    }),

  startDirectoryUpload: (
    sessionId: SessionId,
    localDir: string,
    remoteDir: string,
    overwritePolicy?: OverwritePolicy
  ) =>
    invoke<string[]>("start_directory_upload", {
      sessionId,
      localDir,
      remoteDir,
      overwritePolicy,
    }),

  renamePath: (sessionId: SessionId, from: string, to: string) =>
    invoke<void>("rename_path", { sessionId, from, to }),

  deletePath: (sessionId: SessionId, path: string, recursive: boolean) =>
    invoke<void>("delete_path", { sessionId, path, recursive }),

  createDirectory: (sessionId: SessionId, path: string) =>
    invoke<void>("create_directory", { sessionId, path }),

  chmodPath: (sessionId: SessionId, path: string, mode: number) =>
    invoke<void>("chmod_path", { sessionId, path, mode }),

  respondToHostPrompt: (requestId: string, decision: HostDecision) =>
    invoke<void>("respond_to_host_prompt", { requestId, decision }),
};

export async function onHostPrompt(
  cb: (event: HostPromptEvent) => void
): Promise<UnlistenFn> {
  return listen<HostPromptEvent>("host://prompt", (e) => cb(e.payload));
}

export async function onTerminalData(
  cb: (event: TerminalDataEvent) => void
): Promise<UnlistenFn> {
  return listen<TerminalDataEvent>("terminal://data", (e) => cb(e.payload));
}

export async function onTerminalExit(
  cb: (event: TerminalExitEvent) => void
): Promise<UnlistenFn> {
  return listen<TerminalExitEvent>("terminal://exit", (e) => cb(e.payload));
}

export async function onTransferEvent(
  cb: (kind: "added" | "progress" | "done" | "error", t: Transfer) => void
): Promise<UnlistenFn> {
  const unsubs = await Promise.all([
    listen<Transfer>("transfer://added", (e) => cb("added", e.payload)),
    listen<Transfer>("transfer://progress", (e) => cb("progress", e.payload)),
    listen<Transfer>("transfer://done", (e) => cb("done", e.payload)),
    listen<Transfer>("transfer://error", (e) => cb("error", e.payload)),
  ]);
  return () => {
    unsubs.forEach((u) => u());
  };
}
