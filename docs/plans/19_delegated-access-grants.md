# Plan 19 — Delegated access grants (`faro://grant`)

> Status: **Faro side implemented** (deep link, grant exchange, keychain keys,
> bastion hop, consent UI, mock issuer — 140 Rust tests + `tsc` green,
> uncommitted). The ServerKit issuer extension is the follow-up.
> Protocol spec: **`docs/grant-links.md`** (open, issuer-agnostic — read that
> first). Supersedes the sharing ideas sketched in
> `99_scoped-connection-sharing.md` for the SSH case.

## The use case

An agency owns access to client servers. Client firewalls whitelist **one
static IP** (the agency's bastion). The agency wants to hand a developer
access to *N* servers with **one link** — revocable, expiring, auditable, and
without ever giving the developer a password or the agency's private key.

## Design (chosen)

Issuer-brokered, key-based grants:

- The **issuer** (ServerKit extension, reference implementation) holds real
  access and mints short-lived, one-time **grant tokens**.
- The link (`faro://grant?issuer=…&token=…`) carries only the token — never a
  credential, and it never auto-connects (consent dialog, same trust model as
  `docs/deep-links.md`).
- Faro generates a **per-user ed25519 keypair locally**, uploads only the
  public key, and imports the granted servers as normal profiles grouped under
  the issuer's folder. Private key lives in the **OS keychain**, not
  `profiles.json`.
- Optional per-connection **jump host (bastion)** gives every developer the
  same static source IP — the static-IP requirement that motivated this.

Alternatives considered: shipping SSH creds in the link (rejected — violates
the deep-link security model, unrevocable once leaked); routing everything
through faro-agentd pairings (rejected for now — agent isn't on the servers
today; Track D may revisit).

## Faro work items (this plan implements)

1. **Deep link** — `src-tauri/src/deeplink.rs`: `grant` action + `issuer` /
   `token` / `name` params; whitelist in `handle_urls`; tests. TS mirror in
   `src/lib/types.ts`.
2. **Grant exchange** — new `src-tauri/src/grant.rs`:
   - `fetch_grant_manifest(issuer, token)` — validated GET (HTTPS-only except
     loopback; version/limits checks per spec).
   - `accept_grant(issuer, token, manifest)` — in-memory ed25519 keygen
     (building blocks in `keys.rs`), POST public key, store private key via
     `credentials::set_secret("grant-key:<profile-id>", pem)` + `db::record_keychain`,
     upsert profiles (`group`, jump fields, `AuthMethod::KeyRef`).
   - Structured `FaroError` results; both registered in `lib.rs`
     `invoke_handler!`.
3. **Keychain-backed SSH auth** — new `AuthMethod::KeyRef { key_ref }`
   variant; `ssh_connect` resolves it via `credentials::get_secret` +
   `russh_keys::decode_secret_key` (never touches disk). `delete_profile`
   cleans up the keychain entry. ProfileEditor renders it read-only as
   "Managed key (grant)".
4. **Bastion / ProxyJump** — `ConnectionProfile` gains optional
   `jump_host` / `jump_port` / `jump_username`; `ssh_connect` chains:
   connect to jump (same auth material) → `channel_open_direct_tcpip(target)`
   → `client::connect_stream`. All session consumers (SFTP, terminal) ride the
   same path.
5. **Consent UI** — `GrantDialog`: on deep link, fetch + show issuer, grant
   name, expiry, server list; **Accept** runs the exchange, imports profiles
   into the rail group, toasts the result; failures surface via
   `toastError`/`FaroError`.
6. **Tests & verification** — unit tests for deep-link parsing + manifest
   validation (incl. the snake_case issuer wire form, verbatim from the spec);
   `src-tauri/tests/grant_issuer_mock.py` (a two-endpoint reference issuer,
   doubles as sample code for third parties), curl-verified: manifest 200,
   key upload `{installed, failed}`, unknown token 404. A full
   `scripts/verify-grant-flow.mjs` driving the Tauri commands needs a running
   app instance and is deferred to the issuer-extension phase. Definition of
   done met: `cargo check -p faro` clean, `cargo test -p faro` 140 green,
   `npx tsc --noEmit` exit 0.

## ServerKit issuer extension (follow-up, spec ready)

Backend-only extension (frontend optional later), modeled on
`serverkit-localkit` + `serverkit-analytics` precedents — see ServerKit
`docs/EXTENSIONS.md`:

- `plugin.json`: `entry_point: "grants:bp"`, `url_prefix: "/api/v1/faro-grants"`,
  `permissions: ["db", "agent.command:file:write"]`, a `jobs` entry for TTL
  expiry sweeps.
- **Admin routes** (`@admin_required`): create grant (pick servers, TTL,
  optional bastion, scope) → returns the `faro://grant` link to copy;
  list/revoke grants.
- **Public routes** (token-authed, rate-limited — the analytics `/collect`
  pattern): the two `/.well-known/faro-grant/…` endpoints from the spec.
- **Storage**: `ext_serverkit_faro_grants` table (hashed token, server id set,
  jump config, TTL, redeemed/revoked state) via manifest `models`.
- **Key install/revoke**: `agents.for_plugin(slug).run(server_id, "file:write",
  …)` appends/removes the one `authorized_keys` line (fan-out via
  `fleet_sweep()` per `docs/FLEET_CONTRACT.md`), after
  `require_permission(slug, "agent.command:file:write")`.
- **Audit**: `audit("grant.issued" / "grant.redeemed" / "grant.revoked", …)`.

Nothing in this is ServerKit-specific on the wire — any panel implementing the
two endpoints in `docs/grant-links.md` can issue grants to Faro.

## Explicit non-goals (v1)

- Password-based grants (key-install only).
- Multi-hop jump chains (single bastion hop).
- Granting faro-agent pairings (Track D territory).
- ServerKit extension UI polish (link-copy button in the panel is enough).
