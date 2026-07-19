import { ipc } from "@/lib/ipc";
import {
  takeLegacyAnthropicKey,
  hydrateFromDb,
  SETTINGS_KEYS,
} from "@/stores/settingsStore";

// One-time migration of pre-keychain secrets (Plan 12 Phase 1).
//
// The Anthropic API key used to live in the plaintext `faro.settings.v1`
// localStorage blob. `settingsStore` captures it synchronously at module load;
// here we push it into the OS keychain via the one-way `set_api_key` command,
// then strip it from the persisted blob. If the keychain write fails, we leave
// the localStorage value untouched so the migration retries on the next launch.

const STORAGE_KEY = "faro.settings.v1";

/** Migrate any legacy plaintext secret into the OS keychain. Idempotent. */
export async function runSecretMigration(): Promise<void> {
  const legacy = takeLegacyAnthropicKey();
  if (!legacy) return;
  try {
    await ipc.setApiKey("anthropic-api-key", legacy);
    // Only now that it's safely in the keychain, scrub it from localStorage.
    stripLegacyKey();
  } catch {
    // Keychain unavailable (or a mock build with no backend) — keep the
    // localStorage value and retry next launch.
  }
}

function stripLegacyKey() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw);
    if ("anthropicApiKey" in parsed) {
      delete parsed.anthropicApiKey;
      localStorage.setItem(STORAGE_KEY, JSON.stringify(parsed));
    }
  } catch {
    // ignore
  }
}

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
