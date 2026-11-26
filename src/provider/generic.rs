//! Generic templated GET/POST provider for non-Cloudflare services.

use anyhow::{bail, Context, Result};
use reqwest::Method;
use tracing::info;

use super::Applied;
use crate::config::GenericRequest;
use crate::template::{render, RenderCtx};

pub struct GenericProvider {
    client: reqwest::Client,
    req: GenericRequest,
}

impl GenericProvider {
    pub fn new(req: GenericRequest) -> Result<Self> {
        let client = crate::http::client_builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self { client, req })
    }

    pub async fn ensure(&self, ctx: &RenderCtx) -> Result<Applied> {
        let url = render(&self.req.url, ctx)?;
        let method = Method::from_bytes(self.req.method.to_uppercase().as_bytes())
            .with_context(|| format!("invalid HTTP method '{}'", self.req.method))?;

        let mut rb = self.client.request(method, &url);
        for (k, v) in &self.req.headers {
            rb = rb.header(k, render(v, ctx)?);
        }
        if let Some(body) = &self.req.body {
            rb = rb.body(render(body, ctx)?);
        }

        let resp = rb.send().await.with_context(|| format!("requesting {url}"))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();

        let status_ok = self.req.success.status.contains(&status);
        let body_ok = self
            .req
            .success
            .body_contains
            .as_ref()
            .is_none_or(|needle| text.contains(needle));

        if !status_ok || !body_ok {
            bail!("generic provider rejected update: HTTP {status}, body: {text}");
        }

        info!(domain = %ctx.domain, rtype = %ctx.record_type, ip = %ctx.ip, "generic provider updated");
        // The generic provider cannot tell "changed" from "unchanged"; the
        // engine only calls it when a change is needed, so report Changed.
        Ok(Applied::Changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SuccessSpec;
    use std::collections::BTreeMap;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx(ip: &str) -> RenderCtx {
        RenderCtx {
            profile: "home".into(),
            domain: "a.example.com".into(),
            record_type: "A".into(),
            family: "ipv4".into(),
            ip: ip.into(),
            old_ip: String::new(),
            ttl: 120,
            proxied: false,
            zone: String::new(),
            event: "changed".into(),
            error: String::new(),
            timestamp: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn req(url: String, body_contains: Option<&str>) -> GenericRequest {
        GenericRequest {
            method: "GET".into(),
            url,
            headers: BTreeMap::new(),
            body: None,
            success: SuccessSpec {
                status: vec![200],
                body_contains: body_contains.map(|s| s.to_string()),
            },
        }
    }

    #[tokio::test]
    async fn get_with_templated_query_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/update"))
            .and(query_param("ip", "1.2.3.4"))
            .respond_with(ResponseTemplate::new(200).set_body_string("good"))
            .expect(1)
            .mount(&server)
            .await;

        let url = format!("{}/update?ip={{{{ip}}}}", server.uri());
        let p = GenericProvider::new(req(url, Some("good"))).unwrap();
        assert_eq!(p.ensure(&ctx("1.2.3.4")).await.unwrap(), Applied::Changed);
    }

    #[tokio::test]
    async fn rejects_on_body_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("nochg"))
            .mount(&server)
            .await;

        let url = format!("{}/update?ip={{{{ip}}}}", server.uri());
        let p = GenericProvider::new(req(url, Some("good"))).unwrap();
        assert!(p.ensure(&ctx("1.2.3.4")).await.is_err());
    }
}
