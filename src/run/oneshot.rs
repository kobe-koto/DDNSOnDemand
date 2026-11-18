//! One-shot mode: process every selected profile once, then exit.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::error;

use crate::config::Config;
use crate::engine::Engine;
use crate::ip::StackClients;
use crate::provider::Provider;
use crate::state::State;

pub struct Options {
    pub state_path: std::path::PathBuf,
    pub profiles: Vec<String>,
    pub dry_run: bool,
    pub force: bool,
}

pub async fn run(config: &Config, opts: Options) -> Result<()> {
    let clients = StackClients::new(config.defaults.http_timeout)?;
    let state = Arc::new(Mutex::new(State::load(&opts.state_path)?));
    let engine = Engine::new(
        clients,
        state,
        opts.state_path.clone(),
        opts.dry_run,
        opts.force,
    )?;

    for (name, profile) in crate::run::selected_profiles(config, &opts.profiles) {
        // Dry-run stays offline; otherwise build the provider once.
        let provider = if opts.dry_run {
            None
        } else {
            match Provider::build(&profile.targets.provider).await {
                Ok(p) => Some(p),
                Err(e) => {
                    error!(profile = %name, error = %e, "failed to initialise provider; skipping");
                    continue;
                }
            }
        };
        engine.run_profile(name, profile, provider.as_ref()).await;
    }
    Ok(())
}
