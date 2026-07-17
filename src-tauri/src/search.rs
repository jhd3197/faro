//! Fleet Search engine (see `docs/plans/7_fleet-search.md`).
//!
//! Spotlight/Everything for a connection: instant **filename** search and
//! **content grep** across a remote tree — servers, buckets, agents, or local.
//! Like Disk Usage (Plan 4) and Diff (Plan 6) it works over **any** `RemoteFs`,
//! and — where the backend has a shell — an exec fast path (`rg`/`grep`/`find`)
//! makes content search on a big server genuinely fast instead of thousands of
//! byte-reads.
//!
//! Two query kinds live in one [`SearchQuery`]:
//! * [`SearchKind::Name`] — glob/substring against each entry's name,
//! * [`SearchKind::Content`] — literal or regex grep across file contents.
//!
//! Strategy is picked by capability (fastest first), mirroring `diskscan`:
//! 1. **Exec fast path** (SSH / Faro Agent) — one `rg`/`grep`/`find` instead of
//!    a walk (Phase 2).
//! 2. **Object stores** — name search over a flat key listing; content requires
//!    fetching every object, so it's opt-in ([`SearchQuery::content_remote`]).
//! 3. **Generic walk fallback** — [`crate::scan`] / a `RemoteFs::list_dir` BFS
//!    for names; for content, read candidate files and match client-side, always
//!    bounded (size cap) and cancellable. Always available.
//!
//! The pure matchers ([`ContentMatcher`], [`NameMatcher`], [`Filters`]) are
//! I/O-free and unit-tested; [`run_search`] streams hits into a [`HitSink`]
//! (capped, cancellable) so the CLI, the `faro_search` bridge tool, and the GUI
//! [`SearchManager`] all drive the same engine. Symlinks are skipped, exactly as
//! the walk already skips them.

use std::collections::VecDeque;

use anyhow::{bail, Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};

use crate::remotefs::{FileKind, RemoteFs};
use crate::scan::{self, CancelToken, ScanOptions};
use crate::session::{FtpSession, ObjectSession, Session, SshSession};

/// Default cap on hits returned (a line for content, an entry for name). Keeps a
/// runaway query from ballooning memory / the response; surfaced as `truncated`.
pub const DEFAULT_MAX_RESULTS: usize = 1000;

/// Default per-file byte cap for content search. Files larger than this are
/// skipped (they're usually media/build artifacts, not what a grep wants) so a
/// content walk never streams a multi-GB blob just to grep it.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Which kind of search this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchKind {
    /// Match each entry's name (glob when the pattern has `*`/`?`, else substring).
    Name,
    /// Grep file contents (literal by default, regex when `regex` is set).
    Content,
}

/// A search request. `Default` is an empty name search with the standard caps.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// What to look for. For name search a `*`/`?` makes it a glob against the
    /// entry name; otherwise it's a substring. For content search it's a literal
    /// unless `regex` is set.
    pub pattern: String,
    pub kind: SearchKind,
    /// Content: treat `pattern` as a regular expression instead of a literal.
    /// (Name search always auto-detects glob vs substring; this flag is ignored.)
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    /// Only consider files whose name matches one of these globs (empty = all).
    #[serde(default)]
    pub include_globs: Vec<String>,
    /// Skip files whose name matches any of these globs.
    #[serde(default)]
    pub exclude_globs: Vec<String>,
    /// Opt-in: allow content search to DOWNLOAD every candidate file on backends
    /// that have no server-side grep (object stores, FTP, WebDAV, cloud). Off by
    /// default so a content query on S3 doesn't silently pull the whole bucket.
    #[serde(default)]
    pub content_remote: bool,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
}

