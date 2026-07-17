//! Windows Cloud Filter provider for on-demand virtual folders (Plan 9).
//!
//! Built on the safe `cloud-filter` wrapper over `cldapi.dll`, so the unsafe
//! minifilter surface stays in a vetted crate rather than hand-rolled FFI —
//! important for code that can only be verified manually in Explorer.
//!
//! Per on-demand pair we:
//!   1. Register a **per-pair sync root** (`Faro.<pair-id>` + the user's SID) so
//!      each folder is its own registration and orphan cleanup is exact.
//!   2. Eagerly create top-level placeholders from a `RemoteFs::list_dir` walk.
//!   3. Connect a [`FaroFilter`] whose `fetch_data` **hydrates** on access by
//!      downloading through the same `TransferManager` path the UI uses, and
//!      whose `fetch_placeholders` lazily populates subdirectories on expand.
//!
//! The `SyncFilter` callbacks are synchronous and run on OS threads, so they
//! bridge to Faro's async backends via a captured `tokio::runtime::Handle` and
//! `block_on` — safe here because these are cldapi-owned threads, never tokio
//! workers.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tauri::AppHandle;
use tokio::runtime::Handle;

use cloud_filter::error::{CResult, CloudErrorKind};
use cloud_filter::filter::{info, ticket, Request, SyncFilter};
use cloud_filter::metadata::Metadata;
use cloud_filter::placeholder::{PinOptions, PinState, Placeholder};
use cloud_filter::placeholder_file::PlaceholderFile;
use cloud_filter::root::{
    Connection, HydrationType, PopulationType, SecurityId, Session, SyncRootId, SyncRootIdBuilder,
    SyncRootInfo,
};
use cloud_filter::utility::{FileTime, WriteAt};

use super::Hydrator;
use crate::remotefs::FileKind;

/// Cloud Filter reads/writes must be 4 KiB-aligned except the final chunk.
const CHUNK: usize = 64 * 1024;

/// On-demand virtual folders are available when the Cloud Filter API is present
/// (Windows 10 1709+). `is_supported()` is the OS-level probe.
pub fn supported() -> bool {
    cloud_filter::root::is_supported().unwrap_or(false)
}

/// Deterministic per-pair sync-root identity, so `unregister` can be rebuilt from
/// just the pair id (orphan cleanup after a config wipe or crash).
fn sync_root_id(pair_id: &str) -> Result<SyncRootId> {
    let provider = format!("Faro.{pair_id}");
    let sid = SecurityId::current_user().map_err(|e| anyhow!("current user SID: {e:?}"))?;
    Ok(SyncRootIdBuilder::new(provider).user_security_id(sid).build())
}

/// A live on-demand root: the Cloud Filter connection plus its registration id.
/// Dropping `connection` disconnects (leaving the folder present-but-offline);
/// full removal additionally calls [`unregister_root`].
pub struct Provider {
    connection: Connection<FaroFilter>,
    local_root: PathBuf,
    #[allow(dead_code)] // kept so the id's lifetime is tied to the connection's
    sync_root_id: SyncRootId,
}

impl Provider {
    pub async fn start(
        pair_id: String,
        local_root: PathBuf,
        remote_root: String,
        hydrator: Arc<dyn Hydrator>,
        _app: AppHandle,
    ) -> Result<Provider> {
        let id = sync_root_id(&pair_id)?;

        // Register once (idempotent across restarts). Display name = the folder's
        // own name so Explorer's nav pane reads naturally.
        if !id.is_registered().map_err(|e| anyhow!("is_registered: {e:?}"))? {
            let display = local_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Faro".into());
            let info = SyncRootInfo::default()
                .with_display_name(&display)
                .with_hydration_type(HydrationType::Full)
                .with_population_type(PopulationType::Full)
                .with_version(env!("CARGO_PKG_VERSION"))
                .with_path(&local_root)
                .map_err(|e| anyhow!("sync root path {}: {e:?}", local_root.display()))?;
            id.register(info).map_err(|e| anyhow!("register sync root: {e:?}"))?;
        }

        // Seed the top level from the remote listing so the folder isn't empty
        // before the first Explorer-driven population. Best-effort per entry.
        seed_top_level(&local_root, &remote_root, hydrator.as_ref()).await;

        let filter = FaroFilter {
            hydrator,
            runtime: Handle::current(),
            remote_root,
            local_root: local_root.clone(),
        };
        let connection = Session::new()
            .connect(&local_root, filter)
            .map_err(|e| anyhow!("connect sync root {}: {e:?}", local_root.display()))?;

        Ok(Provider { connection, local_root, sync_root_id: id })
    }

