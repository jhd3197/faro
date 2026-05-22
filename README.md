# Faro

> A modern desktop client for FTP, SFTP, SSH, and S3-compatible storage.

**Faro** is a developer-first workspace that brings together the things you
usually split between FileZilla, PuTTY, terminal tabs, and S3 browser tools.
Save connections, browse remote files, transfer assets, manage SSH keys, open
terminals, and work with S3-compatible storage — all in one window.

> Faro keeps every server, bucket, and session within reach.

## Status

**v0.7 dev.** New in v0.7: connection importers for OpenSSH config
(`~/.ssh/config`), FileZilla (`sitemanager.xml`), and PuTTY (Windows
registry / `~/.putty/sessions/`). One tabbed import dialog auto-detects
default paths, lists every connection it found, lets the user pick which
to bring in, and saves them to the profile store. Identity files surface as
private-key auth pointing at the original path.

From v0.6: Azure Blob via `ObjectFs` (shared with S3/R2/B2 — different
builder, same `Arc<dyn ObjectStore>`).

From v0.5: S3 / R2 / B2 via object_store 0.11. Provider preset buttons,
capability-aware UI, multipart upload above 16 MB.

From v0.4: FTP and FTPS backends, Pageant pipe support, per-protocol port
defaults.

From v0.3: known-hosts verification with interactive fingerprint prompt,
ssh-agent auth (unix `$SSH_AUTH_SOCK`, OpenSSH-for-Windows named pipe,
Pageant pipe), multi-tab terminals.

From v0.2: connection profiles, SFTP browser, integrated SSH terminal,
drag-and-drop transfers, recursive directory transfers, multi-select,
right-click file ops, settings.

Roadmap: v1.0 polish — sync mode, code signing, auto-updater, landing.

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
- **v0.7** — Profile importers (OpenSSH config, FileZilla, PuTTY) (this)
- **v1.0** — Polish, sync mode, code signing, auto-updater, landing page
