// Faro command-line client. Reuses faro_lib's RemoteFs / Session layers
// (the same code the GUI runs against) so any backend the GUI supports —
// SFTP, FTP, FTPS, S3, R2, B2, Azure Blob — works here too.
//
// Path syntax:
//   ./local/path             local
//   /etc/hosts               local (absolute)
//   C:\Users\me              local (Windows)
//   prod:/var/log            remote, using saved profile "prod"
//
// Saved profiles are read from the same on-disk store the GUI uses, so
// connections you set up in the app are immediately scriptable.

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use faro_lib::profiles::{ConnectionProfile, ProfileStore};
use faro_lib::remotefs::{DirEntry, FileKind, RemoteFs};
use faro_lib::session::{
    open_session as faro_open_session, FtpSession, HostDecision, HostKeyVerifier,
    HostPromptKind, ObjectSession, Session,
};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "faro-cli",
    about = "Faro CLI — SFTP / FTP / S3 / R2 / B2 / Azure Blob from the terminal",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List a directory.
    Ls {
        /// Path. Use `profile:/path` for a saved profile.
        target: String,
        /// Show file sizes in bytes (default: human-readable).
        #[arg(long)]
        bytes: bool,
    },

    /// Run a shell command on a saved SSH profile's server.
    ///
    /// Connects with the profile's stored credentials and runs the command
    /// non-interactively, the same as typing it into that server's shell.
    /// stdout/stderr are passed through and the command's exit code becomes
    /// this process's exit code.
    Exec {
        /// Saved profile name or id (must be SSH/SFTP).
        profile: String,
        /// The command to run. Quote it, or pass it as trailing arguments.
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },

    /// Copy a file from one location to another.
    Cp {
        source: String,
        destination: String,
    },

    /// Rename / move a file (must be within a single backend).
    Mv { source: String, destination: String },

    /// Remove a file. Use --recursive for directories.
    Rm {
        target: String,
        #[arg(short, long)]
        recursive: bool,
    },

    /// Create a directory.
    Mkdir { target: String },

    /// Mirror or copy a directory tree between local and remote.
    Sync {
        /// Local directory.
        local: String,
        /// Remote directory (must be `profile:/path`).
        remote: String,
        /// Direction. Defaults to push (local to remote).
        #[arg(long, value_enum, default_value = "push")]
        direction: Dir,
        /// Mirror mode also deletes destination files that don't exist on the
        /// source. Off by default — the planner is additive.
        #[arg(long)]
        mirror: bool,
        /// Show the plan without executing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Compare two directory trees and report what differs.
    ///
    /// Each side is `profile:/path` (a saved profile), `local:/path`, or a plain
    /// local path — so **remote↔remote** works too (staging vs prod, two servers,
    /// two buckets), which no local diff tool can do. By default files are
    /// compared by size; `--hash` confirms same-size files by content (sha256).
    /// Exits 0 when the trees are identical, 1 when they differ.
    Diff {
        /// Side A: `profile:/path`, `local:/path`, or a local path.
        a: String,
        /// Side B: `profile:/path`, `local:/path`, or a local path.
        b: String,
        /// Confirm same-size files by content hash (sha256). Opt-in — it reads
        /// every same-size file (server-side over SSH where possible).
        #[arg(long)]
        hash: bool,
        /// Emit the full result as JSON (every entry, including unchanged).
        #[arg(long)]
        json: bool,
        /// Also list unchanged files in the human output (JSON always includes them).
        #[arg(long)]
        all: bool,
    },

    /// Search a directory tree by file name or by content (grep).
    ///
    /// The target is `profile:/path` (a saved profile) or a local path. Name
    /// search matches each entry's name (a `*`/`?` makes it a glob, else a
    /// substring); `--content` greps file contents (literal, or `--regex`).
    /// On SSH / Faro Agent servers content search runs `rg`/`grep` server-side;
    /// object stores name-match a flat key listing. Exits 0 when something
    /// matched, 1 when nothing did.
    Search {
        /// Target: `profile:/path` or a local path.
        target: String,
        /// What to look for (a name glob/substring, or a content pattern).
        pattern: String,
        /// Grep file contents instead of matching names.
        #[arg(long)]
        content: bool,
        /// Treat the content pattern as a regular expression (implies --content).
        #[arg(long)]
        regex: bool,
        /// Case-sensitive matching (default: insensitive).
        #[arg(long)]
        case_sensitive: bool,
        /// Only consider files whose name matches this glob (repeatable).
        #[arg(long)]
        include: Vec<String>,
        /// Skip files whose name matches this glob (repeatable).
        #[arg(long)]
        exclude: Vec<String>,
        /// Allow content search to DOWNLOAD every file on backends with no
        /// server-side grep (object stores, FTP, WebDAV, cloud). Off by default.
        #[arg(long)]
        content_remote: bool,
        /// Cap on returned hits (default 1000).
        #[arg(long)]
        max: Option<usize>,
        /// Emit the raw JSON result.
        #[arg(long)]
        json: bool,
    },

    /// Manage saved profiles.
    Profiles {
        #[command(subcommand)]
        action: ProfileCmd,
    },

    /// Operate a server through Faro's running Agent Bridge.
    ///
    /// Unlike the other subcommands (which open their own connection), `agent`
    /// talks to the Bridge you already have open in the Faro app — so commands
    /// go through Faro's per-command approval and show up in its live console.
    /// The bridge URL + token are read from Faro's local discovery file, so you
    /// only pass a server name; you never handle a URL or token.
    Agent {
        #[command(subcommand)]
        action: AgentCmd,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List every saved profile.
    List,
    /// Show details for one profile by name or id.
    Show { name: String },
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Show agent context: bridge state, policy, enabled sessions, saved commands.
    Context,
    /// List the servers the Agent Bridge can currently reach.
    Sessions,
    /// Run a shell command on a server (SSH or Faro Agent). Exits with its exit code.
    Exec {
        /// Saved server name (as shown in Faro) or its session id.
        server: String,
        /// Show what would run and whether it would need approval, without executing.
        #[arg(long)]
        dry_run: bool,
        /// Timeout in milliseconds (default 60000; clamped to 1000–900000).
        #[arg(long)]
        timeout_ms: Option<u64>,
        /// The command to run. Quote it, or pass it as trailing arguments.
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// List a remote directory.
    Ls {
        server: String,
        /// Remote directory (default: ".").
        path: Option<String>,
    },
    /// Read a remote text file (SSH/SFTP, capped at 256 KiB).
    Read { server: String, path: String },
    /// Read several remote text files at once (SSH/SFTP, total capped at 1 MiB).
    ReadBatch {
        server: String,
        /// Remote file paths. Pass multiple times or space-separated.
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Find files whose name matches a glob pattern, recursively (SSH only).
    Glob {
        server: String,
        /// Root directory to search under (default: ".").
        path: Option<String>,
        /// Glob pattern, e.g. '*.log'.
        pattern: String,
    },
    /// Find entries whose name contains a substring, recursively.
    Search {
        server: String,
        /// Case-insensitive substring to match against entry names.
        query: String,
        /// Root directory to search under (default: ".").
        path: Option<String>,
    },
    /// Download a remote file to a local dir (default: your Downloads).
    Download {
        server: String,
        remote_path: String,
        local_dir: Option<String>,
    },
    /// Upload a local file into a remote directory.
    Upload {
        server: String,
        local_path: String,
        remote_dir: String,
    },
    /// Upload a whole local directory tree into a remote directory. One
    /// approval covers the tree; the prompt shows file/byte counts.
    UploadDir {
        server: String,
        /// Local directory to upload (recreated INSIDE the remote directory).
        local_dir: String,
        /// Remote destination directory.
        remote_dir: String,
        /// Replace existing remote files (default: rename colliding uploads).
        #[arg(long)]
        overwrite: bool,
    },
    /// One-way sync between a local directory and a remote directory. Use
    /// --dry-run first: it prints the plan without changing anything.
    Sync {
        server: String,
        /// Local directory (source for push, destination for pull).
        local_dir: String,
        /// Remote directory (destination for push, source for pull).
        remote_dir: String,
        /// Direction. Defaults to push (local to remote).
        #[arg(long, value_enum, default_value = "push")]
        direction: Dir,
        /// Mirror mode ALSO DELETES destination files missing from the source.
        /// The approval prompt in Faro shows the exact delete count.
        #[arg(long)]
        mirror: bool,
        /// Show the plan without executing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Diff two directory trees through the Agent Bridge — any two connected
    /// servers (remote↔remote), or a server vs local. Read-only.
    ///
    /// Each side is `server:/path` (a connected server), `local:/path`, or a
    /// plain path (local). By default files compare by size; `--hash` confirms
    /// same-size files by content.
    Diff {
        /// Side A: `server:/path`, `local:/path`, or a local path.
        a: String,
        /// Side B: `server:/path`, `local:/path`, or a local path.
        b: String,
        /// Confirm same-size files by content hash (sha256).
        #[arg(long)]
        hash: bool,
        /// Emit the raw JSON result.
        #[arg(long)]
        json: bool,
    },
    /// Check a transfer started via `agent download` / `agent upload`.
    Transfer { transfer_id: String },
    /// Tail a remote log file for up to 30 seconds (SSH only).
    Tail {
        server: String,
        /// Remote log file path.
        path: String,
        /// Number of trailing lines to start with (default 50).
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// Show context about a server (protocol, host, port, …).
    Info { server: String },
    /// Recent Agent Bridge activity (what's already run), newest first.
    History {
        /// Limit to one server (name or session id).
        #[arg(long)]
        server: Option<String>,
        /// Max entries (default 50, max 200).
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Run one of your SAVED, pre-approved commands by name (SSH only). These run
    /// with no approval prompt because you wrote and vetted them in Faro.
    Run {
        /// Saved server name (as shown in Faro) or its session id.
        server: String,
        /// Show the saved command without running it.
        #[arg(long)]
        dry_run: bool,
        /// The saved command's name (see `agent commands`).
        name: String,
    },
    /// List your saved, pre-approved commands.
    Commands,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Dir {
    /// Local → remote.
    Push,
    /// Remote → local.
    Pull,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let store = open_store()?;

    match cli.command {
        Cmd::Ls { target, bytes } => cmd_ls(&store, &target, bytes).await,
        Cmd::Exec { profile, command } => cmd_exec(&store, &profile, &command).await,
        Cmd::Cp { source, destination } => cmd_cp(&store, &source, &destination).await,
        Cmd::Mv { source, destination } => cmd_mv(&store, &source, &destination).await,
        Cmd::Rm { target, recursive } => cmd_rm(&store, &target, recursive).await,
        Cmd::Mkdir { target } => cmd_mkdir(&store, &target).await,
        Cmd::Sync {
            local,
            remote,
            direction,
            mirror,
            dry_run,
        } => cmd_sync(&store, &local, &remote, direction, mirror, dry_run).await,
        Cmd::Diff { a, b, hash, json, all } => cmd_diff(&store, &a, &b, hash, json, all).await,
        Cmd::Search {
            target,
            pattern,
            content,
            regex,
            case_sensitive,
            include,
            exclude,
            content_remote,
            max,
            json,
        } => {
            cmd_search(
                &store, &target, &pattern, content, regex, case_sensitive, include, exclude,
                content_remote, max, json,
            )
            .await
        }
        Cmd::Profiles { action } => match action {
            ProfileCmd::List => cmd_profiles_list(&store).await,
            ProfileCmd::Show { name } => cmd_profiles_show(&store, &name).await,
        },
        // The agent subcommands talk to the running bridge over HTTP (blocking
        // ureq); they need no profile store and don't await.
        Cmd::Agent { action } => cmd_agent(action),
    }
}

// ---- Profile store -----------------------------------------------------

/// Resolve the same on-disk path the Tauri GUI uses for its data dir, so a
/// CLI invocation sees the profiles the user set up in the app.
fn default_data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir().ok_or_else(|| anyhow!("could not resolve a data dir"))?;
    Ok(base.join("com.juandenis.faro"))
}

fn open_store() -> Result<ProfileStore> {
    let dir = default_data_dir()?;
    ProfileStore::from_dir(&dir)
}

// ---- Path parsing ------------------------------------------------------

/// Targets passed on the CLI are either local paths or remote paths prefixed
/// with a saved profile name and a colon. We don't support inline URLs
/// (`sftp://user@host/...`) yet — those need secrets we'd have to prompt for,
/// and the saved-profile path covers the daily-use case.
enum Target {
    Local(String),
    Remote { profile_name: String, path: String },
}

fn parse_target(raw: &str) -> Target {
    // Windows drive letters (`C:\`, `D:\`) look like profiles syntactically.
    // Disambiguate by checking whether the char before the colon is a single
    // letter — drive letters are always `[A-Za-z]:`.
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Target::Local(raw.to_string());
        }
    }
    match raw.split_once(':') {
        Some((name, path)) if !name.is_empty() && !name.contains('/') && !name.contains('\\') => {
            Target::Remote {
                profile_name: name.to_string(),
                path: path.to_string(),
            }
        }
        _ => Target::Local(raw.to_string()),
    }
}

async fn find_profile(store: &ProfileStore, name: &str) -> Result<ConnectionProfile> {
    let profiles = store.list().await?;
    let lowered = name.to_ascii_lowercase();
    if let Some(p) = profiles
        .iter()
        .find(|p| p.name.to_ascii_lowercase() == lowered || p.id == name)
    {
        return Ok(p.clone());
    }
    bail!(
        "no profile named `{name}`. List available with `faro-cli profiles list`."
    );
}

// ---- Session opening ---------------------------------------------------

/// Build a session for a remote target. Each invocation opens a fresh
/// connection — short-lived CLI scripts don't need a long-lived manager.
async fn open_session(profile: &ConnectionProfile) -> Result<Session> {
    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(CliHostKeyVerifier);
    faro_open_session(profile, verifier).await
}

fn fs_for(session: &Session) -> Box<dyn RemoteFs> {
    match session {
        Session::Ssh(ssh) => Box::new(faro_lib::remotefs::sftp::SftpFs::new(ssh.clone())),
        Session::Ftp(ftp) => Box::new(faro_lib::remotefs::ftp::FtpFs::new(ftp.clone())),
        Session::Object(obj) => {
            Box::new(faro_lib::remotefs::object::ObjectFs::new(obj.clone()))
        }
        Session::Webdav(dav) => Box::new(faro_lib::remotefs::webdav::WebdavFs::new(dav.clone())),
        Session::Http(http) => Box::new(faro_lib::remotefs::http::HttpFs::new(http.clone())),
        Session::Dropbox(dbx) => Box::new(faro_lib::remotefs::dropbox::DropboxFs::new(dbx.clone())),
        Session::OneDrive(od) => Box::new(faro_lib::remotefs::onedrive::OneDriveFs::new(od.clone())),
        Session::GDrive(gd) => Box::new(faro_lib::remotefs::gdrive::GDriveFs::new(gd.clone())),
        Session::Box(bx) => Box::new(faro_lib::remotefs::boxdrive::BoxFs::new(bx.clone())),
        Session::Agent(agent) => Box::new(faro_lib::remotefs::agent::AgentFs::new(agent.clone())),
    }
}

// ---- Host-key verifier for the CLI -------------------------------------

struct CliHostKeyVerifier;

#[async_trait]
impl HostKeyVerifier for CliHostKeyVerifier {
    async fn decide(
        &self,
        host: &str,
        port: u16,
        key_type: &str,
        fingerprint: &str,
        stored_fingerprint: Option<&str>,
        kind: HostPromptKind,
    ) -> Result<HostDecision, russh::Error> {
        match kind {
            HostPromptKind::Unknown => {
                eprintln!("\n{}", warn("Unknown host key:"));
                eprintln!("  host        {host}:{port}");
                eprintln!("  fingerprint {fingerprint} ({key_type})");
            }
            HostPromptKind::Mismatch => {
                eprintln!("\n{}", warn("Host key changed (possible MITM):"));
                eprintln!("  host        {host}:{port}");
                eprintln!("  server      {fingerprint} ({key_type})");
                if let Some(saved) = stored_fingerprint {
                    eprintln!("  saved       {saved}");
                }
            }
        }
        eprint!("Accept this key? [t]rust+save / [a]ccept once / [r]eject> ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return Ok(HostDecision::Reject);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "t" | "trust" | "y" | "yes" => Ok(HostDecision::Trust),
            "a" | "accept" | "o" | "once" => Ok(HostDecision::Accept),
            _ => Ok(HostDecision::Reject),
        }
    }
}

fn warn(s: &str) -> String {
    format!("\x1b[33m⚠ {s}\x1b[0m")
}

fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}

// ---- Subcommands -------------------------------------------------------

async fn cmd_ls(store: &ProfileStore, raw: &str, bytes: bool) -> Result<()> {
    let entries = match parse_target(raw) {
        Target::Local(p) => faro_lib::remotefs::local::LocalFs.list_dir(&p).await?,
        Target::Remote { profile_name, path } => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            let fs = fs_for(&session);
            fs.list_dir(&path).await?
        }
    };

    let mut entries = entries;
    entries.sort_by(|a, b| match (a.kind, b.kind) {
        (FileKind::Directory, FileKind::Directory) | (_, _) if a.kind == b.kind => {
            a.name.cmp(&b.name)
        }
        (FileKind::Directory, _) => std::cmp::Ordering::Less,
        (_, FileKind::Directory) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    for e in entries {
        print_entry(&e, bytes);
    }
    Ok(())
}

fn print_entry(e: &DirEntry, bytes: bool) {
    let kind = match e.kind {
        FileKind::Directory => "d",
        FileKind::Symlink => "l",
        FileKind::File => "-",
        FileKind::Other => "?",
    };
    let size = if e.kind == FileKind::Directory {
        String::new()
    } else if bytes {
        format!("{}", e.size)
    } else {
        fmt_bytes(e.size)
    };
    let mode = e
        .mode
        .map(|m| format!("{:o}", m & 0o777))
        .unwrap_or_else(|| "---".into());
    println!(
        "{kind} {mode:>4}  {size:>10}  {name}",
        name = e.name
    );
    let _ = e.modified;
}

async fn cmd_exec(store: &ProfileStore, profile_name: &str, command: &[String]) -> Result<()> {
    let profile = find_profile(store, profile_name).await?;
    let session = open_session(&profile).await?;
    let Session::Ssh(ssh) = session else {
        bail!(
            "`exec` needs an SSH/SFTP profile; `{}` is {}",
            profile.name,
            profile.protocol
        );
    };
    let cmd = command.join(" ");
    let out = ssh
        .exec(&cmd)
        .await
        .with_context(|| format!("exec `{cmd}`"))?;

    let mut so = io::stdout();
    so.write_all(out.stdout.as_bytes()).ok();
    so.flush().ok();
    if !out.stderr.is_empty() {
        let mut se = io::stderr();
        se.write_all(out.stderr.as_bytes()).ok();
        se.flush().ok();
    }
    // Propagate the remote command's exit code as our own.
    std::process::exit(out.exit_code.unwrap_or(0))
}

async fn cmd_cp(store: &ProfileStore, src: &str, dst: &str) -> Result<()> {
    let src_t = parse_target(src);
    let dst_t = parse_target(dst);

    match (src_t, dst_t) {
        // local → local
        (Target::Local(s), Target::Local(d)) => {
            let bytes = tokio::fs::copy(&s, &d)
                .await
                .with_context(|| format!("copy {s} -> {d}"))?;
            println!("Copied {} ({})", d, fmt_bytes(bytes));
        }
        // upload: local → remote
        (Target::Local(local_path), Target::Remote { profile_name, path: dest }) => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            let dest_parent = parent_of(&dest);
            let local_name = basename(&local_path);
            upload_file(&session, &local_path, &dest_parent, &local_name).await?;
        }
        // download: remote → local
        (Target::Remote { profile_name, path: src }, Target::Local(local_dir)) => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            download_file(&session, &src, &local_dir).await?;
        }
        // remote → remote: not supported across profiles in v1.1.
        (Target::Remote { .. }, Target::Remote { .. }) => {
            bail!("remote-to-remote copy is not yet supported; download then upload");
        }
    }
    Ok(())
}

async fn cmd_mv(store: &ProfileStore, src: &str, dst: &str) -> Result<()> {
    let src_t = parse_target(src);
    let dst_t = parse_target(dst);
    match (src_t, dst_t) {
        (Target::Local(s), Target::Local(d)) => {
            tokio::fs::rename(&s, &d)
                .await
                .with_context(|| format!("rename {s} -> {d}"))?;
        }
        (
            Target::Remote { profile_name: s_p, path: s_path },
            Target::Remote { profile_name: d_p, path: d_path },
        ) if s_p == d_p => {
            let profile = find_profile(store, &s_p).await?;
            let session = open_session(&profile).await?;
            fs_for(&session).rename(&s_path, &d_path).await?;
        }
        _ => bail!("mv must stay within a single profile or be fully local"),
    }
    Ok(())
}

async fn cmd_rm(store: &ProfileStore, raw: &str, recursive: bool) -> Result<()> {
    match parse_target(raw) {
        Target::Local(p) => {
            faro_lib::remotefs::local::LocalFs.delete(&p, recursive).await?;
        }
        Target::Remote { profile_name, path } => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            fs_for(&session).delete(&path, recursive).await?;
        }
    }
    Ok(())
}

async fn cmd_mkdir(store: &ProfileStore, raw: &str) -> Result<()> {
    match parse_target(raw) {
        Target::Local(p) => {
            faro_lib::remotefs::local::LocalFs.create_dir(&p).await?;
        }
        Target::Remote { profile_name, path } => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            fs_for(&session).create_dir(&path).await?;
        }
    }
    Ok(())
}

