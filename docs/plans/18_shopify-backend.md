# Plan 18 — Shopify backend (themes & store content as a filesystem)

## Context

Faro speaks 13 backends through one `RemoteFs` trait, and the recipe for a new
one is proven (Plan 5): trait impl → `Session` variant + connect path →
`transfer.rs` arms → New-Connection UI entry. This plan adds **Shopify** — and
it's a better fit than it first looks, because **a Shopify theme literally is a
remote filesystem**: the Admin Assets API lists/reads/writes/deletes individual
files (`layout/theme.liquid`, `templates/index.json`, `snippets/*`, `assets/*`)
one key at a time.

Why it's worth a backend slot:

- **No good GUI exists for theme work.** Shopify's own CLI (`shopify theme
  push/pull`) is terminal-only and whole-tree; the admin code editor is a
  single-file textarea. Faro's dual-pane browser + diff + edit-in-place is a
  genuine gap-filler: drag a patched `layout/theme.liquid` from disk onto the
  store and it's deployed.
- **Agency workflow.** People who maintain *many* client stores (the motivating
  case) already keep those servers in Faro. A store becomes just another saved
  connection on the rail — browse the theme, pull it local, run audits/tools
  against it, push fixes back.
- **Agent Bridge synergy.** An MCP agent patching a client's store through an
  approved Faro session (per-command approval, zero credentials shared) is
  exactly the workflow agencies do by hand today.
- **It rides existing machinery.** Browse, transfer queue, directory diff,
  folder sync (local theme checkout ↔ store), disk usage, in-place editing —
  all inherited from the trait. Nothing bespoke in the UI (PRODUCT.md
  principle 4: capability differences hide affordances, never reinvent them).

Scope honesty: Shopify is **not** a file server, so some mappings are virtual
(themes as directories, directories inferred from asset-key prefixes) and a few
affordances must be declared absent (no chmod, no symlinks, no shell). The plan
declares those honestly via `Capabilities` rather than faking them.

---

## API surface (what we're mapping)

REST Admin API (`https://{shop}.myshopify.com/admin/api/{version}/`), token auth:

- `GET /themes.json` — list themes (`id`, `name`, `role`: `main`/`unpublished`/`demo`).
- `GET /themes/{id}/assets.json` — flat list of every asset `key` (+ `size`,
  `updated_at`, `content_type`). This is the whole "directory tree".
- `GET /themes/{id}/assets.json?asset[key]={key}` — read one file. Text assets
  come back as `value`; binary assets as base64 `attachment`.
- `PUT /themes/{id}/assets.json` — write one file (`{ asset: { key, value } }`
  or `{ key, attachment: <base64> }`). Create and update are the same call.
- `DELETE /themes/{id}/assets.json?asset[key]={key}` — delete one file.

Auth — two flavors, both already validated in the wild:
- **Legacy custom-app token**: static `shpat_…` sent as
  `X-Shopify-Access-Token` header. Simplest.
- **Dev Dashboard app (client credentials)**: `client_id` + `client_secret`
  exchanged at `POST /admin/oauth/access_token`
  (`grant_type=client_credentials`); tokens expire after 24h. The session
  fetches on first use and caches with a 5-minute margin — same shape as
  `oauth::RefreshingToken`, no loopback/PKCE involved.

Constraints to design around:

- **Rate limits**: REST leaky bucket ≈ 2 req/s (bucket 40, Plus 80). Faro has
  no throttling pattern yet (Dropbox/GDrive rely on 401-retry only) — Shopify
  needs a real one: a tiny token-bucket in the session + retry-after on 429.
- **Pagination**: `assets.json` is unpaginated (one big list, themes have
  hundreds of files) — fine to fetch once per theme and cache briefly.
- **No atomic rename**: rename = PUT new key + DELETE old key (two calls,
  documented as best-effort).
- **Liquid files are text but JSON templates must stay valid** — Faro just
  moves bytes; validation is the user's business (same as every backend).

---

## Path mapping (the one real design decision)

A Shopify store has *two* hierarchy levels Faro must invent:

