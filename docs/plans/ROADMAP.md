# Faro Roadmap

How the individual plans sequence, and where Faro sits in the wider
DeviceKit / ServerKit ecosystem. Each plan doc has the detail; this is the map
and the order.

## The ecosystem in one picture

Faro is the **client/hub**: a desktop app for reaching files and machines
(SFTP, FTP, S3, and Faro's own Noise-paired **agent**). Two sibling "kits"
manage fleets and hand off to Faro:

- **DeviceKit** — Android device fleet (Flask + React + Kotlin agent + extensions).
  **Connects to Faro at the agent-protocol level**: the Android agent embeds
  Faro's `faro-agentd`, so a phone appears in Faro as a first-class connection
  (browse/transfer/sync). *Done — shipped.*
- **ServerKit** — server control panel, the server-fleet analog (Flask + React +
  **Go** host agent + extensions). **Connects to Faro only via a `faro://`
  deep-link** today (`serverkit-faro` prefills Faro's New Connection with a
  site's SFTP details). It does **not** use Faro's agent protocol. *Shallow link
  only — see Track D.*
- Shared libraries: **Prompture** (multi-LLM) and **Tukuy** underpin the AI
  features across CachiBot / DeviceKit / ServerKit.

So Faro has two integration depths: a **deep** one (embed `faro-agentd` → the
machine is controllable/browsable in Faro; DeviceKit does this) and a **shallow**
one (`faro://` deep-link prefill; ServerKit does this).

---

## Track A — Folder sync (Plan 2)

Continuous "attach a local folder, keep it mirrored" for any backend.

- ✅ **Phase 1–2** — continuous one-way sync (both directions), watcher + poll
  reconciler reusing `sync::plan` + `execute_sync_plan`, Additive/Mirror,
  Settings panel + status pill. *Merged.*
- ⬜ **Safety hardening** *(next — small, high value)*: exclude patterns +
  Mirror-delete guard. Makes the shipped feature safe to rely on.
- ⬜ **Runtime verification**: drive a live sync (easiest: to a paired phone
  agent). Currently compile/type-verified only.
- ⬜ **Persistent state index + `change_signal` capability**: same-size-edit
  detection, incremental scans, and the foundation for bidirectional.
- ⬜ **Phase 3 → Plan 4** (on-demand placeholders) — see Track C.
- ⬜ **Bidirectional + conflict resolution** — needs the index; own effort.

## Track B — More connection backends (Plan 3)

Widen the connection list; each is one `RemoteFs` impl and inherits browse /
transfer / sync for free.

- ✅ Already present (pre-existing): SFTP, FTP/FTPS, S3, **Azure Blob** (via
  `object_store`), Faro Agent, local.
- ⬜ **Phase 0** — S3-compatible presets (R2/B2/Wasabi/MinIO/Spaces): UI presets,
  ~no backend code. *Cheapest win.*
- ⬜ **Phase 1** — WebDAV (Nextcloud/ownCloud/generic). Low effort, high coverage.
- ⬜ **Phase 2** — SMB/CIFS (NAS/Windows shares). Biggest single gap.
- ⬜ **Phase 3** — native GCS (Azure already works).
- ⬜ **Phase 4** — OAuth clouds (Drive/OneDrive/Dropbox). Hardest (OAuth + ID-vs-path).

## Track C — On-demand virtual folders (Plan 4)

OneDrive-style placeholders: files show in the folder, download on open, free-up-
space to evict. Native, per-OS, **Windows-first** (Cloud Filter API).

- ⬜ Windows provider (Cloud Filter API via the `windows` crate) — its own
  feature-flagged effort; reuses the Track A engine for listing + hydration.
- ⬜ macOS File Provider extension, ⬜ Linux FUSE, or the WinFsp/FUSE virtual-mount
  fallback. *Designed in Plan 4; not built.*

## Track D — ServerKit ↔ Faro convergence (cross-project, greenfield)

Today ServerKit reaches Faro only through the `faro://` deep-link. The natural
next step mirrors DeviceKit: **let a ServerKit-managed host expose a Faro agent**
so servers become first-class, browsable/syncable Faro connections (not just a
prefilled SFTP handoff).

- ✅ Shallow: `serverkit-faro` `faro://connect` deep-link (prefill only, no
  credentials). *Shipped.*
- ⬜ **Deep: embed/expose `faro-agentd` on ServerKit hosts.** Two options:
  (a) ServerKit's Go agent speaks the Faro Noise protocol natively (port a
  minimal `faro-agent-proto` responder to Go), or (b) ServerKit installs the
  existing `faro-agentd` binary as a managed service (it already ships
  cross-platform binaries + a one-line installer). Option (b) is far cheaper and
  reuses everything.
- ⬜ With that in place, **Track A folder sync targets a ServerKit host** for
  free — the payoff of the shared `RemoteFs`/agent design compounds across the
  ecosystem.

Note: ServerKit's own roadmap has "remote app/site deployment through connected
agents" as its open item — Track D dovetails with it (Faro becomes the file/sync
surface for those agents).