async fn cmd_sync(
    store: &ProfileStore,
    local: &str,
    remote: &str,
    direction: Dir,
    mirror: bool,
    dry_run: bool,
) -> Result<()> {
    let local_path = match parse_target(local) {
        Target::Local(p) => p,
        Target::Remote { .. } => bail!("sync's first argument must be a local path"),
    };
    let (profile_name, remote_path) = match parse_target(remote) {
        Target::Remote { profile_name, path } => (profile_name, path),
        Target::Local(_) => bail!("sync's second argument must be `profile:/path`"),
    };
    let profile = find_profile(store, &profile_name).await?;
    let session = open_session(&profile).await?;

    let local_fs: Box<dyn RemoteFs> = Box::new(faro_lib::remotefs::local::LocalFs);
    let remote_fs = fs_for(&session);
    let dir = match direction {
        Dir::Push => faro_lib::sync::SyncDirection::LocalToRemote,
        Dir::Pull => faro_lib::sync::SyncDirection::RemoteToLocal,
    };
    let strategy = if mirror {
        faro_lib::sync::SyncStrategy::Mirror
    } else {
        faro_lib::sync::SyncStrategy::Additive
    };

    let plan = faro_lib::sync::plan(
        local_fs.as_ref(),
        remote_fs.as_ref(),
        &local_path,
        &remote_path,
        dir,
        strategy,
    )
    .await?;

    eprintln!(
        "\n{}\n  copies   {}\n  deletes  {}\n  bytes    {}",
        dim(&format!(
            "Plan: {} → {} ({})",
            local_path,
            remote_path,
            if mirror { "mirror" } else { "additive" }
        )),
        plan.copies.len(),
        plan.deletes.len(),
        fmt_bytes(plan.total_bytes),
    );

    if dry_run {
        for c in plan.copies.iter().take(50) {
            eprintln!(
                "  {}  {}  {}",
                reason_label(&c.reason),
                fmt_bytes(c.size),
                c.relative
            );
        }
        if plan.copies.len() > 50 {
            eprintln!("  …and {} more", plan.copies.len() - 50);
        }
        return Ok(());
    }

    if plan.copies.is_empty() && plan.deletes.is_empty() {
        eprintln!("Already in sync.");
        return Ok(());
    }

    let bar = ProgressBar::new(plan.copies.len() as u64);
    bar.set_style(
        ProgressStyle::with_template("{bar:30.cyan/blue} {pos:>3}/{len:3} {msg}")
            .unwrap(),
    );
    for c in &plan.copies {
        bar.set_message(c.relative.clone());
        match direction {
            Dir::Push => {
                let dest_parent = parent_of(&c.destination_path);
                upload_file(&session, &c.source_path, &dest_parent, &basename(&c.source_path))
                    .await?;
            }
            Dir::Pull => {
                let dest_parent = parent_of(&c.destination_path);
                tokio::fs::create_dir_all(&dest_parent).await.ok();
                download_file(&session, &c.source_path, &dest_parent).await?;
            }
        }
        bar.inc(1);
    }
    bar.finish_and_clear();

    for d in &plan.deletes {
        let fs: &dyn RemoteFs = match direction {
            Dir::Push => remote_fs.as_ref(),
            Dir::Pull => local_fs.as_ref(),
        };
        if let Err(e) = fs.delete(&d.path, false).await {
            eprintln!("{} {}: {e}", warn("delete failed"), d.path);
        }
    }
    eprintln!("Sync complete.");
    Ok(())
}

