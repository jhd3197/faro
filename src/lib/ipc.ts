import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConnectionProfile,
  DirEntry,
  Capabilities,
  EditErrorEvent,
  EditSavedEvent,
  EditStartedEvent,
  HostDecision,
  HostPromptEvent,
  AuthPromptEvent,
  AuthChangedEvent,
  ImporterPaths,
  ProfilePreview,
  SessionId,
  SyncDirection,
  SyncPlan,
  SyncStrategy,
  TerminalDataEvent,
  TerminalExitEvent,
  Transfer,
  OverwritePolicy,
  BridgeStatus,
  BridgeActivity,
  BridgeApproval,
  ApprovalDecision,
  ApprovalPolicy,
  AgentExecStart,
  AgentOutput,
  SavedCommand,
  DiscoveredAgent,
  AgentPairResult,
  AgentHostStatus,
  DeepLink,
} from "./types";

// Typed wrappers around the Tauri command surface. The string names must match
// the #[tauri::command] handlers registered in src-tauri/src/commands.rs.

export const ipc = {
  listProfiles: () => invoke<ConnectionProfile[]>("list_profiles"),

  saveProfile: (profile: ConnectionProfile) =>
    invoke<void>("save_profile", { profile }),

  /** Persist a manual rail order: every profile id, in display order. */
  reorderProfiles: (ids: string[]) =>
    invoke<void>("reorder_profiles", { ids }),

  deleteProfile: (id: string) => invoke<void>("delete_profile", { id }),

  connect: (profileId: string) =>
    invoke<SessionId>("connect", { profileId }),

  disconnect: (sessionId: SessionId) =>
    invoke<void>("disconnect", { sessionId }),

  // ---- Faro Agent (Faro-to-Faro remote control) ----

  /** Browse the LAN for faro-agentd daemons over mDNS (best-effort). */
  discoverAgents: () => invoke<DiscoveredAgent[]>("discover_agents"),

  /** This controller's own agent public key, for the pairing UI. */
  agentPublicKey: () => invoke<string>("agent_public_key"),

  /** Pair with a faro-agentd at host:port using a one-time 6-digit code.
   *  Returns the daemon's key to pin into the profile; persists nothing itself,
   *  so a failed pairing leaves no half-configured connection behind. */
  pairAgent: (host: string, port: number, code: string) =>
    invoke<AgentPairResult>("pair_agent", { host, port, code }),

  // ---- Remote control: host THIS machine as a Faro Agent (Settings) ----

  /** Current state of the in-app agent host (enabled, running, policy, peers,
   *  open pairing window). */
  agentHostStatus: () => invoke<AgentHostStatus>("agent_host_status"),

  /** Turn the host on/off; optionally set the listen port. */
  agentHostSetEnabled: (enabled: boolean, port?: number) =>
    invoke<AgentHostStatus>("agent_host_set_enabled", { enabled, port }),

  /** Open a pairing window and get the fresh 6-digit code to read aloud. */
  agentHostOpenPairing: () =>
    invoke<AgentHostStatus>("agent_host_open_pairing"),

  /** Close the pairing window early (e.g. the dialog closed). */
  agentHostClosePairing: () =>
    invoke<AgentHostStatus>("agent_host_close_pairing"),

  /** Set what paired controllers may do on THIS machine. */
  agentHostSetPolicy: (allowExec: boolean, allowWrite: boolean) =>
    invoke<AgentHostStatus>("agent_host_set_policy", { allowExec, allowWrite }),

  /** Un-pin a controller by its public key. */
  agentHostRevokePeer: (publicKey: string) =>
    invoke<AgentHostStatus>("agent_host_revoke_peer", { publicKey }),

  listDirectory: (sessionId: SessionId, path: string) =>
    invoke<DirEntry[]>("list_directory", { sessionId, path }),

  capabilities: (sessionId: SessionId) =>
    invoke<Capabilities>("capabilities", { sessionId }),

  // Base64 of a small local file, for image thumbnails in the grid view.
  readFilePreview: (sessionId: SessionId, path: string) =>
    invoke<string>("read_file_preview", { sessionId, path }),

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

  // Copy a file/folder alongside the original; backend picks a free name.
  duplicatePath: (sessionId: SessionId, path: string) =>
    invoke<void>("duplicate_path", { sessionId, path }),

  // Build a tar.gz/zip of a remote dir on the server, then download it. Returns
  // the resulting transfer id (the archive shows in the transfers panel).
  startArchiveDownload: (
    sessionId: SessionId,
    remotePath: string,
    format: "tar.gz" | "zip"
  ) =>
    invoke<string>("start_archive_download", {
      sessionId,
      remotePath,
      format,
    }),

  respondToHostPrompt: (requestId: string, decision: HostDecision) =>
    invoke<void>("respond_to_host_prompt", { requestId, decision }),

  // Answer a keyboard-interactive auth prompt; pass null to cancel (aborts connect).
  respondToAuthPrompt: (requestId: string, responses: string[] | null) =>
    invoke<void>("respond_to_auth_prompt", { requestId, responses }),

  importerDefaultPaths: () => invoke<ImporterPaths>("importer_default_paths"),

  importOpenssh: (path?: string) =>
    invoke<ProfilePreview[]>("import_openssh", { path }),

  importFilezilla: (path?: string) =>
    invoke<ProfilePreview[]>("import_filezilla", { path }),

  importPutty: () => invoke<ProfilePreview[]>("import_putty"),

  saveImportedProfiles: (previews: ProfilePreview[]) =>
    invoke<number>("save_imported_profiles", { previews }),

  syncPlan: (
    sessionId: SessionId,
    localPath: string,
    remotePath: string,
    direction: SyncDirection,
    strategy: SyncStrategy
  ) =>
    invoke<SyncPlan>("sync_plan", {
      sessionId,
      localPath,
      remotePath,
      direction,
      strategy,
    }),

  syncExecute: (sessionId: SessionId, plan: SyncPlan) =>
    invoke<string[]>("sync_execute", { sessionId, plan }),

  startEdit: (sessionId: SessionId, remotePath: string, editor?: string) =>
    invoke<EditStartedEvent>("start_edit", { sessionId, remotePath, editor }),

  stopEdit: (editId: string) => invoke<void>("stop_edit", { editId }),

  // Agent Bridge
  bridgeStart: () => invoke<BridgeStatus>("bridge_start"),
  bridgeStop: () => invoke<BridgeStatus>("bridge_stop"),
  bridgeSetEnabled: (enabled: boolean) =>
    invoke<BridgeStatus>("bridge_set_enabled", { enabled }),
  bridgeStatus: () => invoke<BridgeStatus>("bridge_status"),
  bridgeSetSessionAccess: (sessionId: SessionId, enabled: boolean) =>
    invoke<BridgeStatus>("bridge_set_session_access", { sessionId, enabled }),
  bridgeSetPolicy: (policy: ApprovalPolicy) =>
    invoke<BridgeStatus>("bridge_set_policy", { policy }),
  bridgeSetActiveSession: (sessionId: SessionId | null) =>
    invoke<void>("bridge_set_active_session", { sessionId }),
  bridgeRegisterMcp: (url: string, token: string) =>
    invoke<string>("bridge_register_mcp", { url, token }),
  agentChat: (req: {
    apiKey: string;
    sessionId: string | null;
    prompt: string;
    history: Array<{ role: "user" | "assistant"; content: string }>;
  }) => invoke<{ content: string }>("agent_chat_cmd", { req }),
  respondToBridgeApproval: (requestId: string, decision: ApprovalDecision) =>
    invoke<void>("respond_to_bridge_approval", { requestId, decision }),
  bridgeActivity: () => invoke<BridgeActivity[]>("bridge_activity"),
  bridgeClearActivity: () => invoke<void>("bridge_clear_activity"),
  // Saved commands (pre-approved; managed only here, never over the bridge).
  bridgeListCommands: () => invoke<SavedCommand[]>("bridge_list_commands"),
  bridgeSaveCommand: (command: SavedCommand) =>
    invoke<SavedCommand[]>("bridge_save_command", { command }),
  bridgeDeleteCommand: (id: string) =>
    invoke<SavedCommand[]>("bridge_delete_command", { id }),
  // Write the agent console text to disk (Downloads) and return the saved path.
  exportAgentLog: (content: string, name: string) =>
    invoke<string>("export_agent_log", { content, name }),
};