    /// Explorer-style "Free up space": mark every hydrated placeholder Unpinned,
    /// which asks the platform to dehydrate it (our `dehydrate` callback allows
    /// it). Returns how many files we marked. Best-effort; non-placeholders and
    /// already-dehydrated files are skipped.
    pub async fn free_up_space(&self) -> Result<u32> {
        Ok(dehydrate_tree(&self.local_root))
    }

    /// Disconnect (keep the registration + placeholders on disk).
    pub fn stop(self) {
        drop(self.connection);
    }
}

/// Disconnect (if a `Provider` was live it's already dropped by the caller) and
/// unregister the pair's sync root — the orphan-safety primitive. Rebuilds the
/// deterministic id from `pair_id`, so it works even when we only have the
/// persisted bookkeeping and no live provider.
pub fn unregister_root(pair_id: &str, _local_root: Option<&Path>) -> Result<()> {
    let id = sync_root_id(pair_id)?;
    if id.is_registered().map_err(|e| anyhow!("is_registered: {e:?}"))? {
        id.unregister().map_err(|e| anyhow!("unregister sync root: {e:?}"))?;
    }
    Ok(())
}

/// Create top-level placeholders for remote entries not already present locally.
async fn seed_top_level(local_root: &Path, remote_root: &str, hydrator: &dyn Hydrator) {
    let entries = match hydrator.list_dir(remote_root).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("virtualfs: seed list_dir {remote_root}: {e:#}");
            return;
        }
    };
    for entry in entries {
        if local_root.join(&entry.name).exists() {
            continue;
        }
        let is_dir = matches!(entry.kind, FileKind::Directory);
        let mut pf = PlaceholderFile::new(&entry.name)
            .metadata(metadata_for(is_dir, entry.size, entry.modified))
            .mark_in_sync()
            .blob(entry.path.clone().into_bytes());
        if !is_dir {
            pf = pf.has_no_children();
        }
        // `create`'s phantom generic `P` is unused in its args, so it must be
        // named explicitly (crate quirk in 0.0.6).
        if let Err(e) = pf.create::<&Path>(local_root) {
            tracing::warn!("virtualfs: seed placeholder {}: {e:?}", entry.name);
        }
    }
}

fn metadata_for(is_dir: bool, size: u64, modified: Option<i64>) -> Metadata {
    let mut m = if is_dir { Metadata::directory() } else { Metadata::file() };
    m = m.size(size);
    if let Some(raw) = modified {
        // Faro reports mtime in ms on some backends, seconds on others — detect.
        let secs = if raw > 1_000_000_000_000 { raw / 1000 } else { raw };
        if let Ok(ft) = FileTime::from_unix_time(secs) {
            m = m.written(ft);
        }
    }
    m
}

/// Recursively mark files Unpinned (request dehydration). Returns the count.
fn dehydrate_tree(dir: &Path) -> u32 {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for entry in rd.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => n += dehydrate_tree(&path),
            Ok(ft) if ft.is_file() => {
                if let Ok(mut ph) = Placeholder::open(&path) {
                    if ph.mark_pin(PinState::Unpinned, PinOptions::default()).is_ok() {
                        n += 1;
                    }
                }
            }
            _ => {}
        }
    }
    n
}

/// The Cloud Filter callback surface for one on-demand root.
struct FaroFilter {
    hydrator: Arc<dyn Hydrator>,
    /// Bridge from the sync OS callback threads into Faro's async backends.
    runtime: Handle,
    remote_root: String,
    local_root: PathBuf,
}

