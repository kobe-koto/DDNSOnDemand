//! Logging initialisation.

use crate::config::{LogConfig, LogFormat};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

/// Initialise the global tracing subscriber.
///
/// `verbose` (from `-v`) overrides the configured level: 1 = debug, 2+ = trace.
///
/// Uses a `Targets` filter (not `EnvFilter`) so we keep `RUST_LOG` and
/// per-target level filtering without pulling in the regex engine.
pub fn init(cfg: &LogConfig, verbose: u8) {
    let level = match verbose {
        0 => cfg.level,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    // Honour RUST_LOG if it is set and parses. Otherwise default this crate to
    // the resolved level and everything else to warn. An unparsable RUST_LOG
    // is reported, not silently dropped. Logging is not up yet, hence stderr.
    let from_env = std::env::var("RUST_LOG").ok().and_then(|raw| match raw.parse::<Targets>() {
        Ok(targets) => Some(targets),
        Err(e) => {
            eprintln!("warning: ignoring RUST_LOG={raw:?}: {e}");
            None
        }
    });
    let targets = from_env.unwrap_or_else(|| {
        Targets::new()
            .with_default(LevelFilter::WARN)
            .with_target("ddnsondemand", level)
    });

    let registry = tracing_subscriber::registry();
    match cfg.format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json().with_filter(targets))
            .init(),
        LogFormat::Text => registry
            .with(tracing_subscriber::fmt::layer().with_filter(targets))
            .init(),
    }
}
