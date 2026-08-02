# Plan 21 — Dynamics 365 / Dataverse backend (web resources as a filesystem)

## Context

Same proven recipe as Plans 5/18/20: one `RemoteFs` impl, one `Session`
variant, transfer/editor arms, New-Connection UI. This plan adds **Dynamics
365 / Power Platform (Dataverse)** — and the fit is the same one that made
Shopify and HubSpot worth a slot, because **Dataverse web resources are
literally files stored in a table**: the `webresource` entity holds
path-like names (`new_/js/form.js`, publisher prefix + virtual folders),
base64 `content`, a type enum, and timestamps. This is exactly what
XrmToolBox's *Web Resources Manager* plugin exposes today — except that tool
is Windows-only, plugin-driven, and single-file-at-a-time. Faro's dual-pane
browser + diff + folder sync + in-place edit is the same gap-filler story.

Why it's worth a backend slot:

- **The incumbent is clunky.** CRM devs edit form scripts in a textarea in
  the maker portal, or round-trip through XrmToolBox/VS Code extensions
  (Dataverse DevTools). Dragging a patched `account.form.js` from disk onto
  the environment and having it deploy is a genuine workflow upgrade.
- **Agency/consultancy workflow.** Dynamics consultancies maintain *many*
  client environments (dev/test/prod per client). Each environment becomes a
  saved connection on the rail — browse web resources, pull the tree local,
  run linters/tools against it, push fixes back. Folder sync (local checkout
  ↔ environment) rides the existing engine.
- **Microsoft auth is already in the house.** `oauth.rs` (loopback + PKCE)
  and the OneDrive backend already speak Microsoft Entra ID. A delegated
  Dynamics sign-in is a *scope change*, not new machinery — unlike HubSpot,
  which needed nothing new either but for the opposite reason (static token).
- **Agent Bridge synergy.** An MCP agent patching a client's form scripts or
  exporting table data through an approved Faro session (per-command
  approval, zero credentials shared) is the consultancy workflow, automated.