// ---- Directory diff ----------------------------------------------------

/// One side of a diff: a local path, or an opened remote session + path. The
/// session is kept alive because `--hash` reads bytes through it after the walk.
enum DiffSide {
    Local(String),
    Remote { session: Session, path: String },
}

/// Parse a diff side. Same rules as `parse_target`, plus an explicit `local:`
/// prefix (the plan's documented syntax) that forces the local filesystem even
/// where a same-named profile exists.
fn parse_diff_target(raw: &str) -> Target {
    if let Some((name, path)) = raw.split_once(':') {
        if name.eq_ignore_ascii_case("local") {
            return Target::Local(path.to_string());
        }
    }
    parse_target(raw)
}

async fn resolve_diff_side(store: &ProfileStore, raw: &str) -> Result<DiffSide> {
    match parse_diff_target(raw) {
        Target::Local(p) => Ok(DiffSide::Local(p)),
        Target::Remote { profile_name, path } => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            Ok(DiffSide::Remote { session, path })
        }
    }
}

async fn cmd_diff(
    store: &ProfileStore,
    a: &str,
    b: &str,
    hash: bool,
    json: bool,
    all: bool,
) -> Result<()> {
    let side_a = resolve_diff_side(store, a).await?;
    let side_b = resolve_diff_side(store, b).await?;

    let fs_a: Box<dyn RemoteFs> = match &side_a {
        DiffSide::Local(_) => Box::new(faro_lib::remotefs::local::LocalFs),
        DiffSide::Remote { session, .. } => fs_for(session),
    };
    let fs_b: Box<dyn RemoteFs> = match &side_b {
        DiffSide::Local(_) => Box::new(faro_lib::remotefs::local::LocalFs),
        DiffSide::Remote { session, .. } => fs_for(session),
    };

    let (root_a, sess_a) = match &side_a {
        DiffSide::Local(p) => (p.as_str(), None),
        DiffSide::Remote { session, path } => (path.as_str(), Some(session)),
    };
    let (root_b, sess_b) = match &side_b {
        DiffSide::Local(p) => (p.as_str(), None),
        DiffSide::Remote { session, path } => (path.as_str(), Some(session)),
    };

    if !json {
        eprintln!(
            "{}",
            dim(&format!(
                "Diffing {} ↔ {}{}",
                root_a,
                root_b,
                if hash { " (hashing content)" } else { "" }
            ))
        );
    }

    let result = faro_lib::diff::diff(
        fs_a.as_ref(),
        root_a,
        sess_a,
        fs_b.as_ref(),
        root_b,
        sess_b,
        hash,
    )
    .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_diff_human(&result, all);
    }

    // Conventional diff exit code: 0 when identical, 1 when the trees differ.
    let s = &result.summary;
    let differ = s.only_in_a + s.only_in_b + s.different;
    std::process::exit(if differ == 0 { 0 } else { 1 })
}

