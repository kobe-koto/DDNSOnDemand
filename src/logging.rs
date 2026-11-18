//! Logging initialisation.

use crate::config::{LogConfig, LogFormat};
use tracing_subscriber::EnvFilter;

/// Initialise the global tracing subscriber.
///
/// `verbose` (from `-v`) overrides the configured level: 1 = debug, 2+ = trace.
pub fn init(cfg: &LogConfig, verbose: u8) {
    let level = match verbose {
        0 => cfg.level.clone(),
        1 => "debug".to_string(),
        _ => "trace".to_string(),
    };

    // Honour RUST_LOG if set, otherwise use the resolved level for our crate.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("ddnsondemand={level},warn")));

    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match cfg.format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Text => builder.init(),
    }
}