```
/                              → themes, as virtual directories
/{theme-name} [main]/          → asset folders inferred from key prefixes
/{theme}/layout/theme.liquid   → one asset
/{theme}/templates/index.json
/{theme}/snippets/header.icons.liquid
```

- **Root (`/`)**: synthesized from `GET /themes.json`. Directory entries named
  `{theme-name} [main]` / `{theme-name}` (role suffix so the live theme is
  unmistakable). Theme id is kept in the session's theme cache keyed by the
  display name.
- **Inside a theme**: fetch `assets.json` once, cache for ~30s, and derive the
  tree from `key` prefixes (`layout/`, `templates/`, …). `DirEntry` for files
  carries `size` + `updated_at` → `change_signal: ChangeSignal::MtimeSize`
  (honest: no etag exists).
- **Capabilities**: `can_chmod: false, can_symlink: false, can_rename: true,
  has_directories: true, has_shell: false`.
- **Live-theme guardrail**: writing into the `[main]` theme is the dangerous
  op. v1 shows it like any other directory (Faro users are professionals) but
  the connect dialog copy says "prefer a duplicate/unpublished theme", and
  `ProfileEditor` gets a hint. (A future Settings toggle could make `[main]`
  read-only — noted, not built.)

---

## Phases

### Phase 1 — Theme filesystem (the backend)

**Rust (8 files + 2 new):**

1. NEW `src-tauri/src/session/shopify.rs` — `ShopifySession` modeled on
   `session/gdrive.rs` shape: shared `reqwest::Client` (`Faro/{version}` UA),
   `api_base` const with `env_or("FARO_SHOPIFY_API_BASE", …)` override for the
   mock, one generic `send()` with **429 retry-after + 5xx backoff**, and a
   `token()` helper: static token pass-through, or client-credentials exchange
   cached in a `Mutex<(String, Instant)>`. `shopify_connect(profile)`:
   resolves credentials, probes `GET /themes.json` (connect-time validation,
   user-actionable error on 401/404), caches the theme list.
   `account_label()` → the shop domain.
   - **Credentials**: `host` = `{shop}.myshopify.com`; the secret (token or
     `client_id:client_secret`) stored in the **OS keychain** via
     `credentials.rs` (`set_secret("shopify:{profile_id}", …)`) — not in
     plaintext `profiles.json`. Profile editor only ever `set`/`has` (the
     existing secrets-never-cross-IPC rule).
2. NEW `src-tauri/src/remotefs/shopify.rs` — `ShopifyFs` impl:
   - `list_dir`: root → theme dirs; `/{theme}` → cached asset keys filtered by
     prefix; subdirs likewise.
   - `read`/`write` helpers (`asset_get`, `asset_put`) used by transfer/editor
     arms; text via `value`, binary via base64 `attachment` (detect by
     `content_type`/extension — Liquid/JSON/CSS/JS/SVG are text, the rest
     binary).
   - `rename`: PUT-then-DELETE (best-effort, documented).
   - `delete`: asset DELETE; recursive delete on a "directory" iterates the
     cached keys under the prefix (rate-limit aware).
   - `create_dir`: materializes `{path}/.faro-keep` placeholder asset (Shopify
     has no empty dirs; key prefixes appear when a file exists — placeholder is
     hidden from `list_dir`).
   - `chmod`: `Err(anyhow!("not supported"))` like other clouds.
   - Inline `#[cfg(test)]` unit tests: key↔path mapping, asset-JSON →
     `DirEntry`, text/binary classification, prefix tree building.
3. `session/mod.rs` — `Session::Shopify(Arc<ShopifySession>)` + the five arms
   (`profile()`, `protocol()` → `"shopify"`, `open_session()`,
   `connect_with_verifier()` (`let _ = app;` HTTP pattern), `disconnect()` —
   stateless, nothing to close).
4. `commands.rs` `fs_for_session()` (L754) + `transfer.rs` `fs_for_session()`
   (L1717) + `faro-cli/src/main.rs` `fs_for` (L~736) — factory arms
   (exhaustive matches force all three).
5. `transfer.rs` — `run_shopify_download` / `run_shopify_upload` modeled on
   the dropbox arms (L1172/L1215), dispatch arms in `start_download` (L~269),
   `start_upload` (L~517), `remote_size()` (L1799 via cached asset `size`),
   `remote_resolve()` (L1903 via `exists()` against the key cache).