fn print_diff_human(result: &faro_lib::diff::DiffResult, all: bool) {
    use faro_lib::diff::{DiffClass, DiffReason};

    let mut shown = 0usize;
    for e in &result.entries {
        if e.class == DiffClass::Same && !all {
            continue;
        }
        let (marker, detail) = match e.class {
            DiffClass::OnlyInA => (
                "\x1b[36mA only\x1b[0m",
                e.a_size.map(fmt_bytes).unwrap_or_default(),
            ),
            DiffClass::OnlyInB => (
                "\x1b[35mB only\x1b[0m",
                e.b_size.map(fmt_bytes).unwrap_or_default(),
            ),
            DiffClass::Different => {
                let why = match e.reason {
                    Some(DiffReason::Size) => format!(
                        "size {} → {}",
                        e.a_size.map(fmt_bytes).unwrap_or_default(),
                        e.b_size.map(fmt_bytes).unwrap_or_default()
                    ),
                    Some(DiffReason::Content) => "content".to_string(),
                    None => String::new(),
                };
                ("\x1b[33mdiffer\x1b[0m", why)
            }
            DiffClass::Same => ("\x1b[2msame\x1b[0m  ", String::new()),
        };
        let hash_note = if e.hash_error.is_some() {
            dim("  [hash unavailable]")
        } else {
            String::new()
        };
        if detail.is_empty() {
            println!("{marker}  {}{hash_note}", e.relative);
        } else {
            println!("{marker}  {}  {}{hash_note}", e.relative, dim(&detail));
        }
        shown += 1;
    }

    if shown == 0 {
        eprintln!("Trees are identical.");
    }

    let s = &result.summary;
    eprintln!(
        "\n{}",
        dim(&format!(
            "{} only in A · {} only in B · {} differ · {} same{}",
            s.only_in_a,
            s.only_in_b,
            s.different,
            s.same,
            if result.hashed { " (hashed)" } else { "" }
        ))
    );
}

