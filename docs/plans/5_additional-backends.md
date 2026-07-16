# Plan 5 — Additional connection sources (backends)

## Context

Faro today speaks SFTP, FTP/FTPS, S3-compatible object storage, **Azure Blob**
(both via `object_store`), local disk, and the Faro Agent — each a `RemoteFs`
impl in `src-tauri/src/remotefs/` (`sftp.rs`, `ftp.rs`, `object.rs`,
`local.rs`, `agent.rs`). Because browse, transfer, directory sync, and the
continuous folder-sync engine all sit on that one trait, **every new backend
inherits all of them for free** — proven by the Agent, which slotted in and
immediately worked with the explorer and transfers. This plan widens the
connection list, cheapest-first.

Each backend is the same recipe:

1. Implement `RemoteFs` (`list_dir`/`rename`/`delete`/`create_dir`/`chmod` +
   `capabilities` — declare an honest `ChangeSignal`).
2. Add a `Session` variant (`session/mod.rs`) and its connect path.
3. Wire chunked read/write + stat into the `transfer.rs` match arms.
4. Add a New-Connection UI entry (`ProfileEditor.tsx`, `src/lib/types.ts`).

---

## Phases (priority order)

### Phase 0 — More S3-compatible presets (≈ no backend code) — partially shipped
`S3_PROVIDER_PRESETS` (`src/lib/types.ts`) + `guessProvider` already ship
**AWS, Cloudflare R2, Backblaze B2**. Extend the same table:
- **First tier:** Wasabi, MinIO, DigitalOcean Spaces, Storj, Hetzner Object
  Storage, GCS interop mode (HMAC keys — superseded by Phase 1 native auth but
  still zero-code).
- **Long tail (cheap to add, judge demand):** Scaleway, Linode/Akamai, Vultr,
  Oracle OCI, IBM COS, Supabase Storage; self-hosted **Ceph RGW, Garage,
  SeaweedFS** (one generic "S3-compatible / self-hosted" preset may cover all
  three).
Work per entry: preset row (label, endpoint hint, default region) + a
`guessProvider` endpoint pattern. Highest value-to-effort in the whole plan.

### Phase 1 — Native Google Cloud Storage (nearly free now)
Azure Blob shipped by flipping `object_store`'s `azure` feature and adding one
builder in `session/object.rs` — **GCS is the identical move**: enable the
`gcp` feature, `GoogleCloudStorageBuilder` with service-account JSON (file
path or pasted key), protocol `"gcs"`, one `ProfileEditor` section. No new
files, no new crate. Do this before WebDAV; it was budgeted as a whole crate
(`google-cloud-storage`) when this plan was first written and is now an
afternoon.

### Phase 2 — WebDAV (`webdav.rs`)
One protocol covers **Nextcloud, ownCloud, Hetzner Storage Box, Koofr, Yandex
Disk, Fastmail Files, Synology WebDAV, and a long tail of generic servers**.
`reqwest` + `PROPFIND` (list/stat), `GET`/`PUT` (read/write, ranged),
`DELETE`, `MKCOL` (mkdir), `MOVE` (rename). Auth: Basic or Bearer.
Capabilities: real directories yes, `chmod` no, rename via `MOVE`,
`change_signal: Etag` (populate `DirEntry.etag` from `getetag`; servers that
omit it degrade to mtime+size).
- Ship **WebDAV provider presets** (the Phase 0 idea generalized beyond S3):
  Nextcloud/ownCloud prefill the `/remote.php/dav/files/<user>/` path
  template; Hetzner Storage Box prefills `https://<user>.your-storagebox.de`.
- Note: `object_store` has an `http` feature that lists via PROPFIND, but it
  imposes object semantics (no real dirs) — hand-roll this one so
  `has_directories: true` is honest.

