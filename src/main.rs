//! DDNSOnDemand entry point.

mod cli;
mod config;
mod engine;
mod ip;
mod logging;
mod notify;
mod provider;
mod run;
mod state;
mod template;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use directories::ProjectDirs;
use tracing::{error, info};

use cli::{Cli, Command};

/// Default User-Agent for all outbound HTTP requests. Some IP/echo services and
/// CDNs reject requests without one (e.g. responding 403), so we always send it.
pub const USER_AGENT: &str = concat!("ddnsondemand/", env!("CARGO_PKG_VERSION"));

/// Current UTC timestamp as an RFC 3339 string (used in state and templates).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A DDNS client is overwhelmingly I/O-bound, so a single-threaded runtime
/// keeps the thread count (and memory) minimal.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = real_main().await {
        // Logging may not be initialised yet, so also print to stderr.
        error!(error = %e, "fatal");
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn real_main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = resolve_config_path(cli.config.clone())?;
    let config = config::load(&config_path)
        .with_context(|| format!("loading config {}", config_path.display()))?;

    logging::init(&config.log, cli.verbose);
    info!(config = %config_path.display(), "loaded configuration");

    let state_path = resolve_state_path(cli.state.clone());

    match cli.command {
        Command::Check { resolve } => check(&config, resolve).await,
        Command::Oneshot => {
            run::oneshot::run(
                &config,
                run::oneshot::Options {
                    state_path,
                    profiles: cli.profiles,
                    dry_run: cli.dry_run,
                    force: cli.force,
                },
            )
            .await
        }
        Command::Daemon => {
            run::daemon::run(
                config,
                run::daemon::Options {
                    state_path,
                    profiles: cli.profiles,
                    dry_run: cli.dry_run,
                    force: cli.force,
                },
            )
            .await
        }
    }
}

/// Validate config and optionally resolve IP sources.
async fn check(config: &config::Config, resolve: bool) -> Result<()> {
    info!(profiles = config.profiles.len(), "configuration is valid");

    if !resolve {
        println!("OK: {} profile(s) configured.", config.profiles.len());
        return Ok(());
    }

    let clients = ip::StackClients::new(config.defaults.http_timeout)?;
    for (name, profile) in &config.profiles {
        let (need_v4, need_v6) = profile.required_families();
        println!("profile '{name}':");
        if need_v4 {
            report(name, "v4", ip::resolve_pool(&profile.ip_sources.v4, ip::Family::V4, &clients).await);
        }
        if need_v6 {
            report(name, "v6", ip::resolve_pool(&profile.ip_sources.v6, ip::Family::V6, &clients).await);
        }
    }
    Ok(())
}

fn report(_profile: &str, family: &str, result: Result<std::net::IpAddr>) {
    match result {
        Ok(ip) => println!("  {family}: {ip}"),
        Err(e) => println!("  {family}: ERROR: {e:#}"),
    }
}

fn resolve_config_path(cli: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = cli {
        return Ok(p);
    }
    if let Ok(p) = std::env::var("DDNSONDEMAND_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let mut candidates = vec![
        PathBuf::from("config.yaml"),
        PathBuf::from("ddnsondemand.yaml"),
    ];
    if let Some(pd) = ProjectDirs::from("", "", "ddnsondemand") {
        candidates.push(pd.config_dir().join("config.yaml"));
    }
    candidates.push(PathBuf::from("/etc/ddnsondemand/config.yaml"));

    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    bail!(
        "no config file found; pass --config. Searched: {}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn resolve_state_path(cli: Option<PathBuf>) -> PathBuf {
    if let Some(p) = cli {
        return p;
    }
    if let Some(pd) = ProjectDirs::from("", "", "ddnsondemand") {
        let dir = pd
            .state_dir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| pd.data_local_dir().to_path_buf());
        return dir.join("state.json");
    }
    PathBuf::from("state.json")
}