// ---- Fleet search ------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn cmd_search(
    store: &ProfileStore,
    target: &str,
    pattern: &str,
    content: bool,
    regex: bool,
    case_sensitive: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    content_remote: bool,
    max: Option<usize>,
    json: bool,
) -> Result<()> {
    use faro_lib::search::{SearchKind, SearchQuery, DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_RESULTS};

    // Resolve the target to a RemoteFs + optional live session (local = None).
    let (fs, session, root): (Box<dyn RemoteFs>, Option<Session>, String) = match parse_target(target) {
        Target::Local(p) => (Box::new(faro_lib::remotefs::local::LocalFs), None, p),
        Target::Remote { profile_name, path } => {
            let profile = find_profile(store, &profile_name).await?;
            let session = open_session(&profile).await?;
            let fs = fs_for(&session);
            (fs, Some(session), path)
        }
    };
    let root = if root.trim().is_empty() { ".".to_string() } else { root };

    // `--regex` implies content search (a regex over names is unusual; the plan
    // scopes regex to content grep).
    let content = content || regex;
    let query = SearchQuery {
        pattern: pattern.to_string(),
        kind: if content { SearchKind::Content } else { SearchKind::Name },
        regex,
        case_sensitive,
        include_globs: include,
        exclude_globs: exclude,
        content_remote,
        max_results: max.unwrap_or(DEFAULT_MAX_RESULTS),
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
    };

    if !json {
        eprintln!(
            "{}",
            dim(&format!(
                "Searching {root} for {pattern:?}{}",
                if content { " (content)" } else { "" }
            ))
        );
    }

    let result = faro_lib::search::search(fs.as_ref(), session.as_ref(), &root, &query).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_search_human(&result);
    }

    // grep convention: 0 when something matched, 1 when nothing did.
    std::process::exit(if result.hits.is_empty() { 1 } else { 0 })
}

fn print_search_human(result: &faro_lib::search::SearchResult) {
    use faro_lib::search::{SearchKind, SearchStrategy};

    for h in &result.hits {
        match result.kind {
            SearchKind::Name => {
                if h.is_dir {
                    println!("\x1b[34m{}/\x1b[0m", h.relative);
                } else {
                    println!("{}  {}", h.relative, dim(&fmt_bytes(h.size)));
                }
            }
            SearchKind::Content => {
                let line = h.line.unwrap_or(0);
                let preview = h.preview.as_deref().unwrap_or("");
                println!(
                    "\x1b[36m{}\x1b[0m:\x1b[33m{}\x1b[0m: {}",
                    h.relative, line, preview
                );
            }
        }
    }

    if result.hits.is_empty() {
        eprintln!("No matches.");
    }

    let s = &result.stats;
    let strat = match s.strategy {
        SearchStrategy::Generic => "walk",
        SearchStrategy::Shell => "exec",
        SearchStrategy::ObjectFlat => "object",
    };
    let mut summary = format!("{} hits · {strat}", result.hits.len());
    if s.truncated {
        summary.push_str(" · capped (more matches exist)");
    }
    if let Some(note) = &s.note {
        summary.push_str(&format!(" · {note}"));
    }
    eprintln!("\n{}", dim(&summary));
}

async fn cmd_profiles_list(store: &ProfileStore) -> Result<()> {
    let profiles = store.list().await?;
    if profiles.is_empty() {
        eprintln!("No profiles saved. Open the GUI to add one.");
        return Ok(());
    }
    for p in profiles {
        let suffix = match p.protocol.as_str() {
            "s3" | "azure" => format!(
                "{}://{}",
                p.protocol,
                p.bucket.as_deref().unwrap_or("?")
            ),
            _ => format!("{}@{}:{}", p.username, p.host, p.port),
        };
        println!("{:<24} {:<6} {}", p.name, p.protocol, suffix);
    }
    Ok(())
}

