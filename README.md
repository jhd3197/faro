<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="128" alt="Faro" />

# Faro

**A modern desktop client for SFTP, FTP, SSH, and S3-compatible storage.**

[![Version](https://img.shields.io/badge/version-1.3.0-8b7ff6?style=flat-square)](https://github.com/jhd3197/faro/releases)
[![License](https://img.shields.io/badge/license-MIT-8b7ff6?style=flat-square)](LICENSE)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-8b7ff6?style=flat-square)](https://tauri.app)

*Servers · storage · sessions, all in one workspace.*

</div>

---

## What it is

Faro is what you'd get if you let FileZilla, PuTTY, and a half-dozen cloud-storage browsers share one window and one connection list. Save a server once, open its files in the dual-pane browser and a terminal tab against the same SSH session, drag-and-drop transfers between sides, sync directories one-way, edit remote files in your local editor with auto-upload on save — and run all of the same operations from a CLI when you don't feel like clicking.

## Backends

| Protocol | Browse | Transfer | Sync | Shell |
|---|:-:|:-:|:-:|:-:|
| **SFTP** (SSH) | ✓ | ✓ | ✓ | ✓ |
| **FTP** | ✓ | ✓ | ✓ | — |
| **FTPS** (explicit) | ✓ | ✓ | ✓ | — |
| **Amazon S3** | ✓ | ✓ | ✓ | — |
| **Cloudflare R2** | ✓ | ✓ | ✓ | — |
| **Backblaze B2** | ✓ | ✓ | ✓ | — |
| **Azure Blob** | ✓ | ✓ | ✓ | — |

## Highlights

- **One SSH session, two surfaces** — the SFTP browser and the terminal pane share a single connection per profile. No PuTTY/FileZilla double-login dance.
- **Known-hosts verification** with an interactive fingerprint prompt. Mismatched keys get a danger-toned UI so MITM attempts are obvious.
- **ssh-agent everywhere** — `$SSH_AUTH_SOCK` on unix, OpenSSH-for-Windows pipe, and Pageant (PuTTY 0.78+) on Windows. `ssh-add` once, connect everywhere.
- **Multi-tab terminals** sharing a single SSH session per profile. Tabs survive switches without re-establishing the channel.
- **Drag-and-drop transfers** between panes, recursive directory transfers, multi-select, overwrite/skip/rename policies, multipart upload for objects > 16 MB.
- **One-way directory sync** — Local↔Remote, Additive (copy only) or Mirror (also delete extras). The planner walks any backend through the same trait, then hands the work to the existing transfer queue for progress.
- **Profile importers** — bring connections in from `~/.ssh/config`, FileZilla's `sitemanager.xml`, and PuTTY's Windows registry / `~/.putty/sessions/`. The import dialog auto-detects each source's default location.
- **Capability-aware UI** — chmod and mkdir hide on backends that don't support them; terminal is SFTP-only; protocol chips show what you're connected to at a glance.
- **`faro-cli`** scripts every backend the GUI speaks, using the same saved profiles. `faro-cli sync ./site prod:/var/www --mirror --dry-run` does what you'd hope.
- **Edit in place** — right-click a remote file → opens in your default editor on a tempfile, watches it with `notify`, debounces, uploads back on every save. A pill in the status bar shows live edit sessions and lets you stop them.

## Develop

```bash
npm install
npm run tauri dev
```

First build is slow — it's compiling the Rust crate tree. Subsequent builds are ~30 s.

**Prerequisites**: Node 20+, Rust 1.88+ (`rustc --version` — Tauri 2's transitive deps require it).

## CLI

The same Cargo package ships a second binary, `faro-cli`, that reuses your saved GUI profiles.

```bash
cd src-tauri
cargo build --bin faro-cli --release
# → src-tauri/target/release/faro-cli

faro-cli profiles list
faro-cli ls prod:/var/log
faro-cli cp ./report.pdf prod:/var/www/uploads
faro-cli sync ./site prod:/var/www/site --mirror --dry-run
faro-cli rm prod:/tmp/build --recursive
```

Path syntax: bare paths are local (including Windows `C:\…`), `name:/path` references a saved profile. The CLI prompts on stdin for unknown host keys and never writes secrets to disk that the GUI hadn't already saved.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  React + TypeScript + Tauri webview                         │
│  Dual-pane browser · xterm.js terminal · sync dialog        │
└──────────────────────────┬──────────────────────────────────┘
                           │  Tauri commands + events
┌──────────────────────────┴──────────────────────────────────┐
│  Rust core (faro_lib)                                       │
│   RemoteFs trait → LocalFs · SftpFs · FtpFs · ObjectFs      │
│   Session enum  → Ssh(SshSession) · Ftp · Object            │
│   SessionManager pools one SSH session per profile          │
│   TransferManager → concurrent file + directory transfers   │
│   sync::plan walks both sides via the RemoteFs trait        │
│   importers/ → OpenSSH · FileZilla · PuTTY                  │
│   known_hosts + HostKeyVerifier (Tauri or stdin)            │
└──────────────────────────┬──────────────────────────────────┘
                           │  same Rust core
┌──────────────────────────┴──────────────────────────────────┐
│  faro-cli  (clap)                                           │
│   ls · cp · mv · rm · mkdir · sync · profiles               │
└─────────────────────────────────────────────────────────────┘
```

The wedge: **everything goes through one `RemoteFs` trait.** Adding a new protocol means writing one trait impl and one builder; the dual-pane browser, the sync planner, the CLI, and the transfer engine pick it up automatically.

## Layout

```
src/                       React frontend
  components/              ConnectionManager, FilePane, Terminal,
                           SyncDialog, ImportDialog, HostKeyModal, …
  stores/                  Zustand stores
  lib/ipc.ts               Typed wrappers around Tauri commands
  lib/types.ts             Shared types (mirror Rust serde structs)

src-tauri/src/
  commands.rs              Tauri command surface
  remotefs/                RemoteFs trait + Local / Sftp / Ftp / Object
  session/                 SshSession, FtpSession, ObjectSession,
                           HostKeyVerifier trait, SessionManager
  terminal.rs              PTY over russh, emits events
  transfer.rs              Per-backend streaming transfers + progress
  sync.rs                  Two-tree diff planner, RemoteFs-driven
  importers/               OpenSSH config, FileZilla XML, PuTTY registry
  known_hosts.rs           ~/.ssh/known_hosts read/write
  bin/faro_cli.rs          CLI binary (clap + indicatif)
```

## Windows PATH gotcha

If you installed Rust via Chocolatey (`choco install rust`), there's a `rustc.exe` at `C:\ProgramData\chocolatey\bin` that **shadows** the rustup-managed toolchain. `rustup update stable` updates the rustup copy but doesn't touch the chocolatey one, so `rustc --version` keeps reporting the old version and Tauri builds fail with messages like `rustc 1.85.0 is not supported by darling@0.23.0`.

```powershell
# Option A — uninstall the chocolatey copy (recommended)
choco uninstall rust

# Option B — keep both, but make rustup win for this shell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
```

## Icons

```bash
# Replace src-tauri/icons/source.png with a 1024×1024 PNG, then:
npm run tauri icon src-tauri/icons/source.png
```

`scripts/process-icon.py` handles cropping a black-bordered source PNG to its rounded-square art and writing `source.png` at the right size.

## Roadmap

- **v1.0** — one-way sync mode
- **v1.1** — `faro-cli` binary
- **v1.2** — edit-in-place external editor
- **v1.3** — custom title bar with File/Edit/View/Help menus + integrated window controls; GitHub Actions release pipeline + CI *(this)*
- **next** — transfer speed limits, queue editing (priority/retry/pause), filename filters (`.gitignore`-style), WebDAV backend, search/filter
- **release polish** — code signing (Apple Developer / Windows EV cert), Tauri auto-updater, landing page

## License

MIT — see [LICENSE](LICENSE).
