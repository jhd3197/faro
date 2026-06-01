import type { ReactNode } from "react";
import {
  Plug,
  Unplug,
  Download,
  Plus,
  Settings as SettingsIcon,
  Info,
  TerminalSquare,
  ArrowDownUp,
  RefreshCw,
  Keyboard,
  Palette,
  SunMoon,
} from "lucide-react";
import { useConnections } from "@/stores/connectionsStore";
import { useTransfers } from "@/stores/transfersStore";
import { useLayout } from "@/stores/layoutStore";
import { useSettings, APP_THEMES } from "@/stores/settingsStore";

export interface Command {
  id: string;
  title: string;
  group: string;
  icon?: ReactNode;
  combo?: string; // canonical, e.g. "mod+t"
  keywords?: string;
  enabled?: boolean; // default true
  run: () => void;
}

// Single source of truth for app actions. Reads stores directly and returns
// freshly-bound command objects, so the palette, the shortcut hook and the
// cheat-sheet all stay in sync with current state.
export function useCommands(): Command[] {
  const profiles = useConnections((s) => s.profiles);
  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const connect = useConnections((s) => s.connect);
  const disconnect = useConnections((s) => s.disconnect);

  const togglePanel = useTransfers((s) => s.togglePanel);

  const toggleTerminal = useLayout((s) => s.toggleTerminal);
  const openDialog = useLayout((s) => s.openDialog);
  const setShortcutsOpen = useLayout((s) => s.setShortcutsOpen);

  const appTheme = useSettings((s) => s.appTheme);
  const setAppTheme = useSettings((s) => s.setAppTheme);

  const activeProfile = profiles.find((p) => p.id === activeProfileId);
  const supportsTerminal = activeProfile?.protocol === "sftp";

  const cmds: Command[] = [
    {
      id: "new-connection",
      title: "New Connection…",
      group: "File",
      icon: <Plus size={14} />,
      combo: "mod+n",
      run: () => openDialog("newConnection"),
    },
    {
      id: "import",
      title: "Import Connections…",
      group: "File",
      icon: <Download size={14} />,
      run: () => openDialog("import"),
    },
    {
      id: "settings",
      title: "Open Settings",
      group: "General",
      icon: <SettingsIcon size={14} />,
      combo: "mod+,",
      run: () => openDialog("settings"),
    },
    {
      id: "shortcuts",
      title: "Keyboard Shortcuts",
      group: "General",
      icon: <Keyboard size={14} />,
      combo: "mod+/",
      run: () => setShortcutsOpen(true),
    },
    {
      id: "about",
      title: "About Faro",
      group: "General",
      icon: <Info size={14} />,
      run: () => openDialog("about"),
    },
    {
      id: "reload",
      title: "Reload Window",
      group: "General",
      icon: <RefreshCw size={14} />,
      combo: "mod+r",
      run: () => window.location.reload(),
    },
    {
      id: "toggle-transfers",
      title: "Toggle Transfer Panel",
      group: "View",
      icon: <ArrowDownUp size={14} />,
      combo: "mod+t",
      run: togglePanel,
    },
    {
      id: "toggle-terminal",
      title: "Toggle Terminal",
      group: "View",
      icon: <TerminalSquare size={14} />,
      combo: "mod+`",
      enabled: !!activeSessionId && supportsTerminal,
      run: toggleTerminal,
    },
    {
      id: "switch-theme",
      title: "Toggle Light / Dark Theme",
      group: "View",
      icon: <SunMoon size={14} />,
      combo: "mod+shift+t",
      run: () => setAppTheme(appTheme === "light" ? "dark" : "light"),
    },
  ];

  if (activeSessionId) {
    cmds.push({
      id: "disconnect",
      title: "Disconnect",
      group: "Connection",
      icon: <Unplug size={14} />,
      run: () => disconnect(),
    });
  }

  for (const p of profiles) {
    cmds.push({
      id: `connect:${p.id}`,
      title: `Connect: ${p.name}`,
      group: "Connection",
      icon: <Plug size={14} />,
      keywords: `${p.username} ${p.host} ${p.protocol}`,
      enabled: activeProfileId !== p.id,
      run: () => {
        connect(p.id).catch(() => {});
      },
    });
  }

  for (const t of APP_THEMES) {
    cmds.push({
      id: `theme:${t.value}`,
      title: `Theme: ${t.label}`,
      group: "Theme",
      icon: <Palette size={14} />,
      enabled: appTheme !== t.value,
      run: () => setAppTheme(t.value),
    });
  }

  return cmds;
}
