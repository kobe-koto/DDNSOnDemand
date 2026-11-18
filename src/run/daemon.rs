//! Daemon mode: one periodic task per profile, with graceful shutdown.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;
use tracing::{error, info};

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

pub async fn run(config: Config, opts: Options) -> Result<()> {
    let clients = StackClients::new(config.defaults.http_timeout)?;
    let state = Arc::new(Mutex::new(State::load(&opts.state_path)?));
    let engine = Arc::new(Engine::new(
        clients,
        state,
        opts.state_path.clone(),
        opts.dry_run,
        opts.force,
    )?);

    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let mut handles = Vec::new();
    let dry_run = opts.dry_run;

    for (name, profile) in crate::run::selected_profiles(&config, &opts.profiles) {
        let name = name.clone();
        let profile = profile.clone();
        let interval = profile.interval.unwrap_or(config.defaults.interval);
        let engine = engine.clone();
        let mut shutdown = shutdown_tx.subscribe();

        info!(profile = %name, interval = ?interval, "scheduling profile");
        handles.push(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // First tick fires immediately, so each profile runs once at start.
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            // Provider is built lazily and cached for the task's lifetime, so
            // e.g. Cloudflare zone resolution happens at most once per profile.
            let mut provider: Option<Provider> = None;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if dry_run {
                            engine.run_profile(&name, &profile, None).await;
                            continue;
                        }
                        if provider.is_none() {
                            match Provider::build(&profile.targets.provider).await {
                                Ok(p) => provider = Some(p),
                                Err(e) => {
                                    error!(profile = %name, error = %e, "provider init failed; retrying next cycle");
                                }
                            }
                        }
                        if let Some(p) = provider.as_ref() {
                            engine.run_profile(&name, &profile, Some(p)).await;
                        }
                    }
                    _ = shutdown.changed() => {
                        info!(profile = %name, "stopping profile task");
                        break;
                    }
                }
            }
        }));
    }

    if handles.is_empty() {
        info!("no enabled profiles matched; nothing to do");
        return Ok(());
    }

    wait_for_shutdown().await;
    info!("shutdown signal received; stopping daemon");
    let _ = shutdown_tx.send(true);
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(_) => return,
        };
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
