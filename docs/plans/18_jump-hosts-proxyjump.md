# Plan 18 — Jump hosts (ProxyJump) & Zero-Trust connectivity

## Context

Real-world driver: teams locking servers down by IP (or closing inbound
ports entirely behind a Cloudflare Tunnel) while every developer works from
home on a dynamic IP. The standard answer is "everyone reaches the target
through one fixed-identity path" — an SSH bastion (`ssh -J` / `ProxyJump`)
or a Zero-Trust tunnel (`cloudflared access`). Faro has **neither** today:
`ssh_connect` does a single direct TCP connect to `host:port`, there is no
proxy support of any kind, and the OpenSSH importer silently discards
`ProxyJump` lines. For a locked-down server Faro simply cannot connect —
which pushes users back to raw OpenSSH and out of the app.

Two complementary capabilities, one plan:

1. **Native jump hosts (ProxyJump)** — connect to a target *through* another
   Faro profile acting as bastion, with full per-hop auth and host-key
   verification. The `ssh -J` mechanism, done natively in russh. This is the
   generally useful feature: it works for any bastion, not one vendor.
2. **Cloudflare Tunnel integration** — profile-level "via Cloudflare Tunnel"
   option that spawns `cloudflared access tcp` and connects through the
   local port it exposes. Pure convenience over what users can already do by
   hand (run `cloudflared`, point Faro at `localhost:<port>`); explicitly
   the second priority, built only if the manual flow proves annoying.

Everything downstream of `ssh_connect` — SFTP, SCP, terminal PTY, transfers,
sync, `faro-cli`, and the Agent Bridge — inherits jump support for free,
because they all ride the same `SshSession`. `faro-agentd` connections are
*not* SSH and are out of scope (jumping to an agentd host would mean
TCP-forwarding the agent port — a separate, later feature).

## What already exists (don't rebuild)

- **One connection choke point** — `ssh_connect` (`src-tauri/src/session/mod.rs`)
  is the only place an SSH transport is created. GUI, `faro-cli`
  (`open_session`), and transparent `reconnect` all route through it. Add the
  jump here and every surface inherits it.
- **russh 0.45 primitives for exactly this** — `client::connect_stream`
  accepts any `AsyncRead + AsyncWrite` stream, and
  `Handle::channel_open_direct_tcpip` + `Channel::into_stream()` (already
  used by `open_sftp_channel` and the SCP code) turns a channel into that
  stream. No new dependencies, no library changes.
- **Per-hop trust + auth for free** — each hop runs a real russh handshake,
  so `ClientHandler::check_server_key` → `known_hosts` → GUI/CLI prompt
  fires naturally for the bastion too, and all three `AuthMethod` variants
  (password / key / agent) work unchanged per hop.
- **Profile store patterns** — `ConnectionProfile` (`profiles/mod.rs`) is
  plain JSON with optional fields skipped on serialize; adding one more
  optional field keeps existing `profiles.json` loading untouched.
- **A bastion fixture already planned** — Plan 13 Phase 5 (Docker SSH E2E
  fixtures) names a bastion (ProxyJump) container and even lists
  "ProxyJump-through-forward" as a consumer. This plan is the feature that
  fixture was waiting for; reuse it rather than building a second rig.
- **Importer skeleton** — `importers/openssh.rs` already parses
  `Host/HostName/User/Port/IdentityFile`; `ProxyJump` is one more directive.

## Approach

### Phase 1 — Jump chain in the backend

The non-negotiable invariant: **a jumped session must behave identically to
a direct one for everything downstream**, including transparent reconnect.

- **Profile schema** (`profiles/mod.rs`): add
  `jump_host: Option<String>` holding the **id of another
  `ConnectionProfile`** (not duplicated host/auth fields — the bastion needs
  its own full auth, and a reference gives multi-hop chains by recursion for
  free). Optional + `skip_serializing_if` per the struct's existing pattern.
- **`ssh_connect`**: when `jump_host` is set — resolve the jump profile from
  the `ProfileStore` (error clearly if missing or non-SSH), recursively
  `ssh_connect` to it, `channel_open_direct_tcpip(target.host, target.port)`
  on the bastion handle, `into_stream()`, then
  `client::connect_stream(config, stream, handler)` in place of
  `client::connect`. Recursion carries a **depth cap (8) and cycle
  detection** on visited profile ids.
- **Chain lifetime**: the target's transport dies if the bastion connection
  drops, so `SshSession` must hold the jump handle(s) alongside its own
  (a nested list of hop handles, dropped after the target). `reconnect`
  re-establishes the **whole chain** outermost-first.
