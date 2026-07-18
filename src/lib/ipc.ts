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
  Skill,
  SkillRunResult,
  SkillDryRunResult,
  DiscoveredAgent,
  AgentPairResult,
  AgentHostStatus,
  CliStatus,
  CliUpdateMode,
  DeepLink,
  SyncPair,
  PairView,
  VirtualFsRootStatus,
  ScanSnapshot,
  DiskScanProgress,
  DiffSnapshot,
  DiffProgress,
  SearchQuery,
  SearchSnapshot,
  SearchProgress,
  SearchHitBatch,
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

  /** Run the interactive Dropbox OAuth flow for a profile (opens the browser,
   *  catches the loopback redirect, stores tokens in the OS keychain). Resolves
   *  with the authorized account label. The editor saves the profile on success. */
  dropboxAuthorize: (profileId: string) =>
    invoke<{ accountLabel: string }>("dropbox_authorize", { profileId }),

  /** Run the interactive OneDrive OAuth flow for a profile (Microsoft Graph). */
  onedriveAuthorize: (profileId: string) =>
    invoke<{ accountLabel: string }>("onedrive_authorize", { profileId }),

  /** Run the interactive Google Drive OAuth flow for a profile. */
  gdriveAuthorize: (profileId: string) =>
    invoke<{ accountLabel: string }>("gdrive_authorize", { profileId }),

  /** Run the interactive Box OAuth flow for a profile. */
  boxAuthorize: (profileId: string) =>
    invoke<{ accountLabel: string }>("box_authorize", { profileId }),

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

  // --- CLI updater (Plan 10 Phase 0c/0d): keep faro-cli in step with the app.
  cliUpdaterStatus: () => invoke<CliStatus>("cli_updater_status"),
  cliUpdaterCheck: () => invoke<CliStatus>("cli_updater_check"),
  cliUpdaterUpdate: () => invoke<CliStatus>("cli_updater_update"),
  cliUpdaterSetMode: (mode: CliUpdateMode) =>
    invoke<CliStatus>("cli_updater_set_mode", { mode }),

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
  // Skills (Plan 8): fleet automations. Hand-authored skills are born approved;
  // AI proposals are approved via bridgeApproveSkill before they can run.
  bridgeListSkills: () => invoke<Skill[]>("bridge_list_skills"),
  bridgeSaveSkill: (skill: Skill) => invoke<Skill[]>("bridge_save_skill", { skill }),
  bridgeDeleteSkill: (id: string) => invoke<Skill[]>("bridge_delete_skill", { id }),
  bridgeApproveSkill: (id: string) => invoke<Skill[]>("bridge_approve_skill", { id }),
  bridgeRunSkill: (
    name: string,
    params: Record<string, string>,
    targets: string[] | null,
    dryRun: boolean,
  ) =>
    invoke<SkillRunResult | SkillDryRunResult>("bridge_run_skill", {
      name,
      params,
      targets,
      dryRun,
    }),
  // Write the agent console text to disk (Downloads) and return the saved path.
  exportAgentLog: (content: string, name: string) =>
    invoke<string>("export_agent_log", { content, name }),

  // ---- Folder Sync (continuous watched sync pairs) ----
  // Every command returns the full, updated list of pairs.
  folderSyncList: () => invoke<PairView[]>("foldersync_list"),
  folderSyncUpsert: (pair: SyncPair) =>
    invoke<PairView[]>("foldersync_upsert", { pair }),
  folderSyncRemove: (id: string) =>
    invoke<PairView[]>("foldersync_remove", { id }),
  folderSyncSetEnabled: (id: string, enabled: boolean) =>
    invoke<PairView[]>("foldersync_set_enabled", { id, enabled }),
  folderSyncSyncNow: (id: string) =>
    invoke<PairView[]>("foldersync_sync_now", { id }),

  // ---- On-demand virtual folders (Plan 9) ----
  /** Whether on-demand placeholders are available (Windows + `virtualfs` build). */
  virtualFsSupported: () => invoke<boolean>("virtualfs_supported"),
  /** Live status of every registered on-demand sync root. */
  virtualFsStatus: () => invoke<VirtualFsRootStatus[]>("virtualfs_status"),
  /** Explorer-style "free up space": dehydrate a pair's hydrated files back to
   *  placeholders. Returns the count dehydrated. */
  virtualFsFreeUpSpace: (pairId: string) =>
    invoke<number>("virtualfs_free_up_space", { pairId }),

  // ---- Disk Usage Explorer ----
  /** Kick off a recursive size scan of `path` on a session; returns a scan id. */
  diskScanStart: (sessionId: SessionId, path: string) =>
    invoke<string>("diskscan_start", { sessionId, path }),
  /** Lightweight status (live counts while scanning). */
  diskScanStatus: (scanId: string) =>
    invoke<ScanSnapshot>("diskscan_status", { scanId }),
  /** Full snapshot including the aggregated tree (present once done). */
  diskScanTree: (scanId: string) =>
    invoke<ScanSnapshot>("diskscan_tree", { scanId }),
  diskScanCancel: (scanId: string) =>
    invoke<void>("diskscan_cancel", { scanId }),
  /** Drop a finished scan from the backend (tab closed). */
  diskScanForget: (scanId: string) =>
    invoke<void>("diskscan_forget", { scanId }),

  /** Start a two-tree directory diff; returns a diff id. Each side is a session
   *  id (or LOCAL_SESSION) + a path. `hash` confirms same-size files by content. */
  diffStart: (
    sessionA: SessionId,
    pathA: string,
    sessionB: SessionId,
    pathB: string,
    hash: boolean
  ) =>
    invoke<string>("diff_start", { sessionA, pathA, sessionB, pathB, hash }),
  /** Lightweight status (live counts while comparing). */
  diffStatus: (diffId: string) =>
    invoke<DiffSnapshot>("diff_status", { diffId }),
  /** Full snapshot including the result (present once done). */
  diffResult: (diffId: string) =>
    invoke<DiffSnapshot>("diff_result", { diffId }),
  diffCancel: (diffId: string) => invoke<void>("diff_cancel", { diffId }),
  /** Drop a finished diff from the backend (view closed). */
  diffForget: (diffId: string) => invoke<void>("diff_forget", { diffId }),

  /** Start a fleet search under `path` on a session; returns a search id. */
  searchStart: (sessionId: SessionId, path: string, query: SearchQuery) =>
    invoke<string>("search_start", { sessionId, path, query }),
  /** Lightweight status (live counts while searching). */
  searchStatus: (searchId: string) =>
    invoke<SearchSnapshot>("search_status", { searchId }),
  /** Full snapshot including the hit list (present once done). */
  searchResult: (searchId: string) =>
    invoke<SearchSnapshot>("search_result", { searchId }),
  searchCancel: (searchId: string) =>
    invoke<void>("search_cancel", { searchId }),
  /** Drop a finished search from the backend (panel closed). */
  searchForget: (searchId: string) =>
    invoke<void>("search_forget", { searchId }),
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

