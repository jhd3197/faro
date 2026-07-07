# Plan 1 — Faro Agent: make pairing actually work, then make it effortless

**Status:** Phases 0–2 + deep links + service/one-liner **implemented** (2026-07-07);
sidecar bundling (Phase 3) deferred by design · **Date:** 2026-07-06 · **Prompted by:**
first real-world pairing attempt (Windows → Windows, both machines owned by the
same person) failed end-to-end.

## Implementation status (2026-07-07)

| Phase | What | Status | Commits |
|-------|------|--------|---------|
| 0a | Pairing false-failure fix; honest handshake/AddrInUse errors; e2e drives the real sequence | ✅ done | daemon serves the paired channel; controller SystemInfo best-effort (compat with ≤1.3 daemons) |
| 0b | Pair by host/port; no orphan profiles; auto-scan; visible selection; "paired" badges | ✅ done | `pair_agent(host,port,code)`; discovery annotated by key fingerprint |
| 1 | One daemon mode — serve + pair on one port (first-frame sniff); `pairable` mDNS flag; `pair --json` | ✅ done | `serve_pairing` gone; window is lazy-expiring; 3 new e2e tests |
| 2 | **Embedded agent** — Settings → Remote control hosts the agent in-app (no download) | ✅ done | `agent_host.rs`; 6 commands; pairing-code dialog; policy + revoke |
| — | `faro://` deep links for one-click "Connect with Faro" (ServerKit) | ✅ done | `deeplink.rs` (unit-tested); `docs/deep-links.md` spec |
| 3 | `faro-agentd install` service + `install-agentd.sh` one-liner | ✅ done | systemd/launchd/schtasks; script uploaded to every release |
| 3 | Bundle agentd/cli in the installer as sidecars + in-app opt-in | ⏸️ deferred | embedded agent + one-liner already cover the common paths; staged separately to protect the push-to-main build |
| 4 | Internet reach (rendezvous/relay), QR pairing, auto-update | ⏳ future | unchanged |

The rest of this document is the original proposal, kept for context.

---


## TL;DR

The Faro Agent feature is architecturally sound (Noise-encrypted, key-pinned,
policy-gated — see `docs/remote-agent.md`) but the *experience* of pairing two
machines is broken in four compounding ways, and the distribution model asks far
too much of the user:

1. **Pairing always reports "failed" — even when it succeeded.** The daemon
   closes the pairing socket immediately after acknowledging, but the app then
   asks for `SystemInfo` on that same socket. The app can never see a success.
   This is the headline bug: *the UI pairing flow has never completed
   successfully.*
2. **A failed pair leaves a half-created connection behind.** The profile is
   persisted *before* pairing runs, so the sidebar gains a connection that dies
   with `"this Faro Agent connection isn't paired yet — pair with a code first"`.
3. **Clicking a discovered machine appears to do nothing.** It silently fills
   the Host/IP text field — no selection state, no next step, and Save stays
   disabled because pairing hasn't happened.
4. **The daemon has two mutually exclusive modes on one port.** Discovery finds
   machines in `run` mode, but `run` mode *refuses pairing*. Pairing requires
   killing the daemon and restarting it as `faro-agentd pair` — and the scan UI
   can't tell the modes apart.

On top of that, reaching this broken flow requires downloading a **separate
daemon binary** from GitHub Releases, renaming it, clicking through SmartScreen,
and driving it from a terminal on the target machine. For the most common case —
*both machines already have Faro installed* — none of that should be necessary.

**Direction:** fix the bugs (Phase 0), collapse the daemon to a single
always-pairable mode (Phase 1), embed the agent inside the Faro app itself so
app↔app pairing needs zero extra downloads (Phase 2), and bundle the CLI/daemon
binaries with the installer with in-app opt-in (Phase 3).

---

## What the first real user hit (repro)

Two Windows 11 machines, Faro installed on both, `faro-agentd` downloaded onto
the target.

