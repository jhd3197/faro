# Plan 20 — HubSpot backend (Design Manager, File Manager & HubDB as a filesystem)

## Context

Faro speaks 14 backends through one `RemoteFs` trait, and the recipe for a new
one is proven twice over (Plans 5 and 18). This plan adds **HubSpot** — and
it's an even better fit than Shopify, because HubSpot's Design Manager **is a
real hierarchical filesystem**, not key-prefix inference: the CMS Source Code
API v3 lists folders with `children` arrays, reads files as octet-streams, and
writes with a multipart PUT. One private-app token unlocks **three** mappable
surfaces in a single connection:

1. **Design Manager** (Source Code API v3) — templates, modules, CSS/JS, HubL
   files, themes. The "website manager". True directories.
2. **File Manager** (Files API v3) — the marketer-facing asset library
   (images, PDFs, docs). Real folders, upload/replace/move/delete.
3. **HubDB** (HubDB API v3) — relational tables behind dynamic pages, with
   first-class CSV export/import — tables as editable virtual files.

Why it's worth a backend slot:

- **No good GUI exists for HubSpot CMS work.** The Design Manager is a
  single-file web editor; the official CLI (`hs fetch`/`hs upload`) is
  terminal-only and whole-tree. Faro's dual-pane browser + diff + in-place
  edit is the same gap-filler story as Shopify themes — drag a patched
  `module.html` onto the portal and it's live (or drafted).
- **Agency workflow, squared.** The people who maintain many client stores
  also maintain many client HubSpot portals. A portal becomes just another
  saved connection on the rail: browse the theme, pull it local, run tools
  against it, push fixes back. Folder sync (local theme checkout ↔ portal
  draft) rides the existing engine.
- **Agent Bridge synergy.** An MCP agent patching a client's portal through an
  approved Faro session (per-command approval, zero credentials shared) is
  exactly the workflow agencies do by hand. HubDB-as-files makes "update the
  pricing table" a file edit the human approves before it lands.
- **It rides existing machinery.** Browse, transfer queue, directory diff,
  folder sync, disk usage, in-place editing — all inherited from the trait.
  Capability differences hide affordances, never reinvent them (PRODUCT.md
  principle 4).

Scope honesty: HubSpot is not a file server, so some mappings are virtual
(draft/published as two roots, HubDB tables as CSV files) and a few
affordances must be declared absent (no chmod, no symlinks, no shell; rename
is copy+delete). The plan declares those honestly via `Capabilities`.

---

## API surface (what we're mapping)

Base: `https://api.hubapi.com`. Auth: **private-app access token** (`pat-…`,
`Authorization: Bearer`). HubSpot sunset legacy API keys in 2022; private apps
are the simple path and need no OAuth dance. Scopes required: `content`
(design manager), `files`, `hubdb`. (OAuth 2.0 exists for public apps and the
`oauth.rs` loopback+PKCE machinery could host it later — dead weight for v1.)

### Surface 1 — CMS Source Code API v3 (Phase 1)

`/cms/v3/source-code/{environment}/…` where `environment` is `draft` or
`published`:

- `GET …/metadata/{path}` (`Accept: application/json`) — file/folder metadata:
  `folder: true`, `children` array, created/updated timestamps. **This is the
  whole directory tree** — no folder download exists; you recurse children.
- `GET …/content/{path}` (`Accept: application/octet-stream`) — download one
  file, binary-safe.
