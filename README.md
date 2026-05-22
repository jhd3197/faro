# Faro

> A modern desktop client for FTP, SFTP, SSH, and S3-compatible storage.

**Faro** is a developer-first workspace that brings together the things you
usually split between FileZilla, PuTTY, terminal tabs, and S3 browser tools.
Save connections, browse remote files, transfer assets, manage SSH keys, open
terminals, and work with S3-compatible storage — all in one window.

> Faro keeps every server, bucket, and session within reach.

## Status

**v1.1.** Adds a `faro-cli` binary that scripts every backend the GUI
speaks. Reuses the same on-disk profile store, the same `RemoteFs` trait,
the same `sync::plan` logic. Subcommands: `ls / cp / mv / rm / mkdir / sync
/ profiles`. Path syntax: `profile:/path` for remote, bare paths (incl.
Windows drive letters) for local.

From v1.0: One-way directory sync (Local→Remote / Remote→Local; Additive or
Mirror). The planner walks both trees via the `RemoteFs` trait so it works
across every backend; the executor reuses the existing transfer queue for
progress. A "Sync" button rides the splitter between the two file panes
when a connection is active.

What's in the bag:

- Protocols: SFTP, FTP, FTPS, AWS S3, Cloudflare R2, Backblaze B2,
  Azure Blob.
- SSH polish: known-hosts verification with interactive fingerprint
  prompt, ssh-agent (unix `$SSH_AUTH_SOCK`, OpenSSH-for-Windows named pipe,
  Pageant pipe), multi-tab terminals.
- Transfers: drag-and-drop, multi-select, recursive directories,
  overwrite/skip/rename policies, multipart S3 above 16 MB, one-way sync
  with optional mirror deletes.
- Profile importers: OpenSSH `~/.ssh/config`, FileZilla `sitemanager.xml`,
  PuTTY (Windows registry / `~/.putty/sessions/`).
- File ops: rename, delete (recursive on dirs), mkdir, chmod (SFTP / unix
  local; SITE CHMOD on FTP; not applicable on object stores).
- UI: capability-aware (mkdir / chmod / terminal hide on backends that
  don't support them), dark / light themes, terminal customisation,
  sort / hidden-file prefs.

External polish that requires infra (code signing, auto-updater, landing
page) is intentionally outside this repo and deferred to release time.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  React + TypeScript + Tauri webview              │
│  (dual-pane browser, xterm.js terminal, profiles)│
└──────────────────┬───────────────────────────────┘
                   │ Tauri commands + events
┌──────────────────┴───────────────────────────────┐
│  Rust core                                        │
│   RemoteFs trait → LocalFs, SftpFs (S3/FTP next) │
│   Session manager  (shared SSH session pool)     │
│   Terminal        (russh PTY channel → events)   │
│   Transfer manager (concurrent file + dir trees) │
│   Profiles        (JSON in app data dir)         │
└──────────────────────────────────────────────────┘
```

The wedge: **terminal and SFTP browser share one SSH session per profile.**
One auth, two surfaces, no PuTTY/FileZilla double-login dance.

## Develop

```bash
npm install
npm run tauri dev
```

First build is slow (Rust crates). Subsequent ~30s.

## CLI

The same crate ships a `faro-cli` binary that uses your saved GUI profiles:

```bash
cd src-tauri
cargo build --bin faro-cli --release
# binary lands at src-tauri/target/release/faro-cli

faro-cli profiles list
faro-cli ls prod:/var/log
faro-cli cp ./report.pdf prod:/var/www/uploads
faro-cli sync ./site prod:/var/www/site --mirror --dry-run
```

Saved profiles are read from the same location the GUI uses, so anything
you set up in the app is immediately scriptable. The CLI prompts on stdin
for unknown host keys and never writes secrets to disk that the GUI hadn't
already saved.

### Prerequisites

- Node 20+
- Rust 1.88+ (Tauri 2.11's transitive deps need this). Check with `rustc --version`.

### Windows PATH gotcha

If you previously installed Rust via Chocolatey (`choco install rust`), there's
a `rustc.exe` at `C:\ProgramData\chocolatey\bin` that *shadows* the
rustup-managed toolchain in `~/.cargo/bin`. `rustup update stable` updates the
rustup copy but doesn't touch the chocolatey one — so `rustc --version` keeps
reporting the old version and Tauri builds fail with messages like
`rustc 1.85.0 is not supported by darling@0.23.0 (requires 1.88)`.

Fix it once, project-wide:

```powershell
# Option A — uninstall the chocolatey copy (recommended)
choco uninstall rust

# Option B — keep both, but make rustup win for this shell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
```

Or in bash:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## Icons

The scaffold ships with placeholder icons. To replace, drop a 512×512 PNG at
`src-tauri/icons/source.png` and run:

```bash
npm run tauri icon src-tauri/icons/source.png
```

## Layout

```
src/                       React frontend
  components/              UI components (panes, modals, settings, etc.)
  stores/                  Zustand stores (connections, transfers, layout, settings)
  lib/ipc.ts               Typed wrappers around Tauri commands
  lib/types.ts             Shared types (mirror Rust serde structs)

src-tauri/src/
  commands.rs              Tauri command surface (IPC entry points)
  remotefs/
    mod.rs                 RemoteFs trait + DirEntry / Capabilities
    sftp.rs                SFTP impl (russh + russh-sftp)
    local.rs               Local filesystem impl
  session/
    mod.rs                 SSH session pool (one per profile)
  terminal.rs              PTY over russh, emits events to frontend
  transfer.rs              Transfer engine + recursive directory walker
  profiles/
    mod.rs                 Profile CRUD, persisted to JSON
```

## Roadmap (concrete)

- **v0.2** — SFTP, integrated terminal, drag-and-drop, recursive transfers, file ops, settings
- **v0.3** — known-hosts verification, ssh-agent (unix + OpenSSH-for-Windows pipe), multi-tab terminals
- **v0.4** — FTP / FTPS backends, Pageant pipe support
- **v0.5** — S3 / R2 / B2 (one backend, three endpoint configs)
- **v0.6** — Azure Blob, ObjectSession refactor
- **v0.7** — Profile importers (OpenSSH config, FileZilla, PuTTY)
- **v1.0** — One-way sync mode (this). Code signing / auto-updater / landing
  page require external infra and are tracked outside this repo.
