//! Cloudflare DNS provider using the API v4.

use std::net::IpAddr;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::{Applied, RecordType};

const API_BASE: &str = "https://api.cloudflare.com/client/v4";

pub struct CloudflareProvider {
    client: reqwest::Client,
    token: String,
    zone_id: String,
    base: String,
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
}

#[derive(Deserialize)]
struct ApiError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct Zone {
    id: String,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    content: String,
    #[serde(default)]
    ttl: u32,
    #[serde(default)]
    proxied: bool,
}

#[derive(Serialize)]
struct RecordBody<'a> {
    #[serde(rename = "type")]
    rtype: &'a str,
    name: &'a str,
    content: String,
    ttl: u32,
    proxied: bool,
}

impl CloudflareProvider {
    pub async fn new(token: &str, zone_id: Option<&str>, zone: Option<&str>) -> Result<Self> {
        let client = crate::http::client_builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let provider = Self {
            client,
            token: token.to_string(),
            zone_id: String::new(),
            base: API_BASE.to_string(),
        };
        let zone_id = match zone_id {
            Some(id) => id.to_string(),
            None => {
                let name = zone.ok_or_else(|| anyhow!("cloudflare: zone_id or zone required"))?;
                provider.resolve_zone_id(name).await?
            }
        };
        Ok(Self { zone_id, ..provider })
    }

    pub fn zone_id(&self) -> &str {
        &self.zone_id
    }

    async fn resolve_zone_id(&self, name: &str) -> Result<String> {
        let url = format!("{}/zones", self.base);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .query(&[("name", name)])
            .send()
            .await
            .context("cloudflare: listing zones")?;
        let env: Envelope<Vec<Zone>> = parse_envelope(resp, "list zones").await?;
        env.result
            .and_then(|mut v| v.drain(..).next())
            .map(|z| z.id)
            .ok_or_else(|| anyhow!("cloudflare: zone '{name}' not found for this token"))
    }

    pub async fn ensure(
        &self,
        domain: &str,
        rtype: RecordType,
        ip: IpAddr,
        ttl: u32,
        proxied: bool,
    ) -> Result<Applied> {
        // Cloudflare requires ttl=1 ("automatic") for proxied records.
        let effective_ttl = if proxied { 1 } else { ttl.max(1) };
        let content = ip.to_string();

        let existing = self.find_record(domain, rtype).await?;
        match existing {
            Some(rec) => {
                if rec.content == content && rec.proxied == proxied && rec.ttl == effective_ttl {
                    debug!(domain, rtype = rtype.as_str(), "cloudflare record already current");
                    return Ok(Applied::Unchanged);
                }
                self.put_record(&rec.id, domain, rtype, content, effective_ttl, proxied)
                    .await?;
                info!(domain, rtype = rtype.as_str(), %ip, "cloudflare record updated");
                Ok(Applied::Changed)
            }
            None => {
                self.post_record(domain, rtype, content, effective_ttl, proxied)
                    .await?;
                info!(domain, rtype = rtype.as_str(), %ip, "cloudflare record created");
                Ok(Applied::Changed)
            }
        }
    }

