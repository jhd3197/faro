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
    /// List the servers the Agent Bridge can currently reach.
    Sessions,
    /// Run a shell command on a server (SSH only). Exits with its exit code.
    Exec {
        /// Saved server name (as shown in Faro) or its session id.
        server: String,
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
    /// Check a transfer started via `agent download` / `agent upload`.
    Transfer { transfer_id: String },
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
        AgentCmd::Exec { server, command } => {
            let id = resolve_server(&ep, &server)?;
            let body = http_post(
                &ep,
                "/exec",
                serde_json::json!({ "sessionId": id, "command": command.join(" ") }),
            )?;
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
        AgentCmd::Transfer { transfer_id } => {
            let body = http_post(&ep, "/transfer", serde_json::json!({ "transferId": transfer_id }))?;
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
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
