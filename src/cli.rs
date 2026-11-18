//! Command-line interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ddnsondemand",
    version,
    about = "On-demand / daemon DDNS client"
)]
pub struct Cli {
    /// Path to the YAML configuration file.
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    /// Path to the state file (last-known IPs). Defaults to an XDG state path.
    #[arg(long, global = true)]
    pub state: Option<PathBuf>,

    /// Limit processing to these profile names (repeatable). Default: all enabled.
    #[arg(long = "profile", global = true)]
    pub profiles: Vec<String>,

    /// Resolve and log intended actions without calling providers or notifiers.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Force provider updates even when the state shows no change.
    #[arg(long, global = true)]
    pub force: bool,

    /// Increase log verbosity (-v = debug, -vv = trace). Overrides config log level.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run continuously, updating each profile on its interval.
    Daemon,
    /// Process every selected profile once, then exit (for systemd timers).
    Oneshot,
    /// Validate the configuration; with --resolve, also resolve IP sources.
    Check {
        /// Resolve each profile's IP sources and print the results.
        #[arg(long)]
        resolve: bool,
    },
}