async fn cmd_profiles_show(store: &ProfileStore, name: &str) -> Result<()> {
    let p = find_profile(store, name).await?;
    let mut json = serde_json::to_value(&p)?;
    // Redact the password / passphrase before printing.
    if let Some(obj) = json.as_object_mut() {
        if let Some(auth) = obj.get_mut("auth") {
            if let Some(auth_obj) = auth.as_object_mut() {
                if auth_obj.contains_key("password") {
                    auth_obj.insert("password".into(), serde_json::json!("<redacted>"));
                }
                if auth_obj.contains_key("passphrase") {
                    auth_obj.insert("passphrase".into(), serde_json::json!("<redacted>"));
                }
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

// ---- Agent Bridge client -----------------------------------------------
//
// `faro-cli agent …` talks to Faro's RUNNING Agent Bridge over localhost HTTP
// rather than opening its own connection. It reads the bridge URL + bearer
// token from the discovery file Faro writes while the bridge is on, so neither
// the user nor an AI agent ever handles a URL or token — and every call still
// goes through Faro's per-command approval and live console.

struct Endpoint {
    url: String,
    token: String,
}

/// Read the bridge URL + token from Faro's discovery file (same data dir the
/// GUI uses). A missing file means the bridge isn't running.
fn read_endpoint() -> Result<Endpoint> {
    let path = default_data_dir()?.join("agent-endpoint.json");
    let bytes = std::fs::read(&path).map_err(|_| {
        anyhow!(
            "Faro's Agent Bridge isn't running. Open Faro and turn on the Agent \
             Bridge (the master switch at the top of the Bridge panel)."
        )
    })?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .context("agent-endpoint.json is corrupt; toggle the Agent Bridge off and on in Faro")?;
    let url = v
        .get("url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("agent-endpoint.json is missing `url`"))?
        .to_string();
    let token = v
        .get("token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("agent-endpoint.json is missing `token`"))?
        .to_string();
    Ok(Endpoint { url, token })
}

/// POST a JSON body to a bridge route. The 180s timeout comfortably exceeds the
/// bridge's 120s approval window, so a call that's blocked waiting for the user
/// to click Approve in Faro doesn't time out on our side first.
fn http_post(ep: &Endpoint, route: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build();
    match agent
        .post(&format!("{}{}", ep.url, route))
        .set("Authorization", &format!("Bearer {}", ep.token))
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(r) => Ok(r.into_json::<serde_json::Value>()?),
        Err(ureq::Error::Status(code, r)) => bail!("Agent Bridge error ({code}): {}", err_text(r, code)),
        Err(e) => bail!(
            "couldn't reach Faro's Agent Bridge at {} ({e}). Is Faro still running with the bridge on?",
            ep.url
        ),
    }
}

fn http_get(ep: &Endpoint, route: &str) -> Result<serde_json::Value> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        .build();
    match agent
        .get(&format!("{}{}", ep.url, route))
        .set("Authorization", &format!("Bearer {}", ep.token))
        .call()
    {
        Ok(r) => Ok(r.into_json::<serde_json::Value>()?),
        Err(ureq::Error::Status(code, r)) => bail!("Agent Bridge error ({code}): {}", err_text(r, code)),
        Err(e) => bail!(
            "couldn't reach Faro's Agent Bridge at {} ({e}). Is Faro still running with the bridge on?",
            ep.url
        ),
    }
}

/// Pull the bridge's `{error}` text out of a non-2xx response (falls back to the
/// status code).
fn err_text(r: ureq::Response, code: u16) -> String {
    r.into_json::<serde_json::Value>()
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_else(|| format!("HTTP {code}"))
}

/// Map a friendly server name to a bridge session id (the REST routes need the
/// exact id). Mirrors the bridge's own MCP name resolution: exact id wins, then
/// a unique case-insensitive name; an ambiguous name errors rather than guesses.
fn resolve_server(ep: &Endpoint, name: &str) -> Result<String> {
    let body = http_get(ep, "/sessions")?;
    let sessions = body
        .get("sessions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if sessions.is_empty() {
        bail!("no server has granted agent access yet — enable one in Faro's Agent Bridge panel");
    }
    if sessions
        .iter()
        .any(|s| s.get("id").and_then(|v| v.as_str()) == Some(name))
    {
        return Ok(name.to_string());
    }
    let matches: Vec<&serde_json::Value> = sessions
        .iter()
        .filter(|s| {
            s.get("name")
                .and_then(|v| v.as_str())
                .map(|n| n.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        })
        .collect();
    match matches.len() {
        1 => Ok(matches[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()),
        0 => {
            let names: Vec<&str> = sessions
                .iter()
                .filter_map(|s| s.get("name").and_then(|v| v.as_str()))
                .collect();
            bail!("no enabled server matches \"{name}\". Available: {}", names.join(", "))
        }
        _ => bail!(
            "\"{name}\" matches more than one connected server; pass the exact session id (see `faro-cli agent sessions`)"
        ),
    }
}

fn cmd_agent(action: AgentCmd) -> Result<()> {
    let ep = read_endpoint()?;
    match action {
        AgentCmd::Context => {
            let body = http_get(&ep, "/context")?;
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            Ok(())
        }
        AgentCmd::Sessions => {
            let body = http_get(&ep, "/sessions")?;
            let sessions = body
                .get("sessions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if sessions.is_empty() {
                eprintln!(
                    "No servers have granted agent access. Enable one in Faro's Agent Bridge panel."
                );
            }
            for s in &sessions {
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let proto = s.get("protocol").and_then(|v| v.as_str()).unwrap_or("?");
                let host = s.get("host").and_then(|v| v.as_str()).unwrap_or("");
                let can_exec = s.get("canExec").and_then(|v| v.as_bool()).unwrap_or(false);
                println!("{name:<24} {proto:<6} {host:<28} exec={can_exec}");
            }
            Ok(())
        }
        AgentCmd::Exec { server, dry_run, timeout_ms, command } => {
            let id = resolve_server(&ep, &server)?;
            let mut req =
                serde_json::json!({ "sessionId": id, "command": command.join(" "), "dryRun": dry_run });
            if let Some(t) = timeout_ms {
                req["timeoutMs"] = serde_json::json!(t);
            }
            let body = http_post(&ep, "/exec", req)?;
            if let Some(out) = body.get("stdout").and_then(|v| v.as_str()) {
                let mut so = io::stdout();
                so.write_all(out.as_bytes()).ok();
                so.flush().ok();
            }
            if let Some(e) = body.get("stderr").and_then(|v| v.as_str()) {
                if !e.is_empty() {
                    let mut se = io::stderr();
                    se.write_all(e.as_bytes()).ok();
                    se.flush().ok();
                }
            }
            if body.get("timedOut").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("{}", warn("command timed out before finishing"));
            }
            // exitCode is a number or null on the wire; null => 0, like cmd_exec.
            let code = body.get("exitCode").and_then(|v| v.as_i64()).unwrap_or(0);
            std::process::exit(code as i32)
        }
        AgentCmd::Ls { server, path } => {
            let id = resolve_server(&ep, &server)?;
            let mut req = serde_json::json!({ "sessionId": id });
            if let Some(p) = path {
                req["path"] = serde_json::Value::String(p);
            }
            let body = http_post(&ep, "/list", req)?;
            print_agent_entries(&body);
            Ok(())
        }
        AgentCmd::Read { server, path } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(&ep, "/read", serde_json::json!({ "sessionId": id, "path": path }))?;
            if let Some(content) = body.get("content").and_then(|v| v.as_str()) {
                let mut so = io::stdout();
                so.write_all(content.as_bytes()).ok();
                so.flush().ok();
            }
            if body.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("\n{}", warn("output truncated at 256 KiB"));
            }
            Ok(())
        }
        AgentCmd::ReadBatch { server, paths } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(
                &ep,
                "/read_batch",
                serde_json::json!({ "sessionId": id, "paths": paths }),
            )?;
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            Ok(())
        }
        AgentCmd::Glob { server, path, pattern } => {
            let id = resolve_server(&ep, &server)?;
            let mut req = serde_json::json!({ "sessionId": id, "pattern": pattern });
            if let Some(p) = path {
                req["path"] = serde_json::Value::String(p);
            }
            let body = http_post(&ep, "/glob", req)?;
            if let Some(matches) = body.get("matches").and_then(|v| v.as_array()) {
                for m in matches {
                    if let Some(p) = m.as_str() {
                        println!("{p}");
                    }
                }
            }
            Ok(())
        }
        AgentCmd::Search { server, query, path } => {
            let id = resolve_server(&ep, &server)?;
            let mut req = serde_json::json!({ "sessionId": id, "query": query });
            if let Some(p) = path {
                req["path"] = serde_json::Value::String(p);
            }
            let body = http_post(&ep, "/search", req)?;
            if let Some(matches) = body.get("matches").and_then(|v| v.as_array()) {
                for m in matches {
                    if let Some(p) = m.get("path").and_then(|v| v.as_str()) {
                        println!("{p}");
                    }
                }
            }
            if body.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("{}", warn("results truncated"));
            }
            Ok(())
        }
        AgentCmd::Download { server, remote_path, local_dir } => {
            let id = resolve_server(&ep, &server)?;
            let mut req = serde_json::json!({ "sessionId": id, "path": remote_path });
            if let Some(d) = local_dir {
                req["localDir"] = serde_json::Value::String(d);
            }
            let body = http_post(&ep, "/download", req)?;
            print_transfer_started(&body);
            Ok(())
        }
        AgentCmd::Upload { server, local_path, remote_dir } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(
                &ep,
                "/upload",
                serde_json::json!({ "sessionId": id, "localPath": local_path, "remoteDir": remote_dir }),
            )?;
            print_transfer_started(&body);
            Ok(())
        }
        AgentCmd::UploadDir { server, local_dir, remote_dir, overwrite } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(
                &ep,
                "/upload_dir",
                serde_json::json!({
                    "sessionId": id,
                    "localDir": local_dir,
                    "remoteDir": remote_dir,
                    "overwrite": overwrite,
                }),
            )?;
            let ids = body
                .get("transferIds")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let total = body.get("totalBytes").and_then(|v| v.as_u64()).unwrap_or(0);
            let root = body.get("remoteRoot").and_then(|v| v.as_str()).unwrap_or("?");
            println!(
                "queued {} file uploads ({}) → {}",
                ids.len(),
                fmt_bytes(total),
                root
            );
            if let Some(first) = ids.first().and_then(|v| v.as_str()) {
                println!("poll with: faro-cli agent transfer {first}");
            }
            Ok(())
        }
        AgentCmd::Sync { server, local_dir, remote_dir, direction, mirror, dry_run } => {
            let id = resolve_server(&ep, &server)?;
            let dir_word = match direction {
                Dir::Push => "push",
                Dir::Pull => "pull",
            };
            let strategy = if mirror { "mirror" } else { "additive" };
            let body = http_post(
                &ep,
                "/sync",
                serde_json::json!({
                    "sessionId": id,
                    "localDir": local_dir,
                    "remoteDir": remote_dir,
                    "direction": dir_word,
                    "strategy": strategy,
                    "dryRun": dry_run,
                }),
            )?;
            let copies = body.get("copyCount").and_then(|v| v.as_u64()).unwrap_or(0);
            let deletes = body.get("deleteCount").and_then(|v| v.as_u64()).unwrap_or(0);
            let bytes = body.get("totalBytes").and_then(|v| v.as_u64()).unwrap_or(0);
            eprintln!(
                "\n{}\n  copies   {}\n  deletes  {}\n  bytes    {}",
                dim(&format!(
                    "Plan: {local_dir} {} {remote_dir} ({strategy}, {dir_word})",
                    if dry_run { "↔" } else { "→" }
                )),
                copies,
                deletes,
                fmt_bytes(bytes),
            );
            if dry_run {
                if let Some(arr) = body.get("copies").and_then(|v| v.as_array()) {
                    for c in arr.iter().take(50) {
                        let rel = c.get("relative").and_then(|v| v.as_str()).unwrap_or("");
                        let size = c.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                        let reason = c.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                        eprintln!("  {reason:<11}  {:>10}  {rel}", fmt_bytes(size));
                    }
                    if arr.len() > 50 {
                        eprintln!("  …and {} more copies", arr.len() - 50);
                    }
                }
                if let Some(arr) = body.get("deletes").and_then(|v| v.as_array()) {
                    for d in arr.iter().take(50) {
                        let rel = d.get("relative").and_then(|v| v.as_str()).unwrap_or("");
                        eprintln!("  {}       {rel}", warn("delete"));
                    }
                    if arr.len() > 50 {
                        eprintln!("  …and {} more deletes", arr.len() - 50);
                    }
                }
                return Ok(());
            }
            if body.get("status").and_then(|v| v.as_str()) == Some("in-sync") {
                eprintln!("Already in sync.");
                return Ok(());
            }
            let ids = body
                .get("transferIds")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            println!("queued {} transfers; deletes applied", ids.len());
            if let Some(first) = ids.first().and_then(|v| v.as_str()) {
                println!("poll with: faro-cli agent transfer {first}");
            }
            Ok(())
        }
        AgentCmd::Diff { a, b, hash, json } => {
            let (server_a, path_a) = split_agent_diff_side(&a);
            let (server_b, path_b) = split_agent_diff_side(&b);
            let mut req = serde_json::json!({ "pathA": path_a, "pathB": path_b, "hash": hash });
            if let Some(s) = server_a {
                req["sessionA"] = serde_json::Value::String(s);
            }
            if let Some(s) = server_b {
                req["sessionB"] = serde_json::Value::String(s);
            }
            let body = http_post(&ep, "/diff", req)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            } else {
                print_agent_diff(&body);
            }
            Ok(())
        }
        AgentCmd::Transfer { transfer_id } => {
            let body = http_post(&ep, "/transfer", serde_json::json!({ "transferId": transfer_id }))?;
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            Ok(())
        }
        AgentCmd::Tail { server, path, lines } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(
                &ep,
                "/tail",
                serde_json::json!({ "sessionId": id, "path": path, "lines": lines }),
            )?;
            if let Some(out) = body.get("stdout").and_then(|v| v.as_str()) {
                let mut so = io::stdout();
                so.write_all(out.as_bytes()).ok();
                so.flush().ok();
            }
            if body.get("timedOut").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("{}", warn("tail stream timed out after 30s"));
            }
            Ok(())
        }
        AgentCmd::Info { server } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(&ep, "/info", serde_json::json!({ "sessionId": id }))?;
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            Ok(())
        }
        AgentCmd::History { server, limit } => {
            let mut req = serde_json::json!({ "limit": limit });
            if let Some(s) = server {
                let id = resolve_server(&ep, &s)?;
                req["sessionId"] = serde_json::Value::String(id);
            }
            let body = http_post(&ep, "/history", req)?;
            if let Some(arr) = body.get("history").and_then(|v| v.as_array()) {
                for h in arr {
                    let kind = h.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                    let ok = h.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let detail = h.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                    let mark = if ok { "ok " } else { "ERR" };
                    println!("[{mark}] {kind:<8} {detail}");
                }
            }
            Ok(())
        }
        AgentCmd::Run { server, dry_run, name } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(&ep, "/run", serde_json::json!({ "sessionId": id, "name": name, "dryRun": dry_run }))?;
            if let Some(out) = body.get("stdout").and_then(|v| v.as_str()) {
                let mut so = io::stdout();
                so.write_all(out.as_bytes()).ok();
                so.flush().ok();
            }
            if let Some(e) = body.get("stderr").and_then(|v| v.as_str()) {
                if !e.is_empty() {
                    let mut se = io::stderr();
                    se.write_all(e.as_bytes()).ok();
                    se.flush().ok();
                }
            }
            if body.get("timedOut").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("{}", warn("command timed out before finishing"));
            }
            let code = body.get("exitCode").and_then(|v| v.as_i64()).unwrap_or(0);
            std::process::exit(code as i32)
        }
        AgentCmd::Commands => {
            let body = http_get(&ep, "/commands")?;
            let cmds = body
                .get("commands")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if cmds.is_empty() {
                eprintln!("No saved commands yet. Add some in Faro's Agent Bridge panel.");
            }
            for c in &cmds {
                let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let command = c.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let desc = c.get("description").and_then(|v| v.as_str()).unwrap_or("");
                if desc.is_empty() {
                    println!("{name:<20} {command}");
                } else {
                    println!("{name:<20} {command}");
                    println!("{:<20} — {desc}", "");
                }
            }
            Ok(())
        }
    }
}