1. Ran `faro-agentd` on the target (defaults to `run` mode, advertises mDNS).
2. In Faro: **New Connection → Faro Agent → Scan local network** — the target
   appeared. Clicked it. *Nothing visibly happened* (Bug 3: it only filled the
   Host field). Save was disabled. Dead end.
3. Eventually got a code (`faro-agentd pair` on the target), typed it, clicked
   **Pair**. The connection *was added to the sidebar*, but a "Pairing failed"
   toast appeared (Bug 1 — the daemon had actually pinned the controller and
   printed `✓ paired`; the app just couldn't see it).
4. Clicked the new connection → error: **"pair with a code first"** (Bug 2's
   orphan profile meeting Bug 1's lost key).

Every step behaved exactly as the code is written. This is not a flaky network —
the happy path does not exist.

---

## The full journey today (why it feels heavy)

| # | Step | Where | Friction |
|---|------|-------|----------|
| 1 | Find the GitHub release page | browser | separate from the app they already installed |
| 2 | Download `faro-agentd-windows-x86_64.exe` | target machine | 4 similarly-named assets to choose from |
| 3 | Rename to `faro-agentd.exe` | target | manual, undocumented in-app |
| 4 | SmartScreen "More info → Run anyway" | target | scary, once per machine |
| 5 | Open a terminal, run `faro-agentd pair` | target | terminal required; code only exists in `pair` mode |
| 6 | Read a 6-digit code off one screen, type it on another | both | fine — this is the consent ceremony, keep it |
| 7 | Scan / click discovered machine | Faro | **Bug 3:** silent field-fill |
| 8 | Click Pair | Faro | **Bug 1:** always reports failure · **Bug 2:** orphan profile |
| 9 | Ctrl-C the pairing window, run `faro-agentd run` | target | **Bug 4:** two modes, same port — connect fails while `pair` mode is still up |
| 10 | Connect | Faro | works only if 1–9 all went right |

Ten steps, two downloads, a rename, a terminal on the target, and three bugs.
The RustDesk experience this feature explicitly cites (`docs/remote-agent.md`)
is: install one exe on both machines, read a code, type it, done.

---

## Root causes

### Bug 1 — pairing always reports failure *(the big one)*

The controller's pairing routine, after receiving the daemon's `Ok` ack, sends
one more request **on the pairing channel** to fetch machine facts for the
confirmation toast:

- `src-tauri/src/session/agent.rs:81-93` — `agent_pair()` sends
  `Request::SystemInfo` and `?`-propagates any transport error.

But the daemon's pairing handler acks and *returns*, dropping the socket:

- `src-tauri/faro-agentd/src/server.rs:139-165` — `pair_connection()` sends
  `Response::Ok`, logs, and returns. There is no request loop in pairing mode.

So the controller's `recv` hits EOF/reset **every time**. `agent_pair` errors →
`pair_agent` (`src-tauri/src/commands.rs:109-130`) never persists
`agent_key` → the UI toasts "Pairing failed" — while the daemon has already
pinned the controller and printed `✓ paired '<name>'`. The two sides
permanently disagree about what happened.

**Why tests missed it:** the e2e test
(`src-tauri/faro-agentd/tests/end_to_end.rs:59-91`) hand-rolls the client and
stops at the `Ok` ack — it never issues the `SystemInfo` request the real
controller sends on the pairing channel. The test exercises a client that
doesn't exist.

### Bug 2 — the profile is persisted before pairing succeeds

`src/components/ProfileEditor.tsx:176-185`: `pair()` calls
`saveProfile(buildProfile())` *first*, because `pair_agent` looks the profile up
by id to find host/port (`commands.rs:114-119`). If pairing then fails (which,
per Bug 1, is always), the unpaired profile is already in the connections list.
Clicking it later hits `src-tauri/src/session/agent.rs:113-117`:
`"this Faro Agent connection isn't paired yet — pair with a code first"`.

The persist-first dance is unnecessary: the controller's identity is global
(`session/agent.rs:24-34`), not per-profile — pairing only needs host, port,
and code. Nothing about pairing requires a saved profile.

### Bug 3 — clicking a discovered machine gives no feedback and leads nowhere

`src/components/ProfileEditor.tsx:534-556`: the click handler is
`setHost(d.host); setPort(d.port); setPortTouched(true)` — it updates a text
field the user may not be looking at. No selected/highlight state, no scroll or
focus to the code input, no attempt to reach the machine. And Save is gated on
`!!host && !!agentKey` (`ProfileEditor.tsx:191-203`), so until the pairing
ceremony completes the whole dialog is a dead end with a disabled button.

### Bug 4 — two mutually exclusive daemon modes, one port

- `serve` (run mode) accepts only pinned-peer `Noise_XX` handshakes
  (`server.rs:42-62`) — a pairing attempt against it fails.
- `serve_pairing` (pair mode) accepts only `Noise_XXpsk3` pairing handshakes
  (`server.rs:114-135`) — a normal connect against it fails.
- Both default to port 8722 (`faro-agentd/src/main.rs:22`), so you can't run
  both; `faro-agentd pair` while `run` is active dies with a bind error.
- Both modes advertise **identical** mDNS TXT records
  (`faro-agentd/src/discovery.rs:27-32`) — the scan UI cannot tell a pairable
  machine from an unpairable one.

Net effect: the machines discovery *finds* are exactly the machines you *can't
pair with*, and after pairing you must restart the daemon before you can
connect. The user has to understand run-vs-pair mode semantics that the UI
never explains.

### Design flaw — distribution: three artifacts, a rename, and a terminal

The installer ships only the GUI (`src-tauri/tauri.conf.json` has no
`externalBin`/`resources`); `faro-cli` and `faro-agentd` are separate release
assets with a manual rename step (`.github/workflows/release.yml:394-412`).
The most common pairing scenario — the user's own second computer, which
already has Faro installed — still requires downloading and babysitting a
terminal daemon.

---

## North star

> **If both machines have Faro installed, pairing is: open Faro on both, click
> "Show pairing code" on one, click the discovered machine and type the code on
> the other. No downloads, no terminal, no renames, under 30 seconds.**

The headless daemon remains — it's the right tool for servers, VMs, and boxes
where you don't want the GUI — but it becomes an *optional* deployment shape
instead of a prerequisite. This is exactly the RustDesk model the feature was
pitched on: one exe is both controller and controllable.

Principles:

- **Pairing is still an explicit consent ceremony.** Nothing below weakens the
  security model (code-as-PSK, key pinning, target-side policy, audit log). We
  are removing *operational* friction, not the ceremony.
- **Every click gives feedback.** Selecting a machine looks selected; pairing
  shows progress; failure states say what to do next.
- **Never persist half-states.** A connection exists only after pairing
  succeeds.

---

## The plan

### Phase 0 — stop the bleeding (bug fixes, no protocol change, ship first)

**0.1 Daemon: service requests inside the pairing session.**
After pinning + `Ok` in `pair_connection` (`server.rs:139-165`), don't drop the
socket — enter the same request loop as `handle_paired` (the peer is now
pinned; policy applies as normal). The controller's `SystemInfo` gets a real
answer, and the freshly paired channel could even be reused as a live session
later. Small, contained change.

**0.2 Controller: make the post-ack `SystemInfo` best-effort.**
In `agent_pair` (`session/agent.rs:81-93`), wrap the info fetch so a transport
error falls back to the stub `SystemInfo` instead of failing the whole pairing
(the pin *has* happened by then — reporting failure after the ack is a lie).
This alone fixes Bug 1 even against **old daemons already in the field**, which
is why we do both 0.1 and 0.2.

**0.3 Pair by host/port; persist only on success.**
Change `pair_agent` (`commands.rs:109-130`) to take `host`, `port`, `code`
directly and return the `server_key` in `AgentPairResult`. The editor
(`ProfileEditor.tsx`) holds the key in state (`setAgentKey`) and the profile is
saved only by Save — or better, auto-saved *on pairing success* with the name
prefilled from the reported hostname, then auto-connected. Kills Bug 2's orphan
profiles; failed pairing leaves zero residue.

**0.4 Make discovery clicks go somewhere.**
- Selected machine gets a visible selected state (border/accent + check).
- Selecting scrolls/focuses the code input with helper text naming the machine:
  *"Enter the code shown on WORKSTATION-2 (run `faro-agentd pair` there if none
  is showing)."*
- Match discovered fingerprints against saved profiles' pinned keys and show
  **"already paired — Connect"** on those rows instead of the pair affordance.
- Auto-run the scan when the Faro Agent protocol tab opens (keep the manual
  rescan button).

**0.5 Honest, actionable errors.**
- Connect against a pair-mode daemon / pair against a run-mode daemon currently
  both surface as generic handshake failures. Detect and say:
  *"The machine is in pairing mode — finish pairing first"* /
  *"The machine isn't accepting pairing — run `faro-agentd pair` on it."*
- `faro-agentd pair` hitting AddrInUse should say *"another faro-agentd is
  already running on this port — stop it first (this limitation goes away in
  v-next)"* instead of a raw bind error.

**0.6 Tests that mirror reality.**
- Extend `end_to_end.rs` so the pairing client does exactly what
  `agent_pair` does — including `SystemInfo` on the pairing channel (this test
  would have caught Bug 1).
- A test that pairing failure leaves no profile behind (frontend or command
  level).

*Estimate: 1–2 days. No wire-format changes; new app pairs with old daemons.*

### Phase 1 — one daemon mode: always serving, pairable on demand

Kill the run/pair split (Bug 4) so a *running* daemon can accept a pairing.

- **Prelude byte.** The responder must choose `Noise_XX` vs `Noise_XXpsk3`
  before replying, so prefix the handshake with one byte: `0x01` paired,
  `0x02` pairing. Daemon accepts `0x02` only while a pairing window is open.
  Bump `PROTOCOL_VERSION`; new daemons accept the un-prefixed legacy handshake
  for one release to stay compatible with old controllers.
- **`faro-agentd pair` becomes a window, not a mode.** `run` gains an optional
  pairing window (opened at startup with `--pair`, or by running
  `faro-agentd pair` which signals the running instance — simplest v1: `pair`
  just runs serve+pairing together in one process). The daemon prints the code
  while continuing to serve existing controllers.
- **Advertise pairability.** Add `pairable=1` (and drop it when the window
  closes) to the mDNS TXT record so the scan list can render *"ready to pair"*
  vs *"running — open pairing on that machine first"*, and the UI can guide
  instead of failing.
- After pairing succeeds the user can connect immediately — no daemon restart.

*Estimate: 2–3 days including compat handshake tests.*

### Phase 2 — the app **is** the agent (zero-download pairing)

`faro-agentd` is already a Tauri-free `lib + bin` crate
(`faro-agentd/src/lib.rs` exports `Daemon`, `serve`, `serve_pairing`,
`pair_connection`; deps are tokio/serde/mdns-sd — all in the app's tree
already). Link it into `faro_lib` and host the daemon in-process:

- **Settings → Remote control** card:
  - Toggle: *"Let my other machines control this computer"* → starts/stops the
    embedded daemon (serve + mDNS advertise). Off by default.
  - **"Show pairing code"** → big 6-digit code dialog (the pairing window),
    auto-expiring after N minutes; toast when a controller pairs, showing its
    name + fingerprint.
  - Policy controls (read-only / allow exec / allow write) mapping to the
    daemon's existing `Policy`, plus the paired-controllers list with revoke
    (config `peers`), and a view of the audit log.
- The embedded daemon keeps its own identity/config in the standard
  `faro-agentd` config dir, so GUI-hosted and headless modes are
  interchangeable on the same machine (only one at a time on the port).
- Windows Firewall will prompt once on first listen — expected; document it in
  the toggle's helper text.

This is the step that answers the actual complaint: for the user's real
scenario (two Windows PCs, Faro on both), the separate exe, the terminal, and
the download all disappear.

*Estimate: 3–5 days (mostly UI).*

### Phase 3 — installer & CLI: bundle it, offer it

For machines where the headless daemon or CLI *is* wanted:

- **Bundle the binaries with the app.** Add `faro-agentd` and `faro-cli` as
  Tauri sidecars (`bundle.externalBin`, target-triple naming) built in the
  `gui` job of `release.yml` before `tauri-action` runs. They ride along in
  every installer (~a few MB) — no separate download, no rename, no second
  SmartScreen hit.
- **In-app opt-in (the "do you want the CLI?" moment).** First-run/Settings
  offers, VS Code-style:
  - *"Install `faro` command line tool"* → copy/link sidecar + add to PATH.
  - *"Install the agent as a background service"* → register the sidecar
    daemon for autostart (`sc create` / launchd plist / systemd unit via a new
    `faro-agentd install` subcommand), so a controlled machine survives
    reboots without a terminal.
- **Installer checkbox (stretch).** A custom NSIS template can add optional
  components at install time; it's fiddlier than the in-app route and unsigned
  installers get replaced wholesale on update, so do the in-app version first.
- **Headless one-liner** for servers, as a release asset + README snippet:
  `irm https://github.com/<repo>/releases/latest/download/install-agentd.ps1 | iex`
  (and the `curl | sh` twin) — downloads, renames, and installs the service.

*Estimate: 2–4 days (sidecar wiring + PATH/service helpers; NSIS stretch extra).*

### Phase 4 — later (already on the roadmap, unchanged)

Rendezvous + NAT hole-punch + relay for off-LAN reach; QR-code pairing;
daemon auto-update. Nothing in Phases 0–3 blocks or reworks these.

---

## The journey after Phases 0–2 (both machines have Faro)

| # | Step | Where |
|---|------|-------|
| 1 | Settings → Remote control → toggle on → **Show pairing code** | target |
| 2 | New Connection → Faro Agent — machine appears (auto-scan), click it | controller |
| 3 | Type the 6 digits → paired, saved, and connected | controller |

Three steps, zero downloads, zero terminals, and the consent ceremony is
intact. Headless targets keep the daemon path, now bundled + service-installable.

## Compatibility

| Controller \ Target | old daemon | new daemon | embedded agent |
|---|---|---|---|
| old app  | broken today (Bug 1) | works (0.1 answers SystemInfo) | works |
| new app  | works via 0.2 fallback | works | works |

Phase 1's prelude byte is the only wire change; new daemons keep a one-release
legacy-handshake fallback, and `PROTOCOL_VERSION` gates anything beyond that.

## Acceptance criteria

- [ ] Pairing through the UI reports success when the daemon pins the
      controller — including against a v1.3.x daemon.
- [ ] A failed pairing leaves no connection in the sidebar and says why it
      failed in actionable terms.
- [ ] Clicking a discovered machine always produces visible state and a next
      step; already-paired machines show as such.
- [ ] Pair → connect works without restarting anything on the target.
- [ ] Two fresh Faro installs on one LAN pair in under 30 seconds with no
      terminal and no extra downloads (Phase 2).
- [ ] The e2e suite drives the *actual* controller pairing sequence.

## Suggested order

Phase 0 immediately (it's small and the feature is unusable without it), then
Phase 2 before Phase 1 if we want the fastest route to the "wow" demo — the
embedded agent can launch on the existing two-mode protocol (the GUI simply
runs the pairing window like `faro-agentd pair` does) and pick up Phase 1's
single-mode elegance after. Phase 3 rides any release train.