impl FaroFilter {
    /// Map an absolute local directory path back to its remote counterpart.
    fn remote_dir_for(&self, absolute: &Path) -> String {
        let rel = absolute.strip_prefix(&self.local_root).unwrap_or(absolute);
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            self.remote_root.clone()
        } else {
            format!("{}/{}", self.remote_root.trim_end_matches('/'), rel)
        }
    }

    /// Hydrate `[start, end)` of `remote_path` into the OS via the ticket, by
    /// downloading the file through the shared transfer path and streaming it in
    /// 4 KiB-aligned chunks.
    fn hydrate(
        &self,
        remote_path: &str,
        ticket: &ticket::FetchData,
        start: u64,
        end: u64,
    ) -> CResult<()> {
        // Security: only ever serve paths inside this pair's remote root
        // (callbacks are OS-supplied; validate before touching the backend).
        if !remote_path.starts_with(self.remote_root.trim_end_matches('/')) {
            return Err(CloudErrorKind::ValidationFailed);
        }

        let temp_dir = std::env::temp_dir()
            .join("faro-vfs")
            .join(uuid::Uuid::new_v4().to_string());
        let hydrator = self.hydrator.clone();
        let rp = remote_path.to_string();
        let td = temp_dir.clone();
        let downloaded = self
            .runtime
            .block_on(async move { hydrator.download_to(&rp, &td).await });

        let result = (|| -> CResult<()> {
            let path = downloaded.map_err(|_| CloudErrorKind::InvalidRequest)?;
            let mut file = File::open(&path).map_err(|_| CloudErrorKind::InvalidRequest)?;
            file.seek(SeekFrom::Start(start))
                .map_err(|_| CloudErrorKind::InvalidRequest)?;

            let mut position = start;
            let mut buffer = vec![0u8; CHUNK];
            while position < end {
                let mut read = file
                    .read(&mut buffer)
                    .map_err(|_| CloudErrorKind::InvalidRequest)?;
                if read == 0 {
                    break;
                }
                // Keep writes 4 KiB-aligned unless this is the final chunk.
                let unaligned = read % 4096;
                if unaligned != 0 && position + (read as u64) < end {
                    read -= unaligned;
                    file.seek(SeekFrom::Current(-(unaligned as i64)))
                        .map_err(|_| CloudErrorKind::InvalidRequest)?;
                }
                if read == 0 {
                    continue;
                }
                ticket
                    .write_at(&buffer[..read], position)
                    .map_err(|_| CloudErrorKind::InvalidRequest)?;
                position += read as u64;
                let _ = ticket.report_progress(end, position);
            }
            Ok(())
        })();

        let _ = std::fs::remove_dir_all(&temp_dir);
        result
    }
}

impl SyncFilter for FaroFilter {
    fn fetch_data(
        &self,
        request: Request,
        ticket: ticket::FetchData,
        info: info::FetchData,
    ) -> CResult<()> {
        let remote_path = String::from_utf8_lossy(request.file_blob()).into_owned();
        let range = info.required_file_range();
        self.hydrate(&remote_path, &ticket, range.start, range.end)
    }

    fn fetch_placeholders(
        &self,
        request: Request,
        ticket: ticket::FetchPlaceholders,
        _info: info::FetchPlaceholders,
    ) -> CResult<()> {
        let absolute = request.path();
        let remote_dir = self.remote_dir_for(&absolute);
        let hydrator = self.hydrator.clone();
        let entries = self
            .runtime
            .block_on(async move { hydrator.list_dir(&remote_dir).await })
            .map_err(|_| CloudErrorKind::InvalidRequest)?;

        let mut placeholders = entries
            .into_iter()
            .filter(|e| !absolute.join(&e.name).exists())
            .map(|e| {
                let is_dir = matches!(e.kind, FileKind::Directory);
                let mut pf = PlaceholderFile::new(&e.name)
                    .metadata(metadata_for(is_dir, e.size, e.modified))
                    .mark_in_sync()
                    .overwrite()
                    .blob(e.path.into_bytes());
                if !is_dir {
                    pf = pf.has_no_children();
                }
                pf
            })
            .collect::<Vec<_>>();

        ticket
            .pass_with_placeholder(&mut placeholders)
            .map_err(|_| CloudErrorKind::InvalidRequest)?;
        Ok(())
    }

    /// Allow dehydration ("free up space"): turn the file back into a placeholder.
    fn dehydrate(
        &self,
        _request: Request,
        ticket: ticket::Dehydrate,
        _info: info::Dehydrate,
    ) -> CResult<()> {
        ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest)?;
        Ok(())
    }

    /// On-demand folders are a live view of the remote: a local delete removes
    /// only the placeholder, never the backend file. Allow the local op.
    fn delete(&self, _request: Request, ticket: ticket::Delete, _info: info::Delete) -> CResult<()> {
        ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest)?;
        Ok(())
    }

    /// Likewise a local rename stays local — we don't mutate the backend.
    fn rename(&self, _request: Request, ticket: ticket::Rename, _info: info::Rename) -> CResult<()> {
        ticket.pass().map_err(|_| CloudErrorKind::InvalidRequest)?;
        Ok(())
    }
}
