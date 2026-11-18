//! Template rendering context and helpers (minijinja).

use anyhow::{Context, Result};
use serde::Serialize;

/// Variables exposed to provider/notification templates.
#[derive(Debug, Clone, Serialize)]
pub struct RenderCtx {
    pub profile: String,
    pub domain: String,
    /// `A` or `AAAA`.
    pub record_type: String,
    /// `ipv4` or `ipv6`.
    pub family: String,
    pub ip: String,
    pub old_ip: String,
    pub ttl: u32,
    pub proxied: bool,
    pub zone: String,
    /// Notification event label (`changed` / `unchanged` / `failed`); empty for providers.
    pub event: String,
    /// Error message for failure notifications; empty otherwise.
    pub error: String,
    pub timestamp: String,
}

/// Render a single template string against the context.
pub fn render(source: &str, ctx: &RenderCtx) -> Result<String> {
    let env = minijinja::Environment::new();
    env.render_str(source, ctx)
        .with_context(|| format!("rendering template: {source}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RenderCtx {
        RenderCtx {
            profile: "home".into(),
            domain: "a.example.com".into(),
            record_type: "A".into(),
            family: "ipv4".into(),
            ip: "1.2.3.4".into(),
            old_ip: "1.1.1.1".into(),
            ttl: 300,
            proxied: false,
            zone: "example.com".into(),
            event: "changed".into(),
            error: String::new(),
            timestamp: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn renders_variables() {
        let out = render("host={{domain}}&ip={{ip}}&t={{record_type}}", &ctx()).unwrap();
        assert_eq!(out, "host=a.example.com&ip=1.2.3.4&t=A");
    }

    #[test]
    fn renders_json_body() {
        let out =
            render(r#"{"ip":"{{ip}}","old":"{{old_ip}}","event":"{{event}}"}"#, &ctx()).unwrap();
        assert_eq!(out, r#"{"ip":"1.2.3.4","old":"1.1.1.1","event":"changed"}"#);
    }
}
