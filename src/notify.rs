//! Templated GET/POST notifications fired on update outcomes.

use anyhow::{Context, Result};
use reqwest::Method;
use tracing::warn;

use crate::config::{Event, NotifyConfig, NotifyTarget};
use crate::template::{render, RenderCtx};

/// Outcome of processing a single record, used to decide which notifications fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Changed,
    Unchanged,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Changed => "changed",
            Outcome::Unchanged => "unchanged",
            Outcome::Failed => "failed",
        }
    }
}

fn should_fire(on: &[Event], outcome: Outcome) -> bool {
    on.iter().any(|e| {
        matches!(
            (e, outcome),
            (Event::Always, _)
                | (Event::Changed, Outcome::Changed)
                | (Event::Success, Outcome::Changed | Outcome::Unchanged)
                | (Event::Error, Outcome::Failed)
        )
    })
}

/// Send any notifications configured for this outcome. Failures are logged, not propagated.
pub async fn fire(
    client: &reqwest::Client,
    cfg: Option<&NotifyConfig>,
    outcome: Outcome,
    ctx: &RenderCtx,
) {
    let Some(cfg) = cfg else { return };
    if !should_fire(&cfg.on, outcome) {
        return;
    }
    for target in &cfg.targets {
        if let Err(e) = send_one(client, target, ctx).await {
            warn!(
                target = target.name.as_deref().unwrap_or(&target.url),
                error = %e,
                "notification failed"
            );
        }
    }
}

async fn send_one(
    client: &reqwest::Client,
    target: &NotifyTarget,
    ctx: &RenderCtx,
) -> Result<()> {
    let url = render(&target.url, ctx)?;
    let method = Method::from_bytes(target.method.to_uppercase().as_bytes())
        .with_context(|| format!("invalid HTTP method '{}'", target.method))?;

    let mut rb = client.request(method, &url);
    for (k, v) in &target.headers {
        rb = rb.header(k, render(v, ctx)?);
    }
    if let Some(body) = &target.body {
        rb = rb.body(render(body, ctx)?);
    }
    rb.send()
        .await
        .with_context(|| format!("sending notification to {url}"))?
        .error_for_status()
        .context("notification endpoint returned error status")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_matching_events() {
        let on = vec![Event::Changed, Event::Error];
        assert!(should_fire(&on, Outcome::Changed));
        assert!(should_fire(&on, Outcome::Failed));
        assert!(!should_fire(&on, Outcome::Unchanged));
    }

    #[test]
    fn success_covers_changed_and_unchanged() {
        let on = vec![Event::Success];
        assert!(should_fire(&on, Outcome::Changed));
        assert!(should_fire(&on, Outcome::Unchanged));
        assert!(!should_fire(&on, Outcome::Failed));
    }

    #[test]
    fn always_fires_for_everything() {
        let on = vec![Event::Always];
        assert!(should_fire(&on, Outcome::Changed));
        assert!(should_fire(&on, Outcome::Unchanged));
        assert!(should_fire(&on, Outcome::Failed));
    }

    use crate::config::NotifyTarget;
    use std::collections::BTreeMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
            zone: String::new(),
            event: "changed".into(),
            error: String::new(),
            timestamp: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[tokio::test]
    async fn posts_rendered_body_on_matching_event() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let cfg = NotifyConfig {
            on: vec![Event::Changed],
            targets: vec![NotifyTarget {
                name: Some("hook".into()),
                method: "POST".into(),
                url: format!("{}/hook", server.uri()),
                headers: BTreeMap::new(),
                body: Some(r#"{"ip":"{{ip}}"}"#.into()),
            }],
        };

        fire(&reqwest::Client::new(), Some(&cfg), Outcome::Changed, &ctx()).await;

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body, br#"{"ip":"1.2.3.4"}"#);
    }

    #[tokio::test]
    async fn skips_when_event_not_matched() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let cfg = NotifyConfig {
            on: vec![Event::Error],
            targets: vec![NotifyTarget {
                name: None,
                method: "POST".into(),
                url: format!("{}/hook", server.uri()),
                headers: BTreeMap::new(),
                body: None,
            }],
        };

        fire(&reqwest::Client::new(), Some(&cfg), Outcome::Changed, &ctx()).await;
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }
}