fn print_agent_entries(body: &serde_json::Value) {
    let Some(entries) = body.get("entries").and_then(|v| v.as_array()) else {
        return;
    };
    for e in entries {
        let name = e.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let kind = e
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        if kind.contains("dir") {
            println!("{name}/");
        } else {
            let size = e.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("{name}\t{}", fmt_bytes(size));
        }
    }
}

/// Split a `agent diff` side into (server, path). `local:/x` and a plain path
/// (or a `C:\…` drive) mean the local filesystem (server = None); `name:/x`
/// targets a connected server. Mirrors the bridge, which treats an absent
/// sessionA/B as local.
fn split_agent_diff_side(raw: &str) -> (Option<String>, String) {
    if let Some((name, path)) = raw.split_once(':') {
        if name.eq_ignore_ascii_case("local") {
            return (None, path.to_string());
        }
        // A Windows drive letter (`C:\…`) is a local path, not a server.
        if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
            return (None, raw.to_string());
        }
        return (Some(name.to_string()), path.to_string());
    }
    (None, raw.to_string())
}

/// Render the Agent Bridge `/diff` response (only differing entries are sent;
/// the summary counts the rest).
fn print_agent_diff(body: &serde_json::Value) {
    let entries = body.get("entries").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for e in &entries {
        let rel = e.get("relative").and_then(|v| v.as_str()).unwrap_or("");
        let class = e.get("class").and_then(|v| v.as_str()).unwrap_or("");
        let a_size = e.get("aSize").and_then(|v| v.as_u64());
        let b_size = e.get("bSize").and_then(|v| v.as_u64());
        let (marker, detail) = match class {
            "onlyInA" => ("\x1b[36mA only\x1b[0m", a_size.map(fmt_bytes).unwrap_or_default()),
            "onlyInB" => ("\x1b[35mB only\x1b[0m", b_size.map(fmt_bytes).unwrap_or_default()),
            "different" => {
                let why = match e.get("reason").and_then(|v| v.as_str()) {
                    Some("size") => format!(
                        "size {} → {}",
                        a_size.map(fmt_bytes).unwrap_or_default(),
                        b_size.map(fmt_bytes).unwrap_or_default()
                    ),
                    Some("content") => "content".to_string(),
                    _ => String::new(),
                };
                ("\x1b[33mdiffer\x1b[0m", why)
            }
            _ => ("?", String::new()),
        };
        let hash_note = if e.get("hashError").is_some() {
            dim("  [hash unavailable]")
        } else {
            String::new()
        };
        if detail.is_empty() {
            println!("{marker}  {rel}{hash_note}");
        } else {
            println!("{marker}  {rel}  {}{hash_note}", dim(&detail));
        }
    }
    if let Some(s) = body.get("summary") {
        let g = |k: &str| s.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let hashed = body.get("hashed").and_then(|v| v.as_bool()).unwrap_or(false);
        eprintln!(
            "\n{}",
            dim(&format!(
                "{} only in A · {} only in B · {} differ · {} same{}",
                g("onlyInA"),
                g("onlyInB"),
                g("different"),
                g("same"),
                if hashed { " (hashed)" } else { "" }
            ))
        );
    }
    if body.get("listTruncated").and_then(|v| v.as_bool()).unwrap_or(false) {
        eprintln!("{}", warn("entry list truncated — use --json for the full result"));
    }
}

