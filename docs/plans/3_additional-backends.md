# Plan 3 — Additional connection sources (backends)

## Context

Faro today speaks SFTP, FTP/FTPS, S3-compatible object storage, local disk, and
the Faro Agent — each a `RemoteFs` impl in `src-tauri/src/remotefs/`
(`sftp.rs`, `ftp.rs`, `object.rs`, `local.rs`, `agent.rs`). Because browse,
transfer, directory sync, and the (planned) continuous sync engine all sit on
that one trait, **every new backend inherits all of them for free** — proven by
the Agent, which slotted in and immediately worked with the explorer and
transfers. This plan widens the connection list, cheapest-first.

Each backend is the same recipe: implement `RemoteFs`
(`list_dir`/`stat`/`read`/`write`/`rename`/`delete`/`create_dir`/`chmod` +
`capabilities`), add a `Session` variant, add a New-Connection UI entry, and
wire chunked read/write into `transfer.rs`.

---

## Phases (priority order)

### Phase 0 — S3-compatible presets (≈ no backend code)
`object.rs` already speaks the S3 API, so these are **endpoint/region presets**,
not new backends: **Cloudflare R2, Backblaze B2, Wasabi, MinIO, DigitalOcean
Spaces, Storj, Google Cloud Storage (interop mode)**. Work: a preset list in the
New Connection dialog so users pick a provider and only enter keys. Highest
value-to-effort in the whole plan.

### Phase 1 — WebDAV (`webdav.rs`)
One protocol covers **Nextcloud, ownCloud, and a long tail of generic servers**.
`reqwest` + `PROPFIND` (list/stat), `GET`/`PUT` (read/write, ranged),
`DELETE`, `MKCOL` (mkdir), `MOVE` (rename). Auth: Basic or Bearer.
Capabilities: real directories yes, `chmod` no, rename via `MOVE`. Low effort,
high coverage — do this first among real backends.

### Phase 2 — SMB/CIFS (`smb.rs`)
Windows shares and **every NAS** (Synology, QNAP, TrueNAS). LAN-first, plain
user/pass — no OAuth. Implement over a Rust SMB client
([`pavao`](https://crates.io/crates/pavao) wraps libsmbclient, or an `smb2`
crate); optional mDNS/NetBIOS discovery. Capabilities: real dirs + rename yes,
`chmod` no. Biggest single real-world gap for a FileZilla-class tool. Watch the
Rust SMB ecosystem maturity (may need a `libsmbclient` FFI dependency).

### Phase 3 — Azure Blob + native Google Cloud Storage
The two big object stores your S3 backend can't reach as-is (different APIs).
Reuse `object.rs`'s object-semantics patterns (faked dirs, copy+delete rename).
- **Azure Blob:** `azure_storage_blobs` crate; auth via account key or SAS.
- **GCS (native):** `google-cloud-storage` crate; service-account JSON.

### Phase 4 — OAuth consumer clouds (Drive / OneDrive / Dropbox) — the hard tier
Big user draw, meaningfully harder:
- **OAuth flow** (loopback/PKCE), token storage (reuse the identity-file
  pattern from `faro-agent-proto`), and refresh handling — a shared
  `oauth.rs` helper used by all three.
- **Path-vs-ID impedance:** Drive/OneDrive address files by opaque ID, not
  path; the trait is path-based. Each backend needs a path↔ID resolver/cache.
- Rate limits + ret/backoff.
Dropbox and Drive also expose **delta cursors** — worth surfacing as a
push/delta capability so the Plan 2 sync engine can skip polling for them.

---

## Cross-cutting
- Each backend declares honest `Capabilities` — the sync engine (Plan 2) and UI
  rely on `change_signal`, `has real dirs`, and `atomic rename` flags.
- Chunked read/write must plug into `transfer.rs` for resumable transfers.
- New Connection UI: group backends (SSH-family / Object storage / Network
  shares / Cloud drives) as the list grows.

## Recommended first batch
**Phase 0 + Phase 1 + Phase 2** (S3 presets + WebDAV + SMB): the largest
coverage gain for the least code, all username/password (no OAuth), all a clean
fit for the trait. That single batch turns "SFTP/FTP/S3 client" into "connects
to almost any file source on a LAN or in the cloud." Azure/GCS and the OAuth
drives follow once the token infrastructure is worth building.

## Key files
- new `src-tauri/src/remotefs/{webdav.rs,smb.rs,azure.rs,gcs.rs,gdrive.rs,onedrive.rs,dropbox.rs}`
- `src-tauri/src/remotefs/mod.rs` (register impls; extend `Capabilities`)
- `src-tauri/src/session/` (add `Session` variants)
- new `src-tauri/src/oauth.rs` (Phase 4 shared helper)
- frontend New Connection dialog (presets + grouped backend picker)

## Risks
- SMB Rust maturity → possible C FFI (`libsmbclient`) and its build/deploy cost.
- OAuth clouds' ID-addressing fights a path-based trait — needs a resolver layer.
- Provider rate limits; multipart ETag ≠ MD5 (ties into sync change detection).
- Credential storage for many providers → consider OS keychain integration.

## Verification
Per backend: connect, browse, up/download a file (chunked), rename, delete;
then attach it to a Plan 2 sync pair unchanged to confirm the engine is truly
backend-agnostic.