    async fn find_record(&self, domain: &str, rtype: RecordType) -> Result<Option<Record>> {
        let url = format!("{}/zones/{}/dns_records", self.base, self.zone_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .query(&[("type", rtype.as_str()), ("name", domain)])
            .send()
            .await
            .context("cloudflare: listing dns records")?;
        let env: Envelope<Vec<Record>> = parse_envelope(resp, "list dns records").await?;
        Ok(env.result.and_then(|mut v| v.drain(..).next()))
    }

    async fn post_record(
        &self,
        domain: &str,
        rtype: RecordType,
        content: String,
        ttl: u32,
        proxied: bool,
    ) -> Result<()> {
        let url = format!("{}/zones/{}/dns_records", self.base, self.zone_id);
        let body = RecordBody {
            rtype: rtype.as_str(),
            name: domain,
            content,
            ttl,
            proxied,
        };
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("cloudflare: creating dns record")?;
        let _: Envelope<Record> = parse_envelope(resp, "create dns record").await?;
        Ok(())
    }

    async fn put_record(
        &self,
        id: &str,
        domain: &str,
        rtype: RecordType,
        content: String,
        ttl: u32,
        proxied: bool,
    ) -> Result<()> {
        let url = format!("{}/zones/{}/dns_records/{id}", self.base, self.zone_id);
        let body = RecordBody {
            rtype: rtype.as_str(),
            name: domain,
            content,
            ttl,
            proxied,
        };
        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("cloudflare: updating dns record")?;
        let _: Envelope<Record> = parse_envelope(resp, "update dns record").await?;
        Ok(())
    }
}

async fn parse_envelope<T: for<'de> Deserialize<'de>>(
    resp: reqwest::Response,
    what: &str,
) -> Result<Envelope<T>> {
    let status = resp.status();
    let text = resp.text().await.context("cloudflare: reading response body")?;
    let env: Envelope<T> = serde_json::from_str(&text)
        .with_context(|| format!("cloudflare: decoding {what} response (HTTP {status}): {text}"))?;
    if !env.success {
        let msg = env
            .errors
            .iter()
            .map(|e| format!("[{}] {}", e.code, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        bail!("cloudflare: {what} failed (HTTP {status}): {msg}");
    }
    Ok(env)
}

#[cfg(test)]
impl CloudflareProvider {
    /// Construct a provider pointed at a test server, skipping zone resolution.
    pub(crate) fn for_test(base: &str, token: &str, zone_id: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            token: token.to_string(),
            zone_id: zone_id.to_string(),
            base: base.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn record_json(content: &str, ttl: u32, proxied: bool) -> serde_json::Value {
        serde_json::json!({"id": "rec1", "content": content, "ttl": ttl, "proxied": proxied})
    }

    #[tokio::test]
    async fn creates_record_when_absent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zones/ZID/dns_records"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"success": true, "errors": [], "result": []}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/zones/ZID/dns_records"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true, "errors": [], "result": record_json("1.2.3.4", 300, false)
            })))
            .expect(1)
            .mount(&server)
            .await;

        let p = CloudflareProvider::for_test(&server.uri(), "tok", "ZID");
        let applied = p
            .ensure("a.example.com", RecordType::A, "1.2.3.4".parse().unwrap(), 300, false)
            .await
            .unwrap();
        assert_eq!(applied, Applied::Changed);
    }

    #[tokio::test]
    async fn updates_record_when_content_differs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zones/ZID/dns_records"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true, "errors": [], "result": [record_json("9.9.9.9", 300, false)]
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/zones/ZID/dns_records/rec1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true, "errors": [], "result": record_json("1.2.3.4", 300, false)
            })))
            .expect(1)
            .mount(&server)
            .await;

        let p = CloudflareProvider::for_test(&server.uri(), "tok", "ZID");
        let applied = p
            .ensure("a.example.com", RecordType::A, "1.2.3.4".parse().unwrap(), 300, false)
            .await
            .unwrap();
        assert_eq!(applied, Applied::Changed);
    }

    #[tokio::test]
    async fn no_update_when_already_current() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zones/ZID/dns_records"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true, "errors": [], "result": [record_json("1.2.3.4", 300, false)]
            })))
            .mount(&server)
            .await;
        // No PUT/POST mock: a call would 404 and fail the test.

        let p = CloudflareProvider::for_test(&server.uri(), "tok", "ZID");
        let applied = p
            .ensure("a.example.com", RecordType::A, "1.2.3.4".parse().unwrap(), 300, false)
            .await
            .unwrap();
        assert_eq!(applied, Applied::Unchanged);
    }

    #[tokio::test]
    async fn surfaces_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zones/ZID/dns_records"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "success": false, "errors": [{"code": 9109, "message": "Invalid access token"}], "result": null
            })))
            .mount(&server)
            .await;

        let p = CloudflareProvider::for_test(&server.uri(), "tok", "ZID");
        let err = p
            .ensure("a.example.com", RecordType::A, "1.2.3.4".parse().unwrap(), 300, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("9109"));
    }
}