/** The AI proposed a new Skill over the bridge (awaiting human approval). */
export async function onBridgeSkillProposed(
  cb: (skill: Skill) => void
): Promise<UnlistenFn> {
  return listen<Skill>("bridge://skill-proposed", (e) => cb(e.payload));
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

/** CLI version-drift status changed (startup check, or an update finished). */
export async function onCliUpdaterStatus(
  cb: (status: CliStatus) => void
): Promise<UnlistenFn> {
  return listen<CliStatus>("cli-updater://status", (e) => cb(e.payload));
}

/** A folder-sync pair changed (config edited, or a background sync updated its
 *  runtime state). Payload is empty — re-fetch the list on receipt. */
export async function onFolderSyncChanged(
  cb: () => void
): Promise<UnlistenFn> {
  return listen("foldersync://changed", () => cb());
}

/** A faro:// deep link was opened (from a hosting panel like ServerKit). */
export async function onDeepLink(
  cb: (link: DeepLink) => void
): Promise<UnlistenFn> {
  return listen<DeepLink>("deep-link://open", (e) => cb(e.payload));
}

/** Disk-usage scan lifecycle. `progress` carries live counts; the terminal
 *  events (`done`/`error`/`canceled`) carry a full ScanSnapshot with the tree. */
export async function onDiskScanEvent(
  cb: (
    kind: "progress" | "done" | "error" | "canceled",
    payload: DiskScanProgress | ScanSnapshot
  ) => void
): Promise<UnlistenFn> {
  const unsubs = await Promise.all([
    listen<DiskScanProgress>("diskscan://progress", (e) =>
      cb("progress", e.payload)
    ),
    listen<ScanSnapshot>("diskscan://done", (e) => cb("done", e.payload)),
    listen<ScanSnapshot>("diskscan://error", (e) => cb("error", e.payload)),
    listen<ScanSnapshot>("diskscan://canceled", (e) =>
      cb("canceled", e.payload)
    ),
  ]);
  return () => {
    unsubs.forEach((u) => u());
  };
}

/** Directory-diff lifecycle. `progress` carries live file counts + phase; the
 *  terminal events (`done`/`error`/`canceled`) carry a full DiffSnapshot. */
export async function onDiffEvent(
  cb: (
    kind: "progress" | "done" | "error" | "canceled",
    payload: DiffProgress | DiffSnapshot
  ) => void
): Promise<UnlistenFn> {
  const unsubs = await Promise.all([
    listen<DiffProgress>("diff://progress", (e) => cb("progress", e.payload)),
    listen<DiffSnapshot>("diff://done", (e) => cb("done", e.payload)),
    listen<DiffSnapshot>("diff://error", (e) => cb("error", e.payload)),
    listen<DiffSnapshot>("diff://canceled", (e) => cb("canceled", e.payload)),
  ]);
  return () => {
    unsubs.forEach((u) => u());
  };
}

/** Fleet-search lifecycle. `progress` carries live counts + strategy, `hit`
 *  streams batches of new hits, and the terminal events (`done`/`error`/
 *  `canceled`) carry a full SearchSnapshot (fetch the hit list on `done`). */
export async function onSearchEvent(
  cb: (
    kind: "progress" | "hit" | "done" | "error" | "canceled",
    payload: SearchProgress | SearchHitBatch | SearchSnapshot
  ) => void
): Promise<UnlistenFn> {
  const unsubs = await Promise.all([
    listen<SearchProgress>("search://progress", (e) => cb("progress", e.payload)),
    listen<SearchHitBatch>("search://hit", (e) => cb("hit", e.payload)),
    listen<SearchSnapshot>("search://done", (e) => cb("done", e.payload)),
    listen<SearchSnapshot>("search://error", (e) => cb("error", e.payload)),
    listen<SearchSnapshot>("search://canceled", (e) => cb("canceled", e.payload)),
  ]);
  return () => {
    unsubs.forEach((u) => u());
  };
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