fn print_transfer_started(body: &serde_json::Value) {
    let id = body.get("transferId").and_then(|v| v.as_str()).unwrap_or("?");
    println!("transfer started: {id}");
    if let Some(dir) = body.get("localDir").and_then(|v| v.as_str()) {
        println!("  → {dir}");
    }
    println!("poll with: faro-cli agent transfer {id}");
}

// ---- Transfer helpers --------------------------------------------------

/// Upload `local_path` into `remote_parent`, naming the result `name`. Uses
/// each backend's native streaming path so we don't pull the whole file into
/// memory.
async fn upload_file(
    session: &Session,
    local_path: &str,
    remote_parent: &str,
    name: &str,
) -> Result<()> {
    let local_size = tokio::fs::metadata(local_path)
        .await
        .with_context(|| format!("stat {local_path}"))?
        .len();
    let remote_path = join_remote(remote_parent, name);
    let bar = make_bar(local_size, name);

    match session {
        Session::Ssh(ssh) => {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let cell = ssh.ensure_sftp().await?;
            let sftp = cell.lock().await;
            let mut remote = sftp
                .create(&remote_path)
                .await
                .with_context(|| format!("create remote {remote_path}"))?;
            let mut local = tokio::fs::File::open(local_path).await?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = local.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                remote.write_all(&buf[..n]).await?;
                bar.inc(n as u64);
            }
            remote.flush().await?;
        }
        Session::Ftp(ftp) => upload_ftp(ftp.clone(), local_path.to_string(), remote_path.clone()).await?,
        Session::Object(obj) => {
            upload_object(obj.clone(), local_path, &remote_path, &bar).await?
        }
        Session::Webdav(_)
        | Session::Http(_)
        | Session::Dropbox(_)
        | Session::OneDrive(_)
        | Session::GDrive(_)
        | Session::Box(_) => {
            anyhow::bail!("uploads to this connection type are not yet supported in the CLI")
        }
        Session::Agent(_) => anyhow::bail!("faro-agent connections are not supported in the CLI"),
    }

    bar.finish_with_message(format!("uploaded {name}"));
    Ok(())
}

async fn upload_ftp(
    ftp: Arc<FtpSession>,
    local_path: String,
    remote_path: String,
) -> Result<()> {
    ftp.with_stream(move |stream| {
        let file = std::fs::File::open(&local_path)
            .with_context(|| format!("open {local_path}"))?;
        let mut reader = std::io::BufReader::new(file);
        stream.put_from_reader(&remote_path, &mut reader)?;
        Ok(())
    })
    .await
}

async fn upload_object(
    obj: Arc<ObjectSession>,
    local_path: &str,
    remote_path: &str,
    bar: &ProgressBar,
) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let key = remote_path.trim_start_matches('/');
    let p = object_store::path::Path::from(key);
    let mut file = tokio::fs::File::open(local_path).await?;
    let meta = file.metadata().await?;
    if meta.len() <= 16 * 1024 * 1024 {
        let mut buf = Vec::with_capacity(meta.len() as usize);
        file.read_to_end(&mut buf).await?;
        obj.store
            .put(&p, bytes::Bytes::from(buf).into())
            .await
            .with_context(|| format!("s3 put {key}"))?;
        bar.inc(meta.len());
    } else {
        let mut upload = obj.store.put_multipart(&p).await?;
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        loop {
            let mut filled = 0;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..]).await?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            let chunk = bytes::Bytes::copy_from_slice(&buf[..filled]);
            upload.put_part(chunk.into()).await?;
            bar.inc(filled as u64);
            if filled < buf.len() {
                break;
            }
        }
        upload.complete().await?;
    }
    Ok(())
}

async fn download_file(session: &Session, remote_path: &str, local_dir: &str) -> Result<()> {
    let local_dir_path = PathBuf::from(local_dir);
    tokio::fs::create_dir_all(&local_dir_path).await.ok();
    let name = basename(remote_path);
    let final_path = local_dir_path.join(&name);

    let bar = make_bar(0, &name);

    match session {
        Session::Ssh(ssh) => {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let cell = ssh.ensure_sftp().await?;
            let sftp = cell.lock().await;
            let mut remote = sftp
                .open(remote_path)
                .await
                .with_context(|| format!("open {remote_path}"))?;
            let mut local = tokio::fs::File::create(&final_path).await?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = remote.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                local.write_all(&buf[..n]).await?;
                bar.inc(n as u64);
            }
            local.flush().await?;
        }
        Session::Ftp(ftp) => {
            let ftp = ftp.clone();
            let remote = remote_path.to_string();
            let final_path_clone = final_path.clone();
            ftp.with_stream(move |stream| {
                let file = std::fs::File::create(&final_path_clone)
                    .with_context(|| format!("create {}", final_path_clone.display()))?;
                stream.retr_to_writer(&remote, std::io::BufWriter::new(file))?;
                Ok(())
            })
            .await?;
        }
        Session::Object(obj) => {
            use tokio::io::AsyncWriteExt;
            let key = remote_path.trim_start_matches('/');
            let p = object_store::path::Path::from(key);
            let get = obj.store.get(&p).await?;
            let mut file = tokio::fs::File::create(&final_path).await?;
            let mut stream = get.into_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
                bar.inc(chunk.len() as u64);
            }
            file.flush().await?;
        }
        Session::Webdav(_)
        | Session::Http(_)
        | Session::Dropbox(_)
        | Session::OneDrive(_)
        | Session::GDrive(_)
        | Session::Box(_) => {
            anyhow::bail!("downloads from this connection type are not yet supported in the CLI")
        }
        Session::Agent(_) => anyhow::bail!("faro-agent connections are not supported in the CLI"),
    }

    bar.finish_with_message(format!("downloaded {name}"));
    Ok(())
}

// ---- Misc helpers ------------------------------------------------------

fn make_bar(total: u64, name: &str) -> ProgressBar {
    let bar = if total > 0 {
        let b = ProgressBar::new(total);
        b.set_style(
            ProgressStyle::with_template(
                "{bar:30.cyan/blue} {bytes:>10} / {total_bytes:>10}  {msg}",
            )
            .unwrap(),
        );
        b
    } else {
        let b = ProgressBar::new_spinner();
        b.set_style(ProgressStyle::with_template("{spinner} {bytes:>10}  {msg}").unwrap());
        b
    };
    bar.set_message(name.to_string());
    bar
}

fn basename(path: &str) -> String {
    path.rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(path)
        .to_string()
}

fn parent_of(path: &str) -> String {
    match path.rfind(|c: char| c == '/' || c == '\\') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

fn join_remote(parent: &str, name: &str) -> String {
    if parent.ends_with('/') || parent.ends_with('\\') {
        format!("{parent}{name}")
    } else if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn reason_label(r: &faro_lib::sync::SyncReason) -> &'static str {
    match r {
        faro_lib::sync::SyncReason::Missing => "new   ",
        faro_lib::sync::SyncReason::Newer => "newer ",
        faro_lib::sync::SyncReason::SizeChanged => "size  ",
        faro_lib::sync::SyncReason::Edited => "edited",
    }
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
