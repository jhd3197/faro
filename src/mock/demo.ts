// Demo bootstrap: exposes the zustand stores on `window.__demo` so the headless
// screenshot script (scripts/capture-screenshots.mjs) can drive the UI into each
// state — open dialogs, switch sessions, expand the rail, etc. Loaded only when
// VITE_MOCK is set (see main.tsx).
import { useLayout } from "@/stores/layoutStore";
import { useConnections } from "@/stores/connectionsStore";
import { useTransfers } from "@/stores/transfersStore";
import { useBridge } from "@/stores/bridgeStore";
import { useTerminals } from "@/stores/terminalsStore";
import { useSettings } from "@/stores/settingsStore";
import { useDiskScan } from "@/stores/diskScanStore";
import { emit } from "./event";

(window as any).__demo = {
  useLayout,
  useConnections,
  useTransfers,
  useBridge,
  useTerminals,
  useSettings,
  useDiskScan,
  // Convenience: focus a server by profile id (connects if needed).
  focusProfile: (profileId: string) =>
    useConnections.getState().connect(profileId),
  // Push a mock Tauri event (e.g. a bridge://approval) for screenshots.
  emit,
};
