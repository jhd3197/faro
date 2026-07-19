# Plan 12 — Security, settings & portability hardening

## Context

Four backend-discipline gaps. None are glamorous; all four are the difference
between "hobby app" and "I trust this with my production boxes":

1. **The Anthropic API key sits in plaintext localStorage** (`settingsStore`).
   The rule should be strict: *credentials never cross the IPC boundary* — the
   frontend never sees a secret; Rust fetches from the OS keychain at the
   moment of use. Faro already keychains profile secrets
   (`src-tauri/src/profiles/mod.rs`); the AI key is the leak.
2. **Settings live in localStorage.** It works, but it's webview-private
   (invisible to `faro-cli` and future surfaces), tied to a hand-rolled
   migration key, and it forced the pre-paint theme hack to live in
   `main.tsx`. Settings belong in SQLite, *injected* into the window before
   first paint from Rust — one source of truth, zero flash.
3. **Errors cross IPC as strings.** Every Rust error should serialize as
   `{kind, message}` so the frontend pattern-matches on a stable discriminant
   (auth vs. not-found vs. permission) instead of regexing English. Faro's
   toasts currently get whatever `Display` says.
4. **No backup/restore.** Thirteen backends worth of profiles + keychain
   secrets + `faro.db` state, and moving to a new machine means re-entering
   everything. The fix: an encrypted container (Argon2id + AES-256-GCM) that
   exports the DB *and* all keychain credentials behind one password.

## What already exists (don't rebuild)

- OS keychain integration in profiles (`profiles/mod.rs:64`) — secrets already
  leave the JSON; this plan extends that pattern, doesn't invent it.
- `faro.db` (SQLite via rusqlite, in `AppState`, `db.rs`) with migrations —
  settings become one more table.
- `main.tsx` already applies `data-theme` pre-paint; `styles.css` holds the 16
  themes and accent override (`src/lib/accent.ts`). Only the *source* changes.
- `agent.rs` reads the API key at call time — swapping its source is a small,
  contained change.

## Approach

### Phase 1 — Keychain-everything credentials
- Move the Anthropic API key (and any future service keys) to the OS keychain
  via the existing keyring path, service `com.faro.credentials`, keyed by
  purpose (`anthropic-api-key`, …).
- **Frontend never sees the value.** Settings UI shows a Set/••••/Clear
  affordance; `agent.rs` fetches the key from the keychain inside Rust at
  request time. One-way write command `set_api_key`, no read command.
- Migrate on upgrade: if a key exists in localStorage, write it to the
  keychain, then delete it from the persisted store.
- Audit: grep the frontend for every secret-shaped value; only non-secrets
  stay in settings.

### Phase 2 — Settings in `faro.db` + pre-paint injection
- `settings` table (key TEXT PRIMARY KEY, value TEXT/JSON) + `settings_get_all`
  / `settings_set` commands; a small Rust-side typed accessor.
- **Pre-paint injection:** create the window in `setup()` with an
  `initialization_script` that reads theme/accent/font from `faro.db` and sets
  `data-theme` etc. on `<html>` before first paint (kills the dark→light flash
  structurally, not by timing luck). The settings store then *seeds itself from
  the injected DOM attributes* on boot, then hydrates the rest via
  `settings_get_all`.
- One-time migration from the localStorage store (keep the view-migration key
  logic as the importer; delete after successful import).
- Keep the store API unchanged so components don't churn — only the
  persistence adapter moves.

### Phase 3 — Structured errors across IPC
- Every Rust error enum that crosses a command boundary serializes as
  `{kind, message}` (serde tag). New `FaroError` helper so new commands get it
  by construction.
- Frontend: `src/lib/errors.ts` — `kindOf(e)` + a toast mapper (auth →
  "reconnect", permission → actionable hint, not-found → …). No string
  matching on messages.
- Roll out command-group by command-group (transfer, remotefs, bridge,
  terminal); old string errors keep rendering as generic toasts until migrated.

### Phase 4 — Encrypted backup / restore
- Container: magic `FAROBAK\x01` + version + Argon2id (64 MiB, t=3) →
  AES-256-GCM over a gzipped archive; header as AEAD associated data so a wrong
  password fails the tag cleanly, never partially decrypts.
- Contents: profiles JSON, `faro.db` snapshot, bridge config, sync pairs, and
  **all keychain credentials** (enumerated and re-injected on restore).
- Commands `backup_export(path, password)` / `backup_import(path, password)`
  (restore requires app restart or a clean state reload); UI in Settings →
  danger zone, with a plain "what's inside" summary before export.
- `faro-cli backup export/import` for headless migration.

## Integration points
- `src-tauri/src/profiles/mod.rs` (keyring reuse), `agent.rs` (key source),
  `db.rs` (settings table), `lib.rs` (window `initialization_script`, new
  commands), new `src-tauri/src/backup.rs`.
- `src/stores/settingsStore.ts` (adapter swap), `src/lib/errors.ts` (new),
  `src/components/Settings.tsx` (API-key affordance + backup UI), `faro-cli`.

## Risks
- **Settings migration bricking first-run** — migrate-then-verify counts;
  keep the localStorage blob untouched until `faro.db` reads back equal.
- **Keychain enumeration for backup is platform-specific** — Windows DPAPI and
  macOS Keychain don't support listing; keep Faro's own manifest of keychain
  keys (service + account names) in `faro.db` so export knows what to pull.
- **initialization_script ordering** — must run before any stylesheet paints;
  verify on a slow machine / light theme that no flash regresses.
- **Scope discipline** — do NOT redesign settings UI or error copy in this
  plan; adapters only.

## Verification
`cargo check -p faro` + `npx tsc --noEmit` clean. Runtime: set the API key via
UI → confirm nothing secret-shaped remains in webview devtools localStorage and
AI chat still calls; flip theme with devtools throttled → no flash on reload;
trigger an auth error → toast keyed off `kind`; export a backup, wipe app data
on a second profile/machine, import, and confirm connections + AI key work
without re-entering anything. Wrong-password import fails cleanly.
