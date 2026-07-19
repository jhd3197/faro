import { ipc } from "@/lib/ipc";
import { takeLegacyAnthropicKey } from "@/stores/settingsStore";

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