---

## Track E — Brand & protocol logos (Plan 5)

Additive icon layer for recognizable brand marks (S3/Azure/SSH/WordPress) via
Iconify, bundled offline. Deliberately does **not** touch the file-type icons
(Material Icon Theme — more complete + has the extension mapping) or the lucide
UI icons.

- ✅ Files already use Material Icon Theme; UI uses lucide. *(shipped/present)*
- ⬜ **Phase 1–2** — Iconify offline foundation + protocol logos on the rail,
  connection list, and New-Connection picker (logo *plus* the colour monogram).
- ⬜ Phase 3 — tech badges elsewhere. ⬜ Phase 4 — (deferred) evaluate
  consolidating lucide UI icons onto Iconify.

## Track F — Disk Usage Explorer (Plan 6)

A WinDirStat/WizTree-style treemap + size-ranked tree, but over **any** backend
(SFTP/S3/FTP/Agent/local) — opens as a workspace tab like the SSH terminal. The
differentiator vs the desktop tools: it works on **remote servers and buckets**,
with a shell `du` fast path (SSH/Agent) and object-store flat listing so it's
actually fast at scale.

- ⬜ **Phase 1–2** — cross-backend scan engine (RemoteFs walk, progress/cancel)
  + the Canvas treemap tab and size list. ⬜ Phase 3 — `du`/`find` + flat-listing
  fast paths. ⬜ Phase 4 — delete/reveal actions + polish.

## Track G — Directory Diff (Plan 7)
Meld/Beyond Compare for any two backends (incl. remote↔remote), surfaced in the
**GUI, `faro-cli`, and as an MCP `faro_diff` tool**. Reuses `sync.rs`'s diff. ⬜

## Track H — Fleet Search (Plan 8)
Name + content search across any connection; exec `rg`/`grep` fast path on
SSH/agent, walk fallback. GUI + CLI + `faro_search` MCP tool. ⬜

## Track I — Fleet Skills (Plan 9)
Reusable, parameterized, **AI-authorable** automations over the fleet, MCP-native
(the AI composes/saves Skills, then runs them across servers). Builds on the
bridge's existing saved-commands + `faro_exec` + approvals. Safety-gated. ⬜

## Track J — Scoped connection sharing (Plan 10)
Share a box read-only / path-jailed / time-boxed **without a login system**, by
extending the agent's pairing + policy + revocation with scope + expiry. A
browser-served Faro (code-server-style) is the deferred, login-requiring 10b. ⬜

## Near-term quick wins (small, high-value)
- **Editable permissions dialog** — today Properties *shows* mode read-only; add
  a FileZilla-style chmod editor (rwx checkboxes + octal). Backend (`chmod_path`,
  `can_chmod`) already exists. Also the visual foundation for idea #10
  (permissions/security view).
- **CLI conflict policy** — expose `--overwrite` / `--on-conflict
  overwrite|skip|rename` on `faro-cli` single-file `upload`/`download`/`cp`
  (only `upload-dir` has it today; default is rename). Backend `OverwritePolicy`
  already supports all three.

## Recommended global sequence

1. **Track A safety hardening + runtime test** — finish making the shipped sync
   trustworthy. (Small.)
2. **Track B Phase 0/1/2** — S3 presets + WebDAV + SMB. Biggest coverage gain,
   no OAuth, all fit the trait.
3. **Track A state index** — robustness + unlocks bidirectional.
4. **Track D option (b)** — ServerKit installs `faro-agentd` as a managed
   service; cheap, high leverage, and every server becomes syncable via Track A.
5. **Track C Windows on-demand** and **Track B Phase 4 OAuth clouds** — the two
   large, later efforts.

To execute any of these tracks end-to-end in a fresh session, use the local
`docs/plans/prompt.md` runbook — set its one plan-filename knob and paste it.
