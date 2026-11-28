//! Logging initialisation.

use std::io::{self, Write};

use crate::config::{LogConfig, LogFormat};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;

/// A `MakeWriter` that scrubs registered secrets from each log line before it
/// reaches stdout (the fmt layer's default stream). The layer makes one writer
/// per event and writes the whole formatted line, newline included, to it, so
/// masking the buffer on drop operates on one complete line. Secrets never span
/// lines, so per-line masking is sufficient and works for text and JSON alike.
struct MaskingWriter;

impl<'a> MakeWriter<'a> for MaskingWriter {
    type Writer = LineBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        LineBuffer { buf: Vec::new() }
    }
}

struct LineBuffer {
    buf: Vec<u8>,
}

impl Write for LineBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineBuffer {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let masked = crate::secret::mask(&String::from_utf8_lossy(&self.buf));
        // Lock once so a masked line is written atomically, not interleaved.
        let mut out = io::stdout().lock();
        let _ = out.write_all(masked.as_bytes());
        let _ = out.flush();
    }
}

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
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(MaskingWriter)
                    .with_filter(targets),
            )
            .init(),
        LogFormat::Text => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(MaskingWriter)
                    .with_filter(targets),
            )
            .init(),
    }
}
