// Mock of @tauri-apps/api/core `invoke` for the demo build. Returns fictional
// data so the whole UI renders populated without the Rust backend. Unhandled
// commands resolve to null, which is harmless for the screens we capture.
import * as data from "./data";
import { emit } from "./event";

type Args = Record<string, any>;

const emittedTerminals = new Set<string>();

export async function invoke<T = unknown>(cmd: string, args: Args = {}): Promise<T> {
  const out = await dispatch(cmd, args);
  return out as T;
}

async function dispatch(cmd: string, a: Args): Promise<unknown> {
  switch (cmd) {
    // ---- profiles ----
    case "list_profiles":
      return data.profiles;
    case "save_profile": {
      const p = a.profile;
      const i = data.profiles.findIndex((x) => x.id === p.id);
      data.setProfiles(
        i >= 0
          ? data.profiles.map((x) => (x.id === p.id ? p : x))
          : [...data.profiles, p]
      );
      return null;
    }
    case "delete_profile":
      data.setProfiles(data.profiles.filter((x) => x.id !== a.id));
      return null;

    // ---- sessions ----
    case "connect":
      return data.openSession(a.profileId);
    case "disconnect":
      data.closeSession(a.sessionId);
      return null;

    // ---- files ----
    case "list_directory":
      return data.listDir(a.sessionId, a.path);
    case "capabilities":
      return data.capabilities(a.sessionId);
    case "read_file_preview":
      throw new Error("no preview in demo mode");
    case "rename_path":
    case "delete_path":
    case "create_directory":
    case "chmod_path":
    case "duplicate_path":
      return null;
    case "start_archive_download":
      return "t-archive";

    // ---- terminal ----
    case "open_terminal": {
      const id = "term-1";
      // Push the canned transcript once the xterm is listening. Guard against
      // StrictMode's double-mount so the transcript isn't emitted twice.
      if (!emittedTerminals.has(id)) {
        emittedTerminals.add(id);
        setTimeout(() => emit("terminal://data", { terminalId: id, data: data.TERMINAL_TRANSCRIPT }), 120);
      }
      return id;
    }
    case "terminal_write":
    case "terminal_resize":
    case "close_terminal":
      return null;

    // ---- transfers ----
    case "list_transfers":
      return data.transfers;
    case "start_download":
    case "start_upload":
    case "start_archive":
      return "t-new";
    case "start_directory_download":
    case "start_directory_upload":
      return ["t-new"];
    case "cancel_transfer":
      return null;

    // ---- importers ----
    case "importer_default_paths":
      return { openssh: "~/.ssh/config", filezilla: null, putty: null };
    case "import_openssh":
    case "import_filezilla":
    case "import_putty":
      return [];
    case "save_imported_profiles":
      return 0;

    // ---- sync ----
    case "sync_plan":
      return data.syncPlan(a.localPath, a.remotePath);
    case "sync_execute":
      return ["t-sync"];

    // ---- edit-in-place ----
    case "start_edit":
      return {
        editId: "e1",
        sessionId: a.sessionId,
        remotePath: a.remotePath,
        localTempPath: "C:\\Users\\demo\\AppData\\Local\\Temp\\faro-edits\\server.js",
      };
    case "stop_edit":
      return null;

    // ---- agent bridge ----
    case "bridge_status":
    case "bridge_start":
    case "bridge_stop":
    case "bridge_set_enabled":
    case "bridge_set_session_access":
    case "bridge_set_policy":
    case "bridge_register_mcp":
      return data.bridgeStatus;
    case "bridge_activity":
      return data.bridgeActivity;
    case "bridge_clear_activity":
      return null;
    case "bridge_list_commands":
    case "bridge_save_command":
    case "bridge_delete_command":
      return data.savedCommands;
    case "bridge_set_active_session":
      return null;
    case "agent_chat_cmd":
      return { content: "The API service is healthy — 37 days uptime, load 0.08." };
    case "export_agent_log":
      return "C:\\Users\\demo\\Downloads\\faro-agent-console.txt";

    default:
      return null;
  }
}
