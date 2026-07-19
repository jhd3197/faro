# Plan 16 — In-app auto-updater, desktop notifications & one-click PATH install

## Context

Three trust/UX gaps for an app at v1.3.22:

1. **No app updater.** Plan 10 shipped version-drift + self-update for
   *faro-cli* (`src-tauri/src/cli_updater.rs`), but the desktop app itself has
   nothing — `tauri-plugin-updater` isn't in `Cargo.toml`, so every app update
   is a manual download. This also undercuts Plan 10: the CLI drift check
   detects "app newer than CLI", but the app can't update *itself*.
2. **No desktop notifications.** Long transfers, folder-sync errors, and
   edit-in-place save failures only surface as in-app toasts — invisible when
   Faro isn't focused. `tauri-plugin-notification` isn't in `Cargo.toml`.
3. **No PATH integration.** `cli_updater.rs::install_missing` downloads
   faro-cli into an app-owned `bin/` dir and its own comment admits it: *"Not
   added to PATH (that's OS-specific and intrusive) — the UI reports the path
   so the user can wire it up."* Every other dev tool (VS Code's `code`,
   GitHub CLI) offers a one-click "install shell command" — we should too.
   The good news on Windows: the **per-user** PATH lives in
   `HKCU\Environment` and needs **no admin rights at all** — only a
   machine-wide (all-users) PATH does.

## Scope

**In:**
- `tauri-plugin-updater` with **signed** release artifacts, a static update
  manifest (GitHub Releases `latest.json`), check-on-launch, an in-app
  download-progress + restart flow, and a Settings → About "Check for updates".
- `tauri-plugin-notification` for a small, curated set of events: transfer
  batch finished/failed, folder-sync error, edit-in-place save failure — with a
  settings toggle and a "only when window unfocused" default.
- **One-click "Add faro-cli to PATH"** (Windows/macOS/Linux) at the
  **per-user** level — no admin required — with status, add, and remove.

**Explicitly out:**
- Delta updates, channels (beta/nightly), staged rollouts — static full-artifact
  manifest only.
- A notification center / history inside the app — OS toasts only.
- Auto-updating **faro-cli** from the app (Plan 10 already owns CLI updates;
  this plan only reuses its version-drift signal to prompt the user).
- **Machine-wide (all-users) PATH changes.** They need admin/UAC elevation; the
  per-user PATH covers the real scenario (the developer's own terminal). If a
  genuine multi-user machine case appears, it gets an elevated-helper follow-up,
  not a silent registry write.

## Approach

### Phase 1 — Updater plumbing (signed)
- Add `tauri-plugin-updater` (+ `tauri-plugin-process` for relaunch). Generate
  the updater keypair; the **public key goes in `tauri.conf.json`**, the private
  key into CI secrets. **Never ship an unsigned/unchecked update path** —
  signature verification is the whole security model.
- Bundle config: enable updater artifacts; the release CI uploads the signed
  artifacts + a generated `latest.json` to GitHub Releases. Manifest URL
  (with `{{target}}`/`{{current_version}}`) in the updater config.
- macOS note: the app is ad-hoc signed today (`signingIdentity: "-"`); the
  updater still works (its own signatures are independent), but record whether
  we keep ad-hoc or move to a Developer ID at release time.

### Phase 2 — Update UX
- Check on launch (quietly, max once per N hours — reuse the `settings` table
  for `lastUpdateCheck`) and on demand from Settings → About (current version +
  "Check for updates" button + release-notes link).
- Available → dialog with version + notes → download with a progress bar in the
  status area → "Restart to update" (relaunch via the process plugin).
- Respect Plan 10: if the CLI drift check says the CLI is newer than the app,
  the update prompt is the fix path — link the two surfaces.

### Phase 3 — Desktop notifications
- Add `tauri-plugin-notification`; request permission lazily on first eligible
  event (or from Settings), never at startup splash.
- Events (all behind a `notifications` setting, default **on, unfocused-only**):
  - a transfer batch empties the active queue (done / failed counts),
  - folder-sync enters error state,
  - edit-in-place upload fails (the user may have Faro hidden while editing).
- Clicking a notification focuses the window and opens the relevant panel
  (transfer panel / sync settings).

### Phase 4 — One-click PATH install (per-user, no admin)
New `src-tauri/src/path_integration.rs` behind three commands:
`path_status` / `path_add` / `path_remove`, plus a Settings → About row:
"faro-cli on PATH ✓ / not on PATH — [Add to PATH]". Also surfaced right after
Plan 10's CLI install/update succeeds ("Added to PATH so `faro-cli` works in
any terminal").

- **Windows (the case that matters):** read/write the **per-user** `Path`
  value under `HKCU\Environment` via the `winreg` crate — **no admin, no UAC
  prompt**. Append the app-owned `bin/` dir (the one `install_missing` already
  downloads into), deduped and idempotent (re-adding is a no-op; never
  duplicate). Then broadcast `WM_SETTINGCHANGE` (`"Environment"`) so **new**
  terminals pick it up immediately — already-open terminals need a restart,
  which the UI says plainly.
  - **Never use `setx PATH …`** — it truncates the value at 1024 chars and
    flattens `REG_EXPAND_SZ` to `REG_SZ`, silently eating other entries.
    Registry read/modify/write of the exact existing value type only.
  - Preserve the value type (`REG_EXPAND_SZ` stays `REG_EXPAND_SZ`, keep
    `%VARS%` unexpanded), and back up the prior value into `faro.db` before
    the first write so `path_remove` is a faithful restore, not a guess.
- **macOS/Linux:** symlink `faro-cli` into `~/.local/bin` (user-level, no
  sudo). If that dir isn't on the user's PATH (macOS default), append one
  `export PATH=…` line to the detected shell's profile (`~/.zshrc` /
  `~/.bashrc`), guarded by a `# faro-cli` marker comment so removal is exact.