Scope honesty: Dataverse is not a file server. Virtual folders are inferred
from `name` prefixes (the Shopify trick, not HubSpot's real dirs), binary
content moves as base64, changes need an explicit **publish** to go live, and
managed-solution components are read-only in practice. All declared honestly
via `Capabilities` and labeling.

---

## API surface (what we're mapping)

Base: `https://{org}.crm.dynamics.com/api/data/v9.2/` (regional variants:
`crm4` EU, `crm5` APAC, etc. — the org URL is the connection's host field).
OData v4.

### Web resources (Phase 1)

- `GET /webresourceset?$select=name,webresourceid,webresourcetype,modifiedon,ismanaged`
  — the whole "directory tree" in one paged query (`@odata.nextLink`).
  `name` is the path; `webresourcetype` enum: 1 HTML, 2 CSS, 3 JS, 4 XML,
  5 PNG, 6 JPG, 7 GIF, 8 XAP, 9 XSL, 10 ICO, 11 SVG, 12 RESX.
- `GET /webresourceset({id})?$select=content` — one file, base64 in `content`.
- `POST /webresourceset` — create (`{ name, displayname, webresourcetype,
  content: <base64> }`).
- `PATCH /webresourceset({id})` — update content (create ≠ update here,
  unlike Shopify/HubSpot: upsert-by-name is a `$filter` lookup + branch).
- `DELETE /webresourceset({id})` — delete (blocked on managed components and
  on dependency references — surface the OData error honestly).
- `POST /PublishXml` — **publish**: changes to web resources are draft until
  published (`componentstate`: 0 Published / 1 Unpublished). Can target a
  single resource (`<importexportxml><webresources><webresource>{id}…`).
  `RetrieveUnpublishedMultiple` exists for reading drafts; v1 keeps it
  simple (see the publish design decision below).
- Size limit: **5 MB per web resource** by default (org-configurable) —
  refuse larger writes client-side with a clear error.

### Dataverse tables (Phase 2 — the "db helper")

Every table is an OData entity set: `GET /{entityset}?$select=…&$filter=…`
with nextLink paging; `GET /EntityDefinitions` for the schema. This maps to
the HubDB Phase 3 pattern — tables as virtual CSV/JSON files, **read-mostly
first**: export `account.csv`, pull local, diff. Write-back (OData PATCH/POST
rows) only after round-trip semantics are designed (alternate keys, lookups,
choice fields — far easier to clobber than HubDB).

The faro-cli angle (the wp-cli analogy from Plan 10): once a Dynamics session
exists, `faro-cli` gets a `dynamics query "<odata>"` helper that runs an
ad-hoc Web API query against a saved connection and prints CSV/JSON — the
"diagnose via the DB" half of the Plan 10 WordPress workflow, with no server
to SSH into because the "server" is SaaS.

### Auth — two flavors, both already validated in the house

- **Client credentials (app registration + application user)**: `client_id` +
  `client_secret` → token at
  `login.microsoftonline.com/{tenant}/oauth2/v2.0/token`,
  scope `{org-url}/.default`. The agency/daemon pattern; secret in the OS
  keychain. Same exchange shape as Shopify's client-credentials mode — no
  loopback.
- **Delegated (interactive)**: the existing `oauth.rs` loopback+PKCE flow
  with scope `{org-url}/user_impersonation` — a config change on the OneDrive
  pattern (new endpoints/scope, same machinery). A "Sign in with Microsoft"
  button in the editor section, exactly like the OneDrive one.

Connect-time validation: `GET /WhoAmI` — returns user/org ids, doubles as the
`account_label()` source.

### Constraints to design around

- **Service protection limits**: ~6,000 requests / 5 min / user, ≤ 52
  concurrent, 20 min combined execution / 5 min. Reuse the shared
  token-bucket + 429-`Retry-After` + 5xx-backoff helper (Plan 20 extracts it
  from Shopify — by the time this builds, it's one import).
- **Publish semantics (the one real design decision)**: writes land as
  *unpublished*. v1 behavior: after a successful write, immediately
  `PublishXml` that single resource (cheap, targeted) so "save = deployed"
  matches Shopify/HubSpot honesty — and the connect-dialog copy says so:
  *"Edits publish immediately. Prefer a dev/sandbox environment."* A future
  "draft mode" profile toggle (write without publish + a publish-all action)
  is noted, not built.
- **Managed components**: `ismanaged: true` web resources can't be edited in
  place — list them read-only (or hide; listing read-only is more honest),
  `Capabilities`-style, with the OData error as the backstop.
- **No empty dirs / no rename**: folders are name prefixes (`.faro-keep.js`
  placeholder for mkdir — must carry a valid type, so a real extension);
  rename = create-new + delete-old (best-effort, documented).
- **Solution membership**: web resources belong to solutions; creating one
  via the API drops it in the default solution with the default publisher
  prefix. v1 accepts this (documented); `AddSolutionComponent` targeting is a
  Phase 2 nicety for consultancies that ship solutions.

---

## Path mapping

```
/                              → web resource tree, inferred from name prefixes
/new_/js/account.form.js       → one webresource row (type 3)
/new_/css/...                  → type 2
/data/account.csv              → Phase 2 virtual root: tables as files (read-mostly)
```

- **Root (`/`)**: one paged `webresourceset` query, cached ~30 s; tree
  derived from `name` prefixes — the Shopify `ShopifyFs` approach verbatim.
  `/data/` appears when Phase 2 lands.
- **`DirEntry`**: `modifiedon` + decoded content length →
  `change_signal: ChangeSignal::MtimeSize` (honest: no etag; `versionnumber`
  exists but is org-wide, useless per-file).
- **Capabilities**: `can_chmod: false, can_symlink: false, can_rename: true
  (create+delete, best-effort), has_directories: true (virtual),
  has_shell: false`.
- Text vs binary by `webresourcetype`: 1–4, 9, 12 are text (edit-in-place);
  images/XAP/ICO are binary (transfer only) — the Shopify classification
  helper, enum-flavored.

---

## Phases

### Phase 1 — Web resources filesystem (the backend) — ✅ shipped

Shipped: `session/dynamics.rs` + `remotefs/dynamics.rs` (client-credentials
auth only — the keychain blob is `tenant_id:client_id:client_secret`, parsed
with `splitn(3, ':')`; **delegated OAuth deferred**, the ProfileEditor has a
marked spot for the toggle), all Session/transfer/editor/search/preview/CLI
arms, `DynamicsSection` (environment URL with regional-cloud validation),
brand icon, deep-link prefill, and `tests/dynamics_mock.py` with
`live_dynamics_roundtrip` mock-verified (162 lib tests green; hubspot/shopify
roundtrips unaffected). Two documented judgment calls: publish fires after
writes and both halves of rename but **not** after delete (Dataverse deletes
are immediate), and listing sizes are the session-cached decoded length (0
until read — Dataverse has no per-row byte size).

**Rust (8 files + 2 new):**

1. NEW `src-tauri/src/session/dynamics.rs` — `DynamicsSession` on the
   Shopify/HubSpot throttled-REST template: shared `reqwest::Client`
   (`Faro/{version}` UA), `api_base` derived from the org URL with
   `env_or("FARO_DYNAMICS_API_BASE", …)` override for the mock, the shared
   throttle helper, and a `token()` helper: client-credentials exchange
   cached in a `Mutex<(String, Instant)>` (5-min margin, same shape as
   `oauth::RefreshingToken`), or the delegated `RefreshingToken` from
   `oauth.rs`. `dynamics_connect(profile)`: resolves credentials, probes
   `GET /WhoAmI` (user-actionable errors on 401 naming the app-user/
   permission fix), caches nothing else. `account_label()` → org name +
   user.
   - **Credentials**: `host` = org URL (`{org}.crm.dynamics.com`); secret
     (client secret, or OAuth refresh token) in the OS keychain as
     `dynamics:{profile_id}` — never in `profiles.json`.
2. NEW `src-tauri/src/remotefs/dynamics.rs` — `DynamicsFs` impl:
   - `list_dir`: root/subdirs from the cached `webresourceset` query filtered
     by prefix; managed entries flagged read-only; `.faro-keep.js` hidden.
   - `read`/`write` helpers: base64 decode on GET, encode on POST/PATCH
     (lookup-by-`$filter=name eq '…'` → create or update), text/binary by
     type enum, 5 MB client-side guard, `PublishXml` after each successful
     write.
   - `rename`: create-new + delete-old (best-effort, documented; the publish
     step applies to both halves).
   - `delete`: row DELETE; recursive delete iterates cached names under the
     prefix (throttle-aware); dependency/managed errors surface verbatim.
   - `create_dir`: `.faro-keep.js` placeholder (valid type 3, hidden).
   - `chmod`: `Err(anyhow!("not supported"))`.
   - Inline `#[cfg(test)]` unit tests: name↔path mapping, webresourcetype ↔
     text/binary classification, base64 round-trip, prefix tree, PublishXml
     payload construction.
3. `session/mod.rs` — `Session::Dynamics(Arc<DynamicsSession>)` + the five
   arms (`profile()`, `protocol()` → `"dynamics"`, `open_session()`,
   `connect_with_verifier()` — client-credentials path is HTTP-only
   (`let _ = app;`); the delegated path uses the verifier like OneDrive),
   `disconnect()`.
4. `commands.rs` `fs_for_session()` + `transfer.rs` `fs_for_session()` +
   `faro-cli/src/main.rs` `fs_for` — factory arms (exhaustive matches force
   all three).
5. `transfer.rs` — `run_dynamics_download` / `run_dynamics_upload` modeled on
   the shopify arms; dispatch arms in `start_download` / `start_upload`,
   `remote_size()` (content length — note: size needs a content fetch, cache
   it), `remote_resolve()` via name lookup.
6. `editor.rs` read+write arms — in-place form-script editing is the killer
   feature; exhaustive matches require it anyway.
7. `search.rs` — add to the content-search download opt-in list.
8. `preview.rs` — `read_head` arm (JS/CSS/HTML previews come free).

**Frontend (7 files):**

- `src/lib/types.ts` — `"dynamics"` in `Protocol`, `DEFAULT_PORT` 443,
  `PROTOCOL_LABEL` "Dynamics 365".
- `src/lib/brandIcons.tsx` — `dynamics: "simple-icons:dynamics365"`; add to
  `CURATED` in `scripts/gen-brand-icons.mjs`, run `npm run gen:brand-icons`.
- `src/components/ProfileEditor.tsx` — new `DynamicsSection` modeled on the
  Shopify section's auth-mode toggle: **Environment URL** field
  (`{org}.crm.dynamics.com`), auth-mode toggle (**Client credentials**:
  Tenant ID + Client ID + Secret PasswordInputs / **Sign in with Microsoft**:
  the OneDrive-style OAuth button), `protocolHint()` copy: *"Browse and edit
  web resources (form scripts, CSS, HTML) like an FTP site. Edits publish
  immediately — prefer a dev/sandbox environment."* Add to the **"Commerce"**
  picker group (or rename it "Commerce & CRM"). Validation: org-URL format +
  secret present.
- `src/components/ServerRail.tsx` — group order + label (`{org}.crm…`).
- `src/App.tsx` — deep-link `known` protocols
  (`faro://connect?protocol=dynamics&host={org}.crm.dynamics.com` prefill —
  consultancies will love this).
- `src/lib/ipc.ts` — no new command for client credentials; the delegated
  mode needs a `dynamics_authorize` command modeled on `onedrive_authorize`.

**Tests:**

- NEW `src-tauri/tests/dynamics_mock.py` — Python mock in the family style
  (~200 lines): token endpoint, `WhoAmI`, `webresourceset` query with
  `$select`/`$filter`/nextLink paging, row GET/POST/PATCH/DELETE, `PublishXml`
  recording, managed-row write refusal, 429 injection.
- `live_dynamics_roundtrip` — inline, `#[ignore]`, env-gated on
  `FARO_DYNAMICS_MOCK_URL`: connect → WhoAmI → list → upload → read back →
  publish recorded → rename → delete → managed refusal → throttle honored.

**Definition of done:** with the mock running, browse a web-resource tree in
the GUI, edit a form script in-place, see the publish call fire, drag a local
JS file into a folder prefix, and see the change on re-list. Real-runtime
observation, per CLAUDE.md. (The live leg needs a Dataverse dev environment —
free via the Power Apps Developer Plan.)

### Phase 2 — Tables as virtual files + the cli "db helper"

`/data/{table}.csv` read-mostly exports via OData paging, exactly the HubDB
Phase 3 pattern (and the same rule: write-back only after round-trip
semantics — lookups, choices, alternate keys — are designed). Plus the
`faro-cli dynamics query "<odata>"` helper: ad-hoc Web API queries against a
saved connection, CSV/JSON to stdout — the wp-cli-style "diagnose via the DB"
workflow for a SaaS that has no shell.

### Phase 3 — Solution awareness (post-v1, spec later)

List solutions, show membership, `AddSolutionComponent` on create, optional
"publish all customizations" action. Consultancy-grade polish; explicitly not
gating Phases 1–2.

---

## Non-goals (v1)

- **No SharePoint document-management surface** — Dynamics' "documents" tab
  is just SharePoint; Faro already speaks that via the OneDrive/Graph
  backend. Point users there, don't duplicate it.
- **No plugin assemblies / workflows / flows** — deployment-managed
  artifacts, wrong abstraction for a file trait.
- **No annotations/notes attachments** — a per-record file column, not a
  filesystem; revisit only if Phase 2 proves the pattern.
- **No table write-back** in v1 — read-mostly, same sequencing honesty as
  HubSpot/Shopify content phases.
- **No draft-mode toggle** — v1 publishes on write and says so; deferred
  publishing is noted future work.

## Docs & counts on ship

README tech-stack/roadmap backend count bump and ROADMAP status flip.
