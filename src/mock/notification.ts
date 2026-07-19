// Mock of @tauri-apps/plugin-notification for the demo/headless build. Records
// every sent notification on `window.__notifications` so the verify harness can
// assert on them, and treats permission as granted. `onAction` returns a no-op.
type Notif = { title?: string; body?: string };

function sink(): Notif[] {
  const w = window as any;
  w.__notifications ??= [];
  return w.__notifications;
}

export async function isPermissionGranted(): Promise<boolean> {
  return true;
}

export async function requestPermission(): Promise<string> {
  return "granted";
}

export function sendNotification(opts: Notif | string): void {
  const n = typeof opts === "string" ? { body: opts } : opts;
  sink().push(n);
}

// Mirrors the real plugin's PluginListener ({ unregister() }).
export async function onAction(
  _cb: (n: unknown) => void
): Promise<{ unregister: () => Promise<void> }> {
  return { unregister: async () => {} };
}

export async function onNotificationReceived(
  _cb: (n: unknown) => void
): Promise<{ unregister: () => Promise<void> }> {
  return { unregister: async () => {} };
}