export async function onEditSaved(
  cb: (event: EditSavedEvent) => void
): Promise<UnlistenFn> {
  return listen<EditSavedEvent>("editor://saved", (e) => cb(e.payload));
}

export async function onEditError(
  cb: (event: EditErrorEvent) => void
): Promise<UnlistenFn> {
  return listen<EditErrorEvent>("editor://error", (e) => cb(e.payload));
}

export async function onHostPrompt(
  cb: (event: HostPromptEvent) => void
): Promise<UnlistenFn> {
  return listen<HostPromptEvent>("host://prompt", (e) => cb(e.payload));
}

export async function onAuthPrompt(
  cb: (event: AuthPromptEvent) => void
): Promise<UnlistenFn> {
  return listen<AuthPromptEvent>("auth://prompt", (e) => cb(e.payload));
}

export async function onAuthChanged(
  cb: (event: AuthChangedEvent) => void
): Promise<UnlistenFn> {
  return listen<AuthChangedEvent>("auth://changed", (e) => cb(e.payload));
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

export async function onBridgeApproval(
  cb: (event: BridgeApproval) => void
): Promise<UnlistenFn> {
  return listen<BridgeApproval>("bridge://approval", (e) => cb(e.payload));
}

export async function onBridgeActivity(
  cb: (event: BridgeActivity) => void
): Promise<UnlistenFn> {
  return listen<BridgeActivity>("bridge://activity", (e) => cb(e.payload));
}

export async function onAgentExecStart(
  cb: (event: AgentExecStart) => void
): Promise<UnlistenFn> {
  return listen<AgentExecStart>("agent://exec-start", (e) => cb(e.payload));
}

export async function onAgentOutput(
  cb: (event: AgentOutput) => void
): Promise<UnlistenFn> {
  return listen<AgentOutput>("agent://output", (e) => cb(e.payload));
}

/** A controller finished pairing with THIS machine's hosted agent. */
export async function onAgentHostPaired(
  cb: (event: { name: string; key: string }) => void
): Promise<UnlistenFn> {
  return listen<{ name: string; key: string }>("agent-host://paired", (e) =>
    cb(e.payload)
  );
}

/** A faro:// deep link was opened (from a hosting panel like ServerKit). */
export async function onDeepLink(
  cb: (link: DeepLink) => void
): Promise<UnlistenFn> {
  return listen<DeepLink>("deep-link://open", (e) => cb(e.payload));
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