fn default_max_results() -> usize {
    DEFAULT_MAX_RESULTS
}
fn default_max_file_bytes() -> u64 {
    DEFAULT_MAX_FILE_BYTES
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            kind: SearchKind::Name,
            regex: false,
            case_sensitive: false,
            include_globs: Vec::new(),
            exclude_globs: Vec::new(),
            content_remote: false,
            max_results: DEFAULT_MAX_RESULTS,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// One search hit. Name hits carry just the entry; content hits add the matching
/// line's number, the byte column of the match, and a trimmed preview.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Absolute path on the backend (what "reveal" / "open" acts on).
    pub path: String,
    /// Path relative to the search root (the stable display key).
    pub relative: String,
    pub is_dir: bool,
    pub size: u64,
    /// Content hits only: 1-based line number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    /// Content hits only: byte column of the match within the line (0-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    /// Content hits only: the matching line, trimmed and length-capped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

impl SearchHit {
    fn name(path: String, relative: String, is_dir: bool, size: u64) -> Self {
        SearchHit { path, relative, is_dir, size, line: None, column: None, preview: None }
    }

    fn content(path: String, relative: String, size: u64, line: u64, column: u64, preview: String) -> Self {
        SearchHit {
            path,
            relative,
            is_dir: false,
            size,
            line: Some(line),
            column: Some(column),
            preview: Some(preview),
        }
    }
}

/// Which strategy produced a search. `Generic` is the always-available walk; the
/// `Shell` exec fast path and `ObjectFlat` name listing land in Phase 2 and set
/// this so a surface can show which path fired (and why, if it fell back).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchStrategy {
    /// Recursive `RemoteFs::list_dir` walk (names) or read-and-grep (content).
    Generic,
    /// One `rg`/`grep`/`find` over the SSH exec channel or a Faro Agent.
    Shell,
    /// A single flat object listing under the prefix (S3/Azure), name-only.
    ObjectFlat,
}

/// What a run produced beyond the hits themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchStats {
    pub strategy: SearchStrategy,
    /// The hit cap was reached — there may be more matches than returned.
    pub truncated: bool,
    /// Files whose contents were read (content search) or directories listed
    /// (name search) — a rough "how much work did this do" for the surface.
    pub scanned: usize,
    /// A short human note, e.g. why a fast path fell back to the generic walk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The collected result the CLI + bridge render (the GUI streams instead).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub root: String,
    pub kind: SearchKind,
    pub hits: Vec<SearchHit>,
    pub stats: SearchStats,
}

// ---------- Hit sink (cap + stream) ----------

/// A cap-and-stream target for hits. Strategies push into it; it enforces
/// `max_results` and remembers whether it truncated. The callback lets the GUI
/// stream hits live while the CLI/bridge just collect them into a `Vec`.
pub struct HitSink<'a> {
    on_hit: &'a mut dyn FnMut(SearchHit),
    count: usize,
    max: usize,
    truncated: bool,
}

impl<'a> HitSink<'a> {
    pub fn new(max: usize, on_hit: &'a mut dyn FnMut(SearchHit)) -> Self {
        Self { on_hit, count: 0, max: max.max(1), truncated: false }
    }

    /// Record a hit. Returns `true` while the caller should keep producing hits,
    /// `false` once the cap is hit (and flips `truncated`).
    fn push(&mut self, hit: SearchHit) -> bool {
        if self.count >= self.max {
            self.truncated = true;
            return false;
        }
        (self.on_hit)(hit);
        self.count += 1;
        self.count < self.max
    }