- **Error surfacing**: identify which hop failed in the error message
  ("jump host 'bastion': connection refused") — chains make opaque errors
  miserable otherwise.

### Phase 2 — GUI + CLI surface

- `src/lib/types.ts`: `jumpHost?: string` on `ConnectionProfile`.
- `ProfileEditor.tsx`: a "Connect via jump host" dropdown listing the user's
  SSH/SFTP profiles (exclude the profile being edited; show chain depth
  indirectly by just listing profiles — cycle prevention is enforced
  backend-side).
- Small badge/indicator on connection rows that jump ("via bastion") so the
  topology is visible in the connection list.
- `faro-cli` needs nothing — it resolves the same profiles — but verify
  `faro-cli` connect/exec through a jumped profile works and prints hop
  errors legibly.

### Phase 3 — OpenSSH importer

- `importers/openssh.rs`: parse `ProxyJump` (and `ProxyCommand cloudflared
  access ssh --hostname %h` → see Phase 4 note). A `ProxyJump` alias that
  matches another imported/importable `Host` block becomes a `jump_host`
  reference; unmatched aliases import as a stub profile flagged "incomplete"
  so the chain still resolves by name after the user fills it in.

### Phase 4 — Cloudflare Tunnel integration (optional, deferred)

Only if the manual `cloudflared access tcp --url localhost:<port>` flow
(works today, zero Faro code) proves annoying in practice:

- Profile fields: `cf_tunnel: bool` + reuse `host` as the tunnel hostname.
- Locate `cloudflared` (PATH + per-OS default install dirs), actionable
  error with install link if missing.
- On connect: spawn `cloudflared access tcp --hostname <host> --url
  localhost:<free-port>` as a managed child, wait for the ready line, then
  `ssh_connect` to the local port (no jump logic involved — plain connect).
- Child lifecycle tied to the session: kill on disconnect, respawn on
  reconnect, reap on app exit.

### Phase 5 — E2E against the Plan 13 fixtures

- Consume the bastion fixture from Plan 13 Phase 5: direct-connect profile
  rejected by the target's firewall rule, jumped profile succeeds;
  browse/transfer/terminal through the chain; kill the bastion mid-session →
  target session reports the hop failure; reconnect rebuilds the chain.
- Cycle/depth-cap unit tests need no Docker (resolve against a synthetic
  `ProfileStore`).

## Integration points

`src-tauri/src/profiles/mod.rs` (field), `src-tauri/src/session/mod.rs`
(`ssh_connect`, `SshSession` hop handles, `reconnect`),
`src-tauri/src/importers/openssh.rs`, `src/lib/types.ts`,
`src/components/ProfileEditor.tsx`, connection-list badge,
`tests/e2e/` (Plan 13 Phase 5 fixtures). Phase 4 adds a small
`src-tauri/src/cftunnel.rs` child-process manager.

## Risks

- **Handle lifetime bugs** — dropping a hop handle early kills the chain;
  order of fields in `SshSession` matters (Rust drops in declaration order).
  Covered by the Phase 5 kill-the-bastion test.
- **Reconnect storms through a chain** — a flapping bastion triggers
  whole-chain rebuilds; reuse the existing single-flight reconnect guard and
  don't parallelize hop rebuilds.
- **Profile deletion breaking chains** — deleting a profile that others
  reference as `jump_host` must warn and list dependents, not silently leave
  dangling references (dangling refs already error clearly from Phase 1,
  but a GUI warning is kinder).
- **Phase 4 scope creep** — cloudflared is a moving external binary; keep
  the integration dumb (spawn/wait/kill) and let Cloudflare own auth flow.
  Do not bundle or auto-install cloudflared.
- **`faro-agentd` confusion** — docs/UI copy must be clear that jump hosts
  apply to SSH connections only; agent connections have their own
  (Noise-paired, direct-TCP) transport.

## Verification

`cargo check -p faro` + `npx tsc --noEmit` clean. Runtime, per phase:

1. Against the Docker bastion fixture (or two real boxes): jumped profile
   connects, browses, transfers, opens a terminal; target firewalled to
   refuse direct connections proves the traffic really transits the bastion.
   `faro-cli` connect/exec through the same profile works.
2. Kill the bastion → session errors name the failed hop; reconnect (bastion
   restored) rebuilds the chain transparently.
3. Import an `~/.ssh/config` with `ProxyJump` → profiles arrive chained.
4. Two-hop chain (A → B → C) works; a cyclic chain (A → B → A) is rejected
   with a clear cycle error; depth cap enforced.
5. (Phase 4 only) `cf_tunnel` profile connects with no manual cloudflared
   process; closing the session leaves no orphan `cloudflared` processes.
