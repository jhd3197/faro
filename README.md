<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="128" alt="Faro" />

# Faro

**A modern desktop client for SFTP, FTP, SSH, and S3-compatible storage.**

[![Version](https://img.shields.io/badge/version-1.3.1-8b7ff6?style=flat-square)](https://github.com/jhd3197/faro/releases)
[![License](https://img.shields.io/badge/license-MIT-8b7ff6?style=flat-square)](LICENSE)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-8b7ff6?style=flat-square)](https://tauri.app)

*Servers · storage · sessions, all in one workspace.*

**🤖 New — [Agent Bridge](#-agent-bridge): hand a live server to Claude Code (or any MCP agent) — through your authenticated session, with per-command approval, and zero credentials shared.**

</div>

---

## What it is

Faro is what you'd get if you let FileZilla, PuTTY, and a half-dozen cloud-storage browsers share one window and one connection list. Save a server once, open its files in the dual-pane browser and a terminal tab against the same SSH session, drag-and-drop transfers between sides, sync directories one-way, edit remote files in your local editor with auto-upload on save — and run all of the same operations from a CLI when you don't feel like clicking.

And when you want an AI agent to actually *do* something on a box, the **[Agent Bridge](#-agent-bridge)** lets it run commands through the session you already opened — no remote install, no keys handed over, every command gated behind your approval.

## Download

Grab the latest installer from the [**Releases**](https://github.com/jhd3197/faro/releases/latest) page — every push to `main` publishes fresh builds for all three desktop platforms plus the standalone `faro-cli`.

| Platform | Installer | First-launch note |
|---|---|---|
| **macOS** (Intel + Apple Silicon) | `.dmg` (universal) | One-time `xattr` step ↓ |
| **Windows** (x64) | `.exe` (NSIS) or `.msi` | SmartScreen → *More info → Run anyway* |
| **Linux** (x64) | `.AppImage`, `.deb`, `.rpm` | `chmod +x` the AppImage |

Builds are **unsigned** (no Apple Developer / Windows EV certificate yet), so each OS guards the first launch:

- **macOS** — after dragging **Faro.app** to **/Applications**, run this once in Terminal, then open the app normally:
  ```bash
  xattr -cr /Applications/Faro.app
  ```
  It's needed because the build isn't notarized by Apple; without it macOS reports the app as "damaged."
- **Windows** — on the *"Windows protected your PC"* prompt, click **More info → Run anyway**.

> Prefer to build from source? See [Develop](#develop) below.

## 🤖 Agent Bridge

**Let a local AI agent run commands on your servers — safely.**

This is the part that makes Faro more than a file client. Connect to a server once, and Faro can lend that **already-authenticated SSH session** to a local AI agent — Claude Code, Cursor, anything that speaks [MCP](https://modelcontextprotocol.io) — so the agent operates on the box **without installing anything remote and without ever seeing your credentials.** Faro stays the gatekeeper.

> **Why it's different:** most "AI over SSH" setups make you hand the agent your keys or stand up a server-side daemon. Faro does neither. The agent borrows the session *you* already opened, *you* approve every command, and nothing reaches the server except the commands you OK.

**Wire it into Claude Code — native MCP, auto-discovered tools:**

1. Connect to a server, open the **Bridge** panel (status-bar pill), hit **Start**, and flip on **Allow agent access**.
2. Copy the one-liner the panel generates and run it in your project:
   ```bash
   claude mcp add --transport http faro http://127.0.0.1:<port>/mcp \
     --header "Authorization: Bearer <token>"
   ```
3. Claude Code now has two tools — `faro_list_sessions` and `faro_exec`. Ask it *"check disk usage on the server"* and it runs through Faro. (Prefer curl or another agent? The panel also exports a ready-to-paste `SKILL.md` for the plain HTTP API.)

**The guardrails — all on by default:**

- 🔒 **Localhost only** — bound to `127.0.0.1` on a random port.
- 🔑 **Bearer token** — per-launch, required on every request.
- ☑️ **Per-session opt-in** — no connection is reachable until you turn it on.
- 🙋 **Approve every command** — each `exec` pops a prompt in Faro and blocks until you click Approve (or it times out).
- 📋 **Live audit log** — every command, approval, and denial, right in the panel.

Surface: `GET /health`, `GET /sessions`, `POST /exec`, and `POST /mcp` (MCP Streamable HTTP). It's a hand-rolled localhost server on the existing tokio runtime — **zero new dependencies.**

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

- **🤖 Agent Bridge** — give Claude Code (or any MCP agent) command access to a connected server through your authenticated session, gated by per-command approval and no shared credentials. [Details ↑](#-agent-bridge)
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

A standalone binary, `faro-cli` — its own workspace crate under `src-tauri/faro-cli/` — reuses your saved GUI profiles. Prebuilt binaries ship with every [release](https://github.com/jhd3197/faro/releases/latest), or build it yourself:

```bash
cd src-tauri
cargo build -p faro-cli --release
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
  bridge.rs                Agent Bridge — localhost MCP/HTTP server,
                           per-command approval, audit log
  remotefs/                RemoteFs trait + Local / Sftp / Ftp / Object
  session/                 SshSession, FtpSession, ObjectSession,
                           HostKeyVerifier trait, SessionManager
  terminal.rs              PTY over russh, emits events
  transfer.rs              Per-backend streaming transfers + progress
  sync.rs                  Two-tree diff planner, RemoteFs-driven
  importers/               OpenSSH config, FileZilla XML, PuTTY registry
  known_hosts.rs           ~/.ssh/known_hosts read/write

src-tauri/faro-cli/        Standalone CLI crate — path-depends on faro_lib
  src/main.rs              clap + indicatif: ls·cp·mv·rm·mkdir·sync·profiles
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
- **v1.3** — custom title bar with File/Edit/View/Help menus + integrated window controls; GitHub Actions release pipeline + CI
- **unreleased** — UI density pass (named themes, command palette, sortable detail columns, breadcrumbs, in-pane filter, toasts) and the **🤖 Agent Bridge**: AI-agent command access over native MCP *(this)*
- **next** — transfer speed limits, queue editing (priority/retry/pause), filename filters (`.gitignore`-style), WebDAV backend, search/filter
- **release polish** — code signing (Apple Developer / Windows EV cert), Tauri auto-updater, landing page

## License

MIT — see [LICENSE](LICENSE).