- **Removal** (`path_remove`): delete only our entry/symlink/marker line;
  refuse to touch anything else. Shown as "Remove from PATH" next to the ✓.
- Where the CLI already sits **on** PATH (sidecar or user-installed),
  `path_status` reports the location and the row is informational only.

## Integration points
- `src-tauri/Cargo.toml` — `tauri-plugin-updater`, `tauri-plugin-process`,
  `tauri-plugin-notification`, `winreg` (Windows-only, Phase 4).
- `src-tauri/tauri.conf.json` — updater pubkey + endpoints; bundle updater
  artifacts.
- CI release workflow — signing key secret, artifact + `latest.json` upload.
- `src-tauri/src/lib.rs` — plugin registration; a small `updater.rs` for
  check/download/restart commands; notification emit sites in `transfer.rs`,
  `foldersync.rs`, `editor.rs`.
- `src-tauri/src/path_integration.rs` (new) + `commands.rs` —
  `path_status`/`path_add`/`path_remove`; hooks into
  `cli_updater.rs::install_missing` (offer PATH wiring right after install).
- `src/components/Settings.tsx` — About section + notification toggles + the
  PATH row; `settings` table keys `notifications`, `lastUpdateCheck`.
- `src-tauri/capabilities/` — permissions for the three plugins.

## Risks
- **Key management is the plan.** Losing the private key = users can never
  update again; leaking it = anyone ships "Faro updates". Document custody.
- The manifest endpoint must be highly available and HTTPS; GitHub Releases
  covers both.
- Notification spam — the curated list + unfocused-only default is deliberate;
  do not notify per-file.
- Windows: updating while running needs the NSIS/msi installer path — verify
  the updater can replace the app in-place on Windows specifically.
- **PATH writes are user-environment surgery.** `setx` is banned (1024-char
  truncation, type flattening); preserve the registry value type, dedupe, and
  keep the pre-write backup in `faro.db` so remove = restore. Only ever touch
  our own entry — a bug here corrupts the user's whole environment.

## Verification
- Point the updater at a **local mock `latest.json`** (like Plan 10's CLI
  updater tests): older version → prompt appears, download progresses, restart
  applies; current version → "up to date"; tampered artifact → signature
  rejection, no install.
- Notifications: run a transfer with the window unfocused → one OS toast;
  focused → none; toggle off → none.
- PATH: on Windows, add → `HKCU\Environment\Path` gains exactly one entry
  (value type preserved), a **new** terminal resolves `faro-cli`; add again →
  no duplicate; remove → entry gone, rest of PATH byte-identical. On Linux/macOS
  → symlink lands in `~/.local/bin`, marker line appears once in the shell
  profile, removal cleans both.