### Phase 3 — SMB/CIFS (`smb.rs`) + the LAN/NAS tier
Windows shares and **every NAS** (Synology, QNAP, TrueNAS). LAN-first, plain
user/pass — no OAuth. Options: [`pavao`](https://crates.io/crates/pavao)
(wraps libsmbclient, C FFI build cost) vs the pure-Rust `smb` crate that has
been maturing — re-evaluate at build time. Optional mDNS/NetBIOS discovery.
Capabilities: real dirs + rename yes, `chmod` no, `change_signal: MtimeSize`.
Biggest single real-world gap for a FileZilla-class tool.
- **Free bonus:** Azure Files shares speak SMB 3 over port 445 — an enterprise
  backend for zero extra code once SMB works.
- **Stretch — NFS:** rounds out the NAS story; Rust client crates (e.g.
  `nfs3_client`) are immature, so read-mostly first or defer until they firm up.

### Phase 4 — Read-only HTTP(S) source (optional mini-phase)
Browse any static file server: nginx/Apache autoindex (HTML or nginx JSON
listing) parsed into `list_dir`; ranged `GET` for reads. Capabilities: all
mutations `false`; change signal from `ETag`/`Last-Modified` headers. Listing
parse is inherently fragile — scope it to the two common autoindex formats
plus a "no listing, paste a direct URL" mode. Small code, occasionally magic:
pull a release artifact straight into any pane.

### Phase 5 — OAuth consumer clouds (Dropbox / OneDrive / Drive / Box) — the hard tier
Big user draw, meaningfully harder. Shared infrastructure first:
- **`oauth.rs` helper** used by all of them: loopback/PKCE flow (the `oauth2`
  crate), token storage (OS keychain — see cross-cutting), refresh handling.
- Rate limits + retry/backoff.

Then sequence by API friction, **not** brand size:
1. **Dropbox** — API v2 is *path-based*; slots straight into the trait. Pilot
   the OAuth infra here.
2. **OneDrive** — Microsoft Graph supports path addressing
   (`/drive/root:/path`); near-Dropbox effort.
3. **Google Drive** — strictly ID-addressed; needs the path↔ID resolver/cache.
4. **Box** — enterprise draw; ID-addressed like Drive, reuses its resolver.
5. *(Optional)* **pCloud** — path-based API, small effort, smaller audience.

Dropbox, Drive, and OneDrive all expose **delta cursors**
(`list_folder/continue`, `changes.list`, Graph delta) — surface as a
push/delta capability so the folder-sync engine can skip polling for them.

### Considered and passed (for now)
- **Mega** — client-side crypto protocol, no sane Rust path.
- **Proton Drive / iCloud Drive** — no official public API.
- **rsync daemon** — wire-protocol effort exceeds payoff; folder sync over
  SFTP already covers the use case.
- **Git repos / artifact registries** — different mental model; if ever, as a
  Plan 9 virtual folder, not a `RemoteFs`.

---

## Cross-cutting
- Each backend declares honest `Capabilities` — sync and UI rely on
  `change_signal` (`Etag` for WebDAV/object/HTTP, `MtimeSize` for SMB/NFS),
  `has_directories`, and rename semantics. `has_shell` stays `false` for
  everything in this plan.
- **`transfer.rs` refactor before the backend wave:** each `Session` variant
  today appears in ~6 match arms (read/write streams, stat, fs construction).
  Fold those into session-level helpers first so a new backend is one impl,
  not six scattered diffs.
- **OS keychain** (`keyring` crate or Tauri plugin) for passwords and OAuth
  tokens — prerequisite for Phase 5, nice-to-have before it.
- **OpenDAL option:** Apache `opendal` ships WebDAV/Drive/OneDrive/Dropbox/Box
  services behind one trait and could collapse Phases 2 + 5 implementation
  cost. Trade-offs: a second abstraction beside `object_store`, uneven
  per-service maturity, and OAuth token acquisition is still ours. Evaluate
  seriously before hand-rolling Phase 5.
- New Connection UI: group backends (SSH family / Object storage / Network
  shares / Cloud drives / Other) as the list grows; presets are a generic
  mechanism (S3 today, WebDAV/SFTP hosts tomorrow).

## Recommended batches
1. **Phase 0 + Phase 1** — presets + native GCS: near-zero code, immediate
   provider-list growth.
2. **Phase 2 + Phase 3** — WebDAV + SMB: the coverage jump that turns
   "SFTP/FTP/object-store client" into "connects to almost any file source on
   a LAN or in the cloud." All username/password, no OAuth.
3. **Phase 5** — OAuth tier once the token infrastructure is worth building,
   Dropbox first. Phase 4 (HTTP) slots wherever a small win is welcome.

## Key files
- new `src-tauri/src/remotefs/{webdav.rs,smb.rs,http.rs,dropbox.rs,onedrive.rs,gdrive.rs,boxdrive.rs}`
- `src-tauri/src/remotefs/mod.rs` (register impls)
- `src-tauri/src/session/mod.rs` + `session/object.rs` (GCS builder; new
  `Session` variants) · `src-tauri/src/transfer.rs` (match arms → helpers)
- new `src-tauri/src/oauth.rs` (Phase 5 shared helper)
- `src/lib/types.ts` (preset tables) + `src/components/ProfileEditor.tsx`
  (grouped picker, new sections)

## Risks
- SMB Rust maturity → possible C FFI (`libsmbclient`) and its build/deploy cost.
- ID-addressed APIs (Drive, Box) fight a path-based trait — resolver/cache
  layer, and path renames invalidate cached IDs.
- Provider rate limits; multipart ETag ≠ MD5 (ties into sync change detection).
- Autoindex HTML parsing (Phase 4) breaks per-server — keep the scope narrow.
- Credential sprawl across many providers → OS keychain integration.
- If OpenDAL is adopted, its per-service quality becomes a dependency risk.

## Verification
Per backend: connect, browse, up/download a file (chunked), rename, delete;
then attach it to a folder-sync pair unchanged to confirm the shipped engine
is truly backend-agnostic.
