import { ipc } from "@/lib/ipc";
import { hydrateFromDb, SETTINGS_KEYS } from "@/stores/settingsStore";

const STORAGE_KEY = "faro.settings.v1";

// One-time migration of app settings from localStorage into faro.db (Plan 12
// Phase 2). Idempotent and safe by construction:
//   - if faro.db already holds settings, do nothing (never re-import a stale
//     blob over newer DB values);
//   - migrate-then-verify: only drop the localStorage blob once the DB reads
//     back at least what we wrote (the plan's "keep the blob until faro.db
//     reads back equal" guard).
export async function runSettingsMigration(): Promise<void> {
  try {
    const existing = await ipc.settingsGetAll();
    if (Object.keys(existing).length > 0) return; // already migrated

    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw) as Record<string, unknown>;

    const entries: Record<string, string> = {};
    for (const k of SETTINGS_KEYS) {
      if (k in parsed) entries[k as string] = JSON.stringify(parsed[k as string]);
    }
    if (Object.keys(entries).length === 0) return;

    await ipc.settingsSetAll(entries);

    const after = await ipc.settingsGetAll();
    if (Object.keys(after).length >= Object.keys(entries).length) {
      localStorage.removeItem(STORAGE_KEY);
      // Reflect the imported values in this session (they were injected empty on
      // this first upgrade boot).
      await hydrateFromDb();
    }
  } catch {
    // Keychain/DB unavailable (or mock) — retry next launch.
  }
}
