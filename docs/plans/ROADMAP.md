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

To execute Track A end-to-end in a fresh session, use the local `prompt.md`
runbook at the repo root.
