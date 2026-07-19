// Mock of @tauri-apps/plugin-updater for the demo/headless build. `check()`
// returns a fake Update when `window.__mockUpdate` is set, else null (up to
// date). `downloadAndInstall` streams a couple of progress events and flips
// `window.__installed`, so the verify harness can drive the whole flow.
type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

export type Update = {
  version: string;
  currentVersion: string;
  body: string | null;
  date: string | null;
  available: boolean;
  downloadAndInstall: (onEvent: (e: DownloadEvent) => void) => Promise<void>;
  download: () => Promise<void>;
  install: () => Promise<void>;
  close: () => Promise<void>;
};

export async function check(): Promise<Update | null> {
  const cfg = (window as any).__mockUpdate;
  if (!cfg) return null;
  const size = cfg.size ?? 1000;
  return {
    version: cfg.version,
    currentVersion: cfg.currentVersion ?? "1.3.22",
    body: cfg.body ?? null,
    date: cfg.date ?? null,
    available: true,
    async downloadAndInstall(onEvent) {
      onEvent({ event: "Started", data: { contentLength: size } });
      onEvent({ event: "Progress", data: { chunkLength: Math.floor(size / 2) } });
      onEvent({ event: "Progress", data: { chunkLength: Math.ceil(size / 2) } });
      onEvent({ event: "Finished" });
      (window as any).__installed = true;
    },
    async download() {},
    async install() {},
    async close() {},
  };
}