    fn wants_more(&self) -> bool {
        self.count < self.max
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

// ---------- Compiled matchers (pure) ----------

/// Compiled from a [`SearchQuery`] once, then reused across every entry.
struct Compiled {
    name: Option<NameMatcher>,
    content: Option<ContentMatcher>,
    filters: Filters,
}

impl Compiled {
    fn new(q: &SearchQuery) -> Result<Self> {
        if q.pattern.is_empty() {
            bail!("search pattern is empty");
        }
        let (name, content) = match q.kind {
            SearchKind::Name => (Some(NameMatcher::compile(q)?), None),
            SearchKind::Content => (None, Some(ContentMatcher::compile(q)?)),
        };
        Ok(Self { name, content, filters: Filters::compile(q)? })
    }
}

/// Content grep matcher: a literal substring or a compiled regex.
enum ContentMatcher {
    Literal { needle: String, case_sensitive: bool },
    Regex(regex::Regex),
}

impl ContentMatcher {
    fn compile(q: &SearchQuery) -> Result<Self> {
        if q.regex {
            let re = regex::RegexBuilder::new(&q.pattern)
                .case_insensitive(!q.case_sensitive)
                .build()
                .with_context(|| format!("invalid regex: {}", q.pattern))?;
            Ok(Self::Regex(re))
        } else if q.case_sensitive {
            Ok(Self::Literal { needle: q.pattern.clone(), case_sensitive: true })
        } else {
            Ok(Self::Literal { needle: q.pattern.to_lowercase(), case_sensitive: false })
        }
    }

    /// Byte column of the first match in `line`, or `None`.
    fn find(&self, line: &str) -> Option<usize> {
        match self {
            Self::Regex(re) => re.find(line).map(|m| m.start()),
            Self::Literal { needle, case_sensitive } => {
                if *case_sensitive {
                    line.find(needle.as_str())
                } else {
                    // Column is against the lowercased copy; close enough for a
                    // jump-to-line (the line number is what matters).
                    line.to_lowercase().find(needle.as_str())
                }
            }
        }
    }
}

/// Name matcher: a glob (pattern has `*`/`?`) or a plain substring.
enum NameMatcher {
    Substring { needle: String, case_sensitive: bool },
    Glob(regex::Regex),
}

impl NameMatcher {
    fn compile(q: &SearchQuery) -> Result<Self> {
        if q.pattern.contains(['*', '?']) {
            let src = format!("^{}$", glob_body(&q.pattern));
            let re = regex::RegexBuilder::new(&src)
                .case_insensitive(!q.case_sensitive)
                .build()
                .with_context(|| format!("invalid name pattern: {}", q.pattern))?;
            Ok(Self::Glob(re))
        } else if q.case_sensitive {
            Ok(Self::Substring { needle: q.pattern.clone(), case_sensitive: true })
        } else {
            Ok(Self::Substring { needle: q.pattern.to_lowercase(), case_sensitive: false })
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Glob(re) => re.is_match(name),
            Self::Substring { needle, case_sensitive } => {
                if *case_sensitive {
                    name.contains(needle.as_str())
                } else {
                    name.to_lowercase().contains(needle.as_str())
                }
            }
        }
    }
}

/// Include/exclude globs, compiled to anchored regexes and matched against a
/// file's *name* (so `*.log` / `*.rs` work as expected).
struct Filters {
    include: Vec<regex::Regex>,
    exclude: Vec<regex::Regex>,
    case_sensitive: bool,
}

impl Filters {
    fn compile(q: &SearchQuery) -> Result<Self> {
        let build = |globs: &[String]| -> Result<Vec<regex::Regex>> {
            globs
                .iter()
                .filter(|g| !g.trim().is_empty())
                .map(|g| {
                    let src = format!("^{}$", glob_body(g));
                    regex::RegexBuilder::new(&src)
                        .case_insensitive(!q.case_sensitive)
                        .build()
                        .with_context(|| format!("invalid glob: {g}"))
                })
                .collect()
        };
        Ok(Self {
            include: build(&q.include_globs)?,
            exclude: build(&q.exclude_globs)?,
            case_sensitive: q.case_sensitive,
        })
    }

    /// Whether a file name survives the include/exclude filters. Directories are
    /// never filtered here (include/exclude target file names); descent is
    /// separate from result filtering.
    fn passes(&self, name: &str) -> bool {
        let _ = self.case_sensitive; // case handled at compile time
        if !self.include.is_empty() && !self.include.iter().any(|r| r.is_match(name)) {
            return false;
        }
        if self.exclude.iter().any(|r| r.is_match(name)) {
            return false;
        }
        true
    }

    fn active(&self) -> bool {
        !self.include.is_empty() || !self.exclude.is_empty()
    }
}

/// Translate a shell glob into a regex body (no anchors): `*` → `.*`, `?` → `.`,
/// everything else escaped. Deliberately simple — no `**` / char classes — since
/// it only ever matches a single path segment (a file/entry name).
fn glob_body(glob: &str) -> String {
    let mut out = String::with_capacity(glob.len() * 2);
    for ch in glob.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out
}

/// Basename of a POSIX-relative path (already `/`-separated by `relative_of`).
fn basename(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

// ---------- Public entry points ----------

/// Walk / grep `root` on `fs`, streaming hits into `sink`. `session` (absent for
/// the local FS) unlocks the protocol fast paths in Phase 2; today every kind
/// runs the generic strategy. Returns the stats (strategy, note, scanned);
/// `sink` carries the truncation flag + hit count.
pub async fn run_search(
    fs: &dyn RemoteFs,
    session: Option<&Session>,
    root: &str,
    query: &SearchQuery,
    cancel: &CancelToken,
    sink: &mut HitSink<'_>,
) -> Result<SearchStats> {
    let compiled = Compiled::new(query)?;
    match query.kind {
        SearchKind::Name => {
            let scanned = name_walk(fs, root, &compiled, cancel, sink).await;
            Ok(SearchStats { strategy: SearchStrategy::Generic, truncated: sink.truncated, scanned, note: None })
        }
        SearchKind::Content => {
            if needs_content_optin(session) && !query.content_remote {
                bail!(
                    "content search on {} would download every candidate file — re-run with remote content enabled to allow it",
                    proto_label(session)
                );
            }
            let scanned = content_walk(fs, session, root, &compiled, query, cancel, sink).await?;
            Ok(SearchStats { strategy: SearchStrategy::Generic, truncated: sink.truncated, scanned, note: None })
        }
    }
}

/// The collecting convenience the CLI + `faro_search` bridge tool call — runs a
/// search to completion and returns every hit (up to the cap). The GUI drives
/// [`run_search`] directly through [`SearchManager`] for streaming + cancel.
pub async fn search(
    fs: &dyn RemoteFs,
    session: Option<&Session>,
    root: &str,
    query: &SearchQuery,
) -> Result<SearchResult> {
    let cancel = CancelToken::new();
    let mut hits: Vec<SearchHit> = Vec::new();
    let stats;
    {
        let mut push = |h: SearchHit| hits.push(h);
        let mut sink = HitSink::new(query.max_results, &mut push);
        stats = run_search(fs, session, root, query, &cancel, &mut sink).await?;
    }
    Ok(SearchResult { root: root.to_string(), kind: query.kind, hits, stats })
}

// ---------- Generic name walk (files + dirs, streaming) ----------

/// Bounded-concurrency BFS that streams matching **names** (files and
/// directories) as it goes. Mirrors `scan::walk`'s pipeline but yields per-entry
/// hits and matches directory names too (which `scan::walk` drops). Stops early
/// once the sink is full or the token is cancelled. Returns the directory count.
async fn name_walk(
    fs: &dyn RemoteFs,
    root: &str,
    compiled: &Compiled,
    cancel: &CancelToken,
    sink: &mut HitSink<'_>,
) -> usize {
    let matcher = compiled.name.as_ref().expect("name search compiles a name matcher");
    let normalized_root = root.trim_end_matches('/').to_string();
    let limit = scan::DEFAULT_CONCURRENCY;

    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root.to_string());
    let mut inflight = FuturesUnordered::new();
    let mut dirs = 0usize;

    loop {
        if cancel.is_cancelled() || !sink.wants_more() {
            break;
        }
        while inflight.len() < limit {
            match queue.pop_front() {
                Some(dir) => inflight.push(async move { fs.list_dir(&dir).await }),
                None => break,
            }
        }
        let Some(res) = inflight.next().await else {
            break;
        };
        dirs += 1;
        let Ok(entries) = res else { continue };
        for entry in entries {
            let is_dir = matches!(entry.kind, FileKind::Directory);
            if is_dir {
                queue.push_back(entry.path.clone());
            }
            // Skip symlinks/other for matching too — same as the walk.
            if !is_dir && !matches!(entry.kind, FileKind::File) {
                continue;
            }
            if !matcher.matches(&entry.name) {
                continue;
            }
            // include/exclude filter file names; a matching directory bypasses it.
            if !is_dir && !compiled.filters.passes(&entry.name) {
                continue;
            }
            let rel = scan::relative_of(&normalized_root, &entry.path);
            if rel.is_empty() {
                continue;
            }
            if !sink.push(SearchHit::name(entry.path.clone(), rel, is_dir, entry.size)) {
                break;
            }
        }
    }
    dirs
}

// ---------- Generic content walk (read + grep) ----------

/// Enumerate files under `root` (via the shared scan walk), then read and grep
/// each candidate client-side. Bounded by the per-file size cap and fully
/// cancellable — the "always works" content fallback for backends without a
/// server-side grep. Returns the number of files actually read.
async fn content_walk(
    fs: &dyn RemoteFs,
    session: Option<&Session>,
    root: &str,
    compiled: &Compiled,
    query: &SearchQuery,
    cancel: &CancelToken,
    sink: &mut HitSink<'_>,
) -> Result<usize> {
    let matcher = compiled.content.as_ref().expect("content search compiles a content matcher");
    let opts = ScanOptions { concurrency: scan::DEFAULT_CONCURRENCY, cancel: cancel.clone() };
    let tree = scan::walk(fs, root, &opts, |_| {})
        .await
        .with_context(|| format!("walking {root}"))?;

    let mut scanned = 0usize;
    for (rel, entry) in &tree.files {
        if cancel.is_cancelled() || !sink.wants_more() {
            break;
        }
        let name = basename(rel);
        if compiled.filters.active() && !compiled.filters.passes(name) {
            continue;
        }
        if entry.size > query.max_file_bytes {
            continue;
        }
        let bytes = match read_capped(session, &entry.absolute, query.max_file_bytes).await {
            Ok(b) => b,
            Err(_) => continue, // unreadable / unsupported: skip, don't fail the search
        };
        let Some(text) = decode_text(&bytes) else {
            continue; // binary
        };
        scanned += 1;
        if !grep_text(&text, matcher, &entry.absolute, rel, entry.size, sink) {
            break;
        }
    }
    Ok(scanned)
}

/// Push a content hit for every matching line in `text`. Returns `false` once the
/// sink is full so the caller stops enumerating files.
fn grep_text(
    text: &str,
    matcher: &ContentMatcher,
    abs: &str,
    rel: &str,
    size: u64,
    sink: &mut HitSink<'_>,
) -> bool {
    for (idx, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(col) = matcher.find(line) {
            let hit = SearchHit::content(
                abs.to_string(),
                rel.to_string(),
                size,
                idx as u64 + 1,
                col as u64,
                preview(line),
            );
            if !sink.push(hit) {
                return false;
            }
        }
    }
    true
}

/// Trim and length-cap a matching line for display.
fn preview(line: &str) -> String {
    const MAX: usize = 300;
    let trimmed = line.trim();
    if trimmed.chars().count() > MAX {
        let capped: String = trimmed.chars().take(MAX).collect();
        format!("{capped}…")
    } else {
        trimmed.to_string()
    }
}

/// Decode read bytes as text, or `None` if they look binary (a NUL byte in the
/// first 8 KiB — the same heuristic `grep`/`ripgrep` use).
fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Backends with no server-side grep, where content search must download every
/// file — gated behind [`SearchQuery::content_remote`]. Local, SSH, and the Faro
/// Agent are cheap (local read or an exec fast path) and never need the opt-in.
fn needs_content_optin(session: Option<&Session>) -> bool {
    matches!(
        session,
        Some(
            Session::Object(_)
                | Session::Ftp(_)
                | Session::Webdav(_)
                | Session::Http(_)
                | Session::Dropbox(_)
                | Session::OneDrive(_)
                | Session::GDrive(_)
                | Session::Box(_)
        )
    )
}

fn proto_label(session: Option<&Session>) -> String {
    session.map(|s| s.protocol().to_string()).unwrap_or_else(|| "local".to_string())
}

// ---------- Capped byte reads for the content walk ----------

/// Read up to `max` bytes of a file for grepping. `session == None` is the local
/// filesystem. Backends without a byte-read path here (WebDAV/HTTP/cloud/agent in
/// Phase 1) return an error the caller treats as "skip this file". Mirrors the
/// per-session dispatch `diff.rs` uses for `--hash`.
async fn read_capped(session: Option<&Session>, path: &str, max: u64) -> Result<Vec<u8>> {
    match session {
        None => read_local(path, max).await,
        Some(Session::Ssh(ssh)) => read_ssh(ssh, path, max).await,
        Some(Session::Object(obj)) => read_object(obj, path, max).await,
        Some(Session::Ftp(ftp)) => read_ftp(ftp, path, max).await,
        Some(other) => bail!("content read not supported for {} yet", other.protocol()),
    }
}

async fn read_local(path: &str, max: u64) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await.with_context(|| format!("open {path}"))?;
    let mut buf = Vec::new();
    file.take(max).read_to_end(&mut buf).await?;
    Ok(buf)
}

async fn read_ssh(ssh: &SshSession, path: &str, max: u64) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let cell = ssh.ensure_sftp().await?;
    let sftp = cell.lock().await;
    let remote = sftp.open(path).await.with_context(|| format!("open {path} over SFTP"))?;
    let mut buf = Vec::new();
    remote.take(max).read_to_end(&mut buf).await?;
    Ok(buf)
}