6. `editor.rs` L368/L558 read+write arms (in-place edit — the killer feature
   for theme work; matches are exhaustive so this is required anyway).
7. `search.rs:722` — add to the content-search download opt-in list.
8. `preview.rs` — add a `read_head` arm (Liquid/JSON/CSS previews come free
   once reading works).

**Frontend (7 files):**

- `src/lib/types.ts` — `"shopify"` in `Protocol`, `PROTOCOL_LABEL` ("Shopify"),
  no default port (443 implicit); helper `isShopifyProtocol` if needed.
- `src/lib/brandIcons.tsx` — `shopify: "simple-icons:shopify"`; add to
  `CURATED` in `scripts/gen-brand-icons.mjs`, run `npm run gen:brand-icons`.
- `src/components/ProfileEditor.tsx` — new `ShopifySection` (modeled on
  `WebdavSection`, not OAuth): **Store domain** field
  (`{shop}.myshopify.com`), **auth-mode toggle** (Access token / Client
  credentials), `PasswordInput` for the token or Client ID + Secret pair,
  `protocolHint()` copy: *"Browse and edit theme files like an FTP site.
  Prefer an unpublished/duplicate theme — changes deploy instantly."* Add to a
  picker group (new **"Commerce"** group, or "Cloud drives" for now).
  Validation: domain format + secret present.
- `src/components/ServerRail.tsx` L48/L56 — group order + label
  (`{shop}.myshopify.com`).
- `src/App.tsx:607` — add to deep-link `known` protocols (enables
  `faro://connect?protocol=shopify&host=…` prefills — agencies will love this).
- `src/lib/ipc.ts` — no new command needed (no OAuth dance); secrets go
  through the existing generic `credentials` IPC.

**Tests:**

- NEW `src-tauri/tests/shopify_mock.py` — Python mock in the family style
  (~160 lines): `themes.json`, asset list/get/put/delete against an in-memory
  dict, 429 injection for the throttle test.
- `live_shopify_roundtrip` — inline, `#[ignore]`, env-gated on
  `FARO_SHOPIFY_MOCK_URL`: connect → list themes → list assets → upload →
  read back → rename → delete → throttle honored.

**Definition of done:** with the mock running, browse a two-theme store in the
GUI, edit a Liquid file in-place, drag a local file into a theme, and see the
change reflected on re-list. Real-runtime observation, per CLAUDE.md.

### Phase 2 — Store media (Files API)

A second root directory `/media/` exposing admin **Content → Files** (product
images, docs) via GraphQL `files` query + staged uploads for write. Read is
plain CDN URLs (streams fine); upload is the 3-step staged-upload dance. Makes
Faro the bulk image manager agencies currently do by hand in the admin.
Separate `GraphQL` client in the session (same throttle).

### Phase 3 — Content as virtual files (read-mostly)

Products / collections / pages / articles exposed as editable `*.json` (or
`.md` with front-matter) virtual files under `/content/…` via GraphQL.
Read + write-back of the JSON round-trips the resource. This turns Faro +
folder-sync into a poor-man's content backup/migration tool. **Read-only at
first**; write only after field-mapping is thought through (metafields,
variants, translations are easy to clobber). Explicitly post-v1.

---

## Non-goals (v1)

- **No OAuth loopback** — Shopify's own app models (custom-app token, Dev
  Dashboard client credentials) don't need it; PKCE flow would be dead weight.
- **No Storefront API / checkout / orders / customers** — this is a file
  backend, not a store admin panel.
- **No live-theme write protection beyond labeling** — noted as a possible
  Settings toggle, deliberately not gating v1.
- **No theme push/pull whole-tree command** — folder sync already does this
  generically once Phase 1 lands; don't duplicate it.

## Docs & counts on ship

README tech-stack/roadmap counts ("one `RemoteFs` trait over 13 backends" →
14), roadmap `next` line, and a phase note appended to
`docs/plans/5_additional-backends.md` pointing here (it is the canonical
backend recipe).
