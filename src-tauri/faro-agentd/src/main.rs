//! `faro-agentd` CLI.
//!
//! Usage:
//!   faro-agentd run    [--port N] [--read-only] [--no-mdns]   serve paired controllers
//!   faro-agentd pair   [--port N]                             open a pairing window, print a code
//!   faro-agentd info                                          print this machine's identity + peers
//!   faro-agentd unpair <fingerprint|all>                      remove a pinned controller
//!
//! Shared flags: --config-dir <path>

use anyhow::{bail, Context, Result};
use faro_agent_proto::{identity::Identity, pairing};
use faro_agentd::{
    config::{config_path, identity_path},
    discovery::Advertisement,
    ops, server, Config, Daemon,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

const DEFAULT_PORT: u16 = 8722;

struct Args {
    command: String,
    port: u16,
    config_dir: Option<PathBuf>,
    read_only: bool,
    no_mdns: bool,
    positional: Vec<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        command: "run".into(),
        port: DEFAULT_PORT,
        config_dir: None,
        read_only: false,
        no_mdns: false,
        positional: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    let mut first = true;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                if let Some(v) = it.next() {
                    a.port = v.parse().unwrap_or(DEFAULT_PORT);
                }
            }
            "--config-dir" => a.config_dir = it.next().map(PathBuf::from),
            "--read-only" => a.read_only = true,
            "--no-mdns" => a.no_mdns = true,
            "-h" | "--help" => {
                a.command = "help".into();
            }
            other => {
                if first && !other.starts_with('-') {
                    a.command = other.to_string();
                } else {
                    a.positional.push(other.to_string());
                }
            }
        }
        first = false;
    }
    a
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "faro_agentd=info".into()),
        )
        .init();

    let args = parse_args();
    let dir = faro_agentd::config_dir(args.config_dir.clone())?;
    let identity = Identity::load_or_create(&identity_path(&dir))?;
    let mut config = Config::load(&config_path(&dir))?;
    if args.read_only {
        config.policy.allow_exec = false;
        config.policy.allow_write = false;
    }

    match args.command.as_str() {
        "run" => run(args, dir, identity, config).await,
        "pair" => pair(args, dir, identity, config).await,
        "info" => info(dir, identity, config),
        "unpair" => unpair(dir, config, &args.positional),
        "help" => {
            print_help();
            Ok(())
        }
        other => {
            eprintln!("unknown command '{other}'\n");
            print_help();
            bail!("unknown command");
        }
    }
}

async fn run(args: Args, dir: PathBuf, identity: Identity, config: Config) -> Result<()> {
    let info = ops::system_info();
    let fingerprint = identity.fingerprint();
    let peers = config.peers.len();
    let daemon = Daemon::new(identity, config, dir);

    let listener = TcpListener::bind(("0.0.0.0", args.port))
        .await
        .with_context(|| format!("bind 0.0.0.0:{}", args.port))?;
    let port = listener.local_addr()?.port();

    println!("faro-agentd {} on {} ({})", env!("CARGO_PKG_VERSION"), info.hostname, info.os);
    println!("  identity : {fingerprint}");
    println!("  listening: 0.0.0.0:{port}");
    println!("  peers    : {peers} paired");
    if peers == 0 {
        println!("\n  No controllers paired yet. Run `faro-agentd pair` to add one.");
    }

    let _ad = if args.no_mdns {
        None
    } else {
        match Advertisement::publish(port, &fingerprint, &info.os) {
            Ok(ad) => {
                println!("  mDNS     : advertising _faro-agent._tcp");
                Some(ad)
            }
            Err(e) => {
                println!("  mDNS     : unavailable ({e}) — add this machine by IP in Faro");
                None
            }
        }
    };

    server::serve(listener, daemon).await
}

async fn pair(args: Args, dir: PathBuf, identity: Identity, config: Config) -> Result<()> {
    let info = ops::system_info();
    let fingerprint = identity.fingerprint();
    let daemon = Daemon::new(identity, config, dir);
    let code = pairing::generate_code();

    let listener = TcpListener::bind(("0.0.0.0", args.port))
        .await
        .with_context(|| format!("bind 0.0.0.0:{}", args.port))?;
    let port = listener.local_addr()?.port();

    println!("\n  Pairing {} ({})", info.hostname, info.os);
    println!("  ┌─────────────────────────┐");
    println!("  │   Pairing code: {code}   │");
    println!("  └─────────────────────────┘");
    println!("  fingerprint: {fingerprint}");
    println!("  port       : {port}");
    println!("\n  In Faro: New Connection → Faro Agent → enter this code.");
    println!("  Waiting for controllers to pair (Ctrl-C when done)…\n");

    let _ad = if args.no_mdns {
        None
    } else {
        Advertisement::publish(port, &fingerprint, &info.os).ok()
    };

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = counter.clone();
    server::serve_pairing(listener, daemon, code, move |name, key| {
        let n = c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        println!("  ✓ paired '{name}'  ({key})  [{n} total]");
    })
    .await
}

fn info(dir: PathBuf, identity: Identity, config: Config) -> Result<()> {
    let info = ops::system_info();
    println!("faro-agentd {}", env!("CARGO_PKG_VERSION"));
    println!("  machine    : {} ({}, {})", info.hostname, info.os, info.arch);
    println!("  config dir : {}", dir.display());
    println!("  identity   : {}", identity.fingerprint());
    println!("  public key : {}", identity.public_b64());
    println!(
        "  policy     : exec={}  write={}",
        config.policy.allow_exec, config.policy.allow_write
    );
    println!("  peers      : {}", config.peers.len());
    for p in &config.peers {
        let fp = faro_agent_proto::identity::decode_public(&p.public_key)
            .map(|k| faro_agent_proto::identity::fingerprint_of(&k))
            .unwrap_or_else(|_| "?".into());
        println!("    - {}  {fp}", p.name);
    }
    Ok(())
}

fn unpair(dir: PathBuf, mut config: Config, positional: &[String]) -> Result<()> {
    let Some(target) = positional.first() else {
        bail!("usage: faro-agentd unpair <fingerprint|all>");
    };
    let before = config.peers.len();
    if target == "all" {
        config.peers.clear();
    } else {
        config.peers.retain(|p| {
            let fp = faro_agent_proto::identity::decode_public(&p.public_key)
                .map(|k| faro_agent_proto::identity::fingerprint_of(&k))
                .unwrap_or_default();
            fp != *target && p.public_key != *target
        });
    }
    let removed = before - config.peers.len();
    config.save(&config_path(&dir))?;
    println!("Removed {removed} peer(s). {} remain.", config.peers.len());
    Ok(())
}

fn print_help() {
    println!(
        "faro-agentd {} — control this machine from Faro over an encrypted, paired link\n\n\
         USAGE:\n\
         \x20 faro-agentd run    [--port N] [--read-only] [--no-mdns]   serve paired controllers\n\
         \x20 faro-agentd pair   [--port N] [--no-mdns]                 open a pairing window, print a code\n\
         \x20 faro-agentd info                                          show identity + paired peers\n\
         \x20 faro-agentd unpair <fingerprint|all>                      remove a pinned controller\n\n\
         SHARED FLAGS:\n\
         \x20 --config-dir <path>   override the config/identity directory\n\n\
         Default port: {}",
        env!("CARGO_PKG_VERSION"),
        DEFAULT_PORT
    );
}