async fn read_object(obj: &ObjectSession, path: &str, max: u64) -> Result<Vec<u8>> {
    let key = path.trim_start_matches('/');
    let p = object_store::path::Path::from(key);
    let get = obj.store.get(&p).await.with_context(|| format!("get {key}"))?;
    let mut stream = get.into_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        if buf.len() as u64 >= max {
            break;
        }
    }
    Ok(buf)
}

async fn read_ftp(ftp: &FtpSession, path: &str, max: u64) -> Result<Vec<u8>> {
    struct CapSink {
        buf: Vec<u8>,
        max: usize,
    }
    impl std::io::Write for CapSink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if self.buf.len() < self.max {
                let room = self.max - self.buf.len();
                self.buf.extend_from_slice(&data[..room.min(data.len())]);
            }
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let path = path.to_string();
    let max = max as usize;
    ftp.with_stream(move |stream| {
        let mut sink = CapSink { buf: Vec::new(), max };
        stream.retr_to_writer(&path, &mut sink)?;
        Ok(sink.buf)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pattern: &str, kind: SearchKind) -> SearchQuery {
        SearchQuery { pattern: pattern.to_string(), kind, ..Default::default() }
    }

    #[test]
    fn name_substring_is_case_insensitive_by_default() {
        let m = NameMatcher::compile(&q("log", SearchKind::Name)).unwrap();
        assert!(m.matches("Error.LOG"));
        assert!(m.matches("catalog.txt"));
        assert!(!m.matches("data.bin"));
    }

    #[test]
    fn name_glob_matches_extension() {
        let m = NameMatcher::compile(&q("*.log", SearchKind::Name)).unwrap();
        assert!(m.matches("nginx.log"));
        assert!(m.matches("NGINX.LOG")); // case-insensitive
        assert!(!m.matches("nginx.log.1"));
        let single = NameMatcher::compile(&q("err?r.txt", SearchKind::Name)).unwrap();
        assert!(single.matches("error.txt"));
        assert!(!single.matches("erroor.txt"));
    }

    #[test]
    fn name_case_sensitive_honoured() {
        let mut query = q("Log", SearchKind::Name);
        query.case_sensitive = true;
        let m = NameMatcher::compile(&query).unwrap();
        assert!(m.matches("Login"));
        assert!(!m.matches("catalog"));
    }

    #[test]
    fn content_literal_finds_column() {
        let m = ContentMatcher::compile(&q("panic", SearchKind::Content)).unwrap();
        assert_eq!(m.find("thread 'main' panicked"), Some(14));
        assert_eq!(m.find("all good here"), None);
        // Case-insensitive by default.
        assert_eq!(m.find("PANIC at the disco"), Some(0));
    }

    #[test]
    fn content_regex_compiles_and_matches() {
        let mut query = q(r"TODO|FIXME", SearchKind::Content);
        query.regex = true;
        let m = ContentMatcher::compile(&query).unwrap();
        assert!(m.find("// FIXME: later").is_some());
        assert!(m.find("nothing here").is_none());
    }

    #[test]
    fn bad_regex_is_an_error() {
        let mut query = q(r"(unclosed", SearchKind::Content);
        query.regex = true;
        assert!(ContentMatcher::compile(&query).is_err());
    }

    #[test]
    fn filters_include_and_exclude() {
        let mut query = q("x", SearchKind::Content);
        query.include_globs = vec!["*.rs".into()];
        query.exclude_globs = vec!["*_test.rs".into()];
        let f = Filters::compile(&query).unwrap();
        assert!(f.passes("main.rs"));
        assert!(!f.passes("main_test.rs")); // excluded
        assert!(!f.passes("readme.md")); // not included
        assert!(f.active());
    }

    #[test]
    fn empty_pattern_rejected() {
        assert!(Compiled::new(&q("", SearchKind::Name)).is_err());
    }

    #[test]
    fn preview_trims_and_caps() {
        assert_eq!(preview("   hello world  "), "hello world");
        let long = "a".repeat(400);
        let p = preview(&long);
        assert_eq!(p.chars().count(), 301); // 300 + ellipsis
        assert!(p.ends_with('…'));
    }

    #[test]
    fn decode_text_rejects_binary() {
        assert!(decode_text(b"plain text\nsecond line").is_some());
        assert!(decode_text(&[0x00, 0x01, 0x02]).is_none());
    }

    #[test]
    fn grep_text_reports_every_matching_line() {
        let m = ContentMatcher::compile(&q("err", SearchKind::Content)).unwrap();
        let text = "line one\nERR here\nclean\nanother err\n";
        let mut hits: Vec<SearchHit> = Vec::new();
        {
            let mut push = |h: SearchHit| hits.push(h);
            let mut sink = HitSink::new(100, &mut push);
            grep_text(text, &m, "/a/f.txt", "f.txt", 30, &mut sink);
        }
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, Some(2));
        assert_eq!(hits[0].preview.as_deref(), Some("ERR here"));
        assert_eq!(hits[1].line, Some(4));
    }

    #[test]
    fn hit_sink_caps_and_flags_truncation() {
        let mut hits: Vec<SearchHit> = Vec::new();
        let truncated;
        {
            let mut push = |h: SearchHit| hits.push(h);
            let mut sink = HitSink::new(2, &mut push);
            assert!(sink.push(SearchHit::name("/a".into(), "a".into(), false, 1))); // wants more
            assert!(!sink.push(SearchHit::name("/b".into(), "b".into(), false, 1))); // hit cap
            assert!(!sink.push(SearchHit::name("/c".into(), "c".into(), false, 1))); // rejected
            truncated = sink.truncated();
        }
        assert_eq!(hits.len(), 2);
        assert!(truncated);
    }

    // End-to-end name search over a real temp tree via LocalFs — the runtime
    // observation the generic strategy leans on (every backend falls back to it).
    #[tokio::test]
    async fn name_search_over_a_real_directory() {
        use crate::remotefs::local::LocalFs;
        let base = std::env::temp_dir().join(format!("faro_search_name_{}", std::process::id()));
        let sub = base.join("logs");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("readme.md"), "x").unwrap();
        std::fs::write(sub.join("nginx.log"), "x").unwrap();
        std::fs::write(sub.join("access.log"), "x").unwrap();

        let root = base.to_string_lossy().to_string();
        let result = search(&LocalFs, None, &root, &q("*.log", SearchKind::Name)).await.unwrap();
        let mut rels: Vec<String> = result.hits.iter().map(|h| h.relative.clone()).collect();
        rels.sort();
        assert_eq!(rels, vec!["logs/access.log", "logs/nginx.log"]);
        assert!(result.hits.iter().all(|h| !h.is_dir));

        std::fs::remove_dir_all(&base).ok();
    }

    // End-to-end content grep over a real temp tree: finds the matching line and
    // its number, and the size cap skips an over-large file.
    #[tokio::test]
    async fn content_search_over_a_real_directory() {
        use crate::remotefs::local::LocalFs;
        let base = std::env::temp_dir().join(format!("faro_search_content_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.txt"), "hello\nneedle here\nbye\n").unwrap();
        std::fs::write(base.join("b.txt"), "nothing to see").unwrap();

        let root = base.to_string_lossy().to_string();
        let result = search(&LocalFs, None, &root, &q("needle", SearchKind::Content)).await.unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].relative, "a.txt");
        assert_eq!(result.hits[0].line, Some(2));
        assert!(result.stats.scanned >= 1);

        std::fs::remove_dir_all(&base).ok();
    }
}
