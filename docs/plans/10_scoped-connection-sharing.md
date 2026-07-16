# Plan 10 — Scoped, time-boxed connection sharing

## Context

The goal: give a teammate access to **one server (or one path), for a limited
time, without handing over full server credentials** — an agency giving an
employee scoped access to a box. The instinct was "maybe deploy Faro to the
browser like code-server." That's one option, but it's the heavy one (needs a
web frontend + a login system). The lighter, more Faro-native path already
exists in the **agent protocol**, which does pairing + per-peer policy +
revocation today.

## The key insight — sharing without a login system

A shared connection is really a **scoped, expiring grant to reach a machine**.
The Faro Agent (`faro-agentd`) already has the machinery:
- **pairing** (a 6-digit code → a pinned peer),
- **per-peer policy** (`allowExec` / `allowWrite`, read-only),
- **revocation** (drop a peer).

Extend that with **scope** (a path jail) and **expiry** (a TTL) and you get
"share this box, read-only, under `/var/www`, for 24h" — no accounts, no
website. The teammate pairs their own Faro to the shared agent with a grant that
auto-expires and is path/permission-limited.

## Approach

### Track 10a — Scoped agent grants (the tractable, no-login path)
Extend `faro-agentd` peers/policy (`src-tauri/faro-agentd/src/config.rs`,
`server.rs`, and `agent_host.rs` on the GUI side):
- **Path scope** — a peer grant can pin a root prefix; the daemon rejects ops
  outside it (validate in `ops.rs` against the grant, like a chroot). Reuses the
  existing per-op gating.
- **Expiry** — a grant carries `expiresAt`; the daemon refuses a pinned peer past
  it and auto-prunes (mirror the pairing-window `Instant` expiry already there).
- **Share UX** — "Share this connection" produces a scoped pairing code (role:
  read-only / path / TTL). The recipient pairs their Faro and sees only the
  jailed, time-boxed view. Revoke early anytime (already supported).
- Works for a box running the embedded host **or** a headless `faro-agentd`; and
  since DeviceKit/ServerKit hosts can run the agent, this shares those too.

### Track 10b — Browser view (deferred; needs auth)
A code-server-style web Faro (browse/transfer/terminal in a browser tab, handed
to someone via a link). This is the big one: it needs a web-served frontend and
**a real auth/login layer** — explicitly out of scope for now, captured here so
the design is on record. If pursued, the scoped-grant model from 10a becomes its
permission backbone (a web session = a scoped grant), so 10a is the right
foundation to build first regardless.

## Phases
1. **Grant model** — add scope (path) + expiry to agent peer grants; enforce in
   `ops.rs`; auto-prune expired.
2. **Share flow** — "Share connection" → scoped/TTL pairing code; recipient pairs
   into the jailed view; sharer sees/revokes active shares.
3. **Polish** — named shares, activity log per share, one-click revoke-all.
4. **(Deferred) Browser view** — web frontend + login; reuse the grant model as
   its permission layer. Its own project.

## Integration points
`src-tauri/faro-agentd/src/{config.rs,server.rs,ops.rs}` (scope + expiry on
grants), `src-tauri/src/agent_host.rs` (issue/list/revoke scoped shares),
`faro-agent-proto` (grant fields in pairing), the GUI "Share connection" flow.

## Risks
- **Path-jail correctness is security-critical** — validate every op's resolved
  path against the grant root (defend `..`/symlink escapes); default-deny.
- Clock skew on expiry — enforce on the daemon side (authoritative), not the
  controller.
- Don't over-build into a login system by accident — 10a deliberately needs no
  accounts; keep 10b clearly separate and deferred.

## Verification
Pair a second Faro into a scoped, read-only, path-jailed, short-TTL grant:
confirm it can only see under the jail, can't write, and stops working at expiry;
early revoke cuts it immediately; path-escape attempts (`../`, symlinks) are
denied.