- `PUT …/content/{path}` (multipart/form-data, field `file`) — create **or**
  update, one call (like Shopify's asset PUT).
- `DELETE …/content/{path}` — delete. From `draft` it clears unpublished
  changes; from `published` it removes the file entirely (and immediately
  affects live content that references it).
- `POST …/validate/{path}` — HubL/JSON validation, same errors as the Design
  Manager UI. (Post-v1 nicety; note the hook.)
- `POST /cms/v3/source-code/extract/{path}` — extract an uploaded zip in
  place, async. (Post-v1 nicety for bulk theme import.)

**Draft vs published is the one real design decision.** Uploading to
`published` is equivalent to pressing Publish in the Design Manager — it
deploys instantly *and clears the current draft*. This is HubSpot's analog of
Shopify's `[main]`-theme guardrail, handled the same way: labeling + connect
copy, not a hard gate.

**Extension whitelist** on upload: `css js json html txt md jpg jpeg png gif
map svg ttf woff woff2 zip`. Anything else → user-actionable error *before*
attempting the PUT (client-side extension check).

Composite types: a module is a directory ending in `.module` containing
`module.html`, `module.css`, `meta.json`, etc. — maps naturally as a real
directory. No special-casing.

### Surface 2 — Files API v3 (Phase 2)

`/files/v3/files`, `/files/v3/folders`:

- List: `GET /files/v3/files?parentFolderId=…` (paged) + folder listing.
  Entries carry `id`, `name`, `path`, `size`, `createdAt`/`updatedAt`,
  `defaultHostingUrl`.
- Upload: multipart `POST /files/v3/files` with `folderPath` **or**
  `folderId`, `fileName`, and a required `options.access`
  (`PUBLIC_INDEXABLE` / `PUBLIC_NOT_INDEXABLE` / `PRIVATE`). Replace:
  `PUT /files/v3/files/{id}`. Rename/props: `PATCH`.
- **Folder moves/renames are async** — return a task token to poll. Design
  around it: apply, poll briefly, refresh the listing.
- Read: plain CDN URL for public files; `GET …/{id}/signed-url` for private.
- Delete: `DELETE …/{id}` (recoverable; GDPR-delete variant ignored).

### Surface 3 — HubDB API v3 (Phase 3, read-mostly)

`/cms/v3/hubdb/tables`:

- `GET /tables` + `GET /tables/{idOrName}(/draft)` — table metadata + columns.
- `GET /tables/{idOrName}/rows(/draft)` — rows (paged).
- `POST /tables/{idOrName}/draft/import` — multipart CSV import with a JSON
  `config` (`columnMappings`, `idSourceColumn`, `resetTable`).
- `POST /tables/{idOrName}/draft/publish` — push draft live.

**The row-deletion caveat is real**: a CSV import without row IDs deletes
existing rows not present in the file. Therefore Phase 3 ships **read-only**
(export table → `/{table}.csv` virtual file, pull local, diff) and write-back
only after round-trip semantics (stable row-ID column, sparse PATCH rows as an
alternative to bulk import) are thought through. Same sequencing honesty as
Plan 18 Phase 3.

### Constraints to design around

- **Rate limits**: private apps get ~100 req / 10 s burst (plus daily caps on
  free portals). All three surfaces share **one** portal-wide quota — another
  reason this is one session, not three. Reuse the Shopify pattern: tiny
  token-bucket in the session + `Retry-After` on 429 + 5xx backoff.
- **Pagination**: metadata `children` arrays are unpaginated per folder (fine
  — folders are shallow); Files API and HubDB rows are paged (`limit`/`after`
  cursors).
- **No atomic rename** on the Source Code API: rename = GET + PUT new path +
  DELETE old path (three calls, documented best-effort — same honesty as
  Shopify's PUT+DELETE).
- **No empty dirs** on the Source Code API: folders materialize from file
  paths. `create_dir` materializes `{path}/.faro-keep.txt` (whitelisted
  extension), hidden from `list_dir` — the Shopify `.faro-keep` trick,
  extension-adjusted.

---

## Path mapping

One connection, one portal, three virtual roots:

```
/                              → design (draft), design (published), files, hubdb
/design (draft)/themes/...     → Source Code API, draft environment (read/write)
/design (published)/...        → Source Code API, published environment
/files/library/...             → Files API v3 folder tree (Phase 2)
/hubdb/pricing.csv             → HubDB tables as virtual CSV files (Phase 3, read-only first)
```

- **Root (`/`)**: synthesized — no API call. Two design entries labeled so the
  live one is unmistakable (same `[main]` treatment as Shopify); `files/` and
  `hubdb/` appear as their phases land. Writes into `design (published)` are
  allowed (Faro users are professionals) but the connect-dialog copy says
  *"prefer the draft environment — published writes deploy instantly and clear
  the draft."* (A future Settings toggle could make published read-only —
  noted, not built, same as Shopify.)
- **Capabilities**: `can_chmod: false, can_symlink: false, can_rename: true
  (copy+delete, best-effort), has_directories: true, has_shell: false`.
- **Change signal**: `ChangeSignal::MtimeSize` from metadata
  created/updated timestamps + size — honest; no etag on the Source Code API.
  (Files API likewise carries updatedAt + size.)

---

## Phases

### Phase 1 — Design Manager filesystem (the backend) — ✅ shipped

Shipped as specced: `session/hubspot.rs` + `remotefs/hubspot.rs`, the
throttle extracted to `session/http_throttle.rs` shared with Shopify (its
behavior byte-identical, roundtrip still green), all Session/transfer/editor/
search/preview/CLI arms, `HubSpotSection` in ProfileEditor (token-only,
keychain as `hubspot:{profile_id}`), brand icon, deep-link prefill, and
`tests/hubspot_mock.py` with the `live_hubspot_roundtrip` mock-verified
(149 lib tests + roundtrip green on rustc 1.97). Phases 2–3 below remain
planned.

**Rust (8 files + 2 new):**

1. NEW `src-tauri/src/session/hubspot.rs` — `HubSpotSession` modeled on
   `session/shopify.rs` (which is already the throttled-REST template): shared
   `reqwest::Client` (`Faro/{version}` UA), `api_base` const with
   `env_or("FARO_HUBSPOT_API_BASE", …)` override for the mock, one generic
   `send()` with **token-bucket pacing + 429 `Retry-After` + 5xx backoff**
   (lift the Shopify helper — if it's copy-paste, extract it to a shared
   `session/http_throttle.rs` used by both, don't fork it). Static bearer
   token, no exchange dance. `hubspot_connect(profile)`: resolves the token
   from the keychain, probes `GET /cms/v3/source-code/draft/metadata/` (root)
   — connect-time validation with a user-actionable error on 401/403 naming
   the missing scope. `account_label()` → portal id/domain if cheaply
   fetchable, else "HubSpot".
   - **Credentials**: no `host` field (api.hubapi.com is fixed; the client
     follows HubSpot's region redirect); the `pat-…` token stored in the **OS
     keychain** via `credentials.rs` (`set_secret("hubspot:{profile_id}", …)`)
     — never in plaintext `profiles.json`. Profile editor only ever
     `set`/`has` (secrets-never-cross-IPC rule).
2. NEW `src-tauri/src/remotefs/hubspot.rs` — `HubSpotFs` impl:
   - `list_dir`: root → the virtual roots; `/design (…)` paths → recursive
     `metadata` walk with a short (~30 s) per-folder cache; filter
     `.faro-keep.txt`.
   - `read`/`write` helpers (`content_get`, `content_put`) used by the
     transfer/editor arms; bytes are bytes (octet-stream down, multipart up)
     — no text/binary split needed unlike Shopify. Client-side extension
     whitelist check before PUT with a clear error.
   - `rename`: GET + PUT + DELETE (best-effort, documented).
   - `delete`: content DELETE; recursive delete on a directory walks the
     metadata tree (throttle-aware).
   - `create_dir`: `.faro-keep.txt` placeholder (see constraints).
   - `chmod`: `Err(anyhow!("not supported"))` like other clouds.
   - Inline `#[cfg(test)]` unit tests: path↔environment parsing, metadata-JSON
     → `DirEntry`, extension whitelist, `.faro-keep.txt` filtering.
3. `session/mod.rs` — `Session::HubSpot(Arc<HubSpotSession>)` + the five arms
   (`profile()`, `protocol()` → `"hubspot"`, `open_session()`,
   `connect_with_verifier()` (`let _ = app;` HTTP pattern), `disconnect()` —
   stateless). (Sibling arms sit at L1219/L1725/L1823 today.)
4. `commands.rs` `fs_for_session()` (L795) + `transfer.rs` `fs_for_session()`
   (L1789) + `faro-cli/src/main.rs` `fs_for` — factory arms (exhaustive
   matches force all three).
5. `transfer.rs` — `run_hubspot_download` / `run_hubspot_upload` modeled on
   the shopify arms (L309/L567), dispatch arms in `start_download` /
   `start_upload`, `remote_size()` (L1874 via metadata), `remote_resolve()`
   (L2054 via `exists()` against metadata).
6. `editor.rs` L470/L686 read+write arms (in-place HubL/CSS editing — the
   killer feature; matches are exhaustive so this is required anyway).
7. `search.rs:726` — add to the content-search download opt-in list.
8. `preview.rs:282` — add a `read_head` arm (HubL/JSON/CSS previews come free
   once reading works).

**Frontend (7 files):**

- `src/lib/types.ts` — `"hubspot"` in `Protocol` (L64), `DEFAULT_PORT` 443
  (L112), `PROTOCOL_LABEL` "HubSpot" (L129).
- `src/lib/brandIcons.tsx` — `hubspot: "simple-icons:hubspot"`; add to
  `CURATED` in `scripts/gen-brand-icons.mjs`, run `npm run gen:brand-icons`.
- `src/components/ProfileEditor.tsx` — new `HubSpotSection` (the simplest
  section yet — modeled on the Shopify token mode but **no host field**):
  `PasswordInput` for the private-app token, a scope checklist hint
  (*"token needs `content`, `files`, `hubdb` scopes"*), `protocolHint()`
  copy: *"Browse and edit Design Manager files like an FTP site. Prefer the
  draft environment — published writes deploy instantly."* Add to the
  **"Commerce"** picker group next to Shopify. Validation: `pat-` prefix
  shape + non-empty.
- `src/components/ServerRail.tsx` — group order + label ("HubSpot").
- `src/App.tsx` — add to deep-link `known` protocols
  (`faro://connect?protocol=hubspot` — token can't ride a deep link, so this
  only prefills the editor; still worth it).
- `src/lib/ipc.ts` — no new command needed; secrets go through the existing
  generic `credentials` IPC.

**Tests:**

- NEW `src-tauri/tests/hubspot_mock.py` — Python mock in the family style
  (~180 lines): metadata tree from an in-memory dict, content GET/PUT/DELETE,
  draft/published environments, extension-whitelist 400s, 429 injection for
  the throttle test.
- `live_hubspot_roundtrip` — inline, `#[ignore]`, env-gated on
  `FARO_HUBSPOT_MOCK_URL`: connect → list roots → walk draft tree → upload →
  read back → rename (copy+delete) → delete → published env read → throttle
  honored.

**Definition of done:** with the mock running, browse both design
environments in the GUI, edit a HubL template in-place, drag a local CSS file
into a theme folder, and see the change reflected on re-list. Real-runtime
observation, per CLAUDE.md. (A free HubSpot developer sandbox has full CMS
access for the live leg — no paid portal needed.)

### Phase 2 — File Manager (Files API v3) — ✅ shipped

Shipped: the `/files/` root with paged folder listing (root listing fetches
all pages and filters depth-1 client-side — v3 has no root-scoped param),
multipart upload hardcoded to `PUBLIC_NOT_INDEXABLE`, public CDN + private
signed-URL reads, PATCH rename (v3 stem semantics), the async folder-rename
task-token flow, and real `create_dir`. Cross-folder moves are rejected with
a "copy + delete instead" error. Mock-verified in the extended
`live_hubspot_roundtrip`.

Add the `/files/` virtual root: folder listing + upload/replace/PATCH-rename
+ delete + CDN-URL reads (signed URLs for private files). The async
folder-move task token is the one new shape — poll briefly then refresh.
Uploads default `access: PUBLIC_NOT_INDEXABLE` with a per-connection Settings
note. Makes Faro the bulk asset manager agencies currently do by hand in the
files tool.

### Phase 3 — HubDB as virtual CSV files (read-mostly) — ✅ shipped (read-only half)

Shipped read-only as specced: `/hubdb/{table-name}.csv` exports synthesized
from the table schema + paged rows API — `id` first column (stable row IDs,
the foundation for future write-back), schema-ordered columns, RFC 4180
escaping + `\r\n`. Reads come from the **draft** endpoints (latest state,
consistent with the draft-first labeling; published variants documented as a
future option). `DirEntry` size is honestly 0 (unknowable without fetching
every row; `updatedAt` carries the change signal). All mutations under
`/hubdb/` return the read-only error. Mock-verified in the extended
roundtrip (escaping, nulls, unicode, empty table, unknown-table 404, four
read-only mutation refusals). Write-back remains future work per below.

`/hubdb/{table-name}.csv` — export via rows API (stable row-ID first column),
pull/diff locally. **Read-only at first**; write-back (CSV draft import +
publish, or sparse row PATCH) only after the row-deletion semantics are
designed around. This is the surface the Agent Bridge story cares most about
("update the pricing table" as an approved file edit) — and exactly why it
must not clobber rows.

---

## Non-goals (v1)

- **No OAuth public-app flow** — private-app tokens need no loopback/PKCE;
  `oauth.rs` stays on the shelf unless Faro ever ships as a listed HubSpot
  app.
- **No CRM objects** (contacts/deals/tickets) — this is a file backend, not a
  CRM panel.
- **No pages/blog/landing-page content APIs** — a different surface with its
  own draft model; revisit only if Phase 3's pattern proves out.
- **No HubDB write-back** in v1 — read-only export until row-ID round-tripping
  is safe (see Phase 3).
- **No draft→published push op** beyond what writing to the published
  environment already does — a dedicated "publish draft" action is a future
  affordance, deliberately not gating v1.
- **No validate/zip-extract wiring** — the Source Code API's two bonus
  endpoints are noted hooks, not v1 scope.

## Docs & counts on ship

README tech-stack/roadmap counts ("one `RemoteFs` trait over 14 backends" →
15), ROADMAP status flip, and a phase note appended to
`docs/plans/5_additional-backends.md` pointing here (it is the canonical
backend recipe).
