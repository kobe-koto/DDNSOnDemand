//! HTTP-based IP sources (plain text and JSON).

use std::collections::BTreeMap;
use std::net::IpAddr;

use anyhow::{anyhow, Context, Result};

use super::{check_family, Family};
use crate::http::redact;

pub async fn resolve_http_plain(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    family: Family,
) -> Result<IpAddr> {
    let shown = redact(url);
    let text = get(client, url, headers, &shown)
        .await?
        .text()
        .await
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("reading body from {shown}"))?;
    let ip: IpAddr = text
        .trim()
        .parse()
        .with_context(|| format!("parsing '{}' from {shown} as an IP", text.trim()))?;
    check_family(ip, family)
}

pub async fn resolve_http_json(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    headers: &BTreeMap<String, String>,
    family: Family,
) -> Result<IpAddr> {
    let shown = redact(url);
    let value: serde_json::Value = get(client, url, headers, &shown)
        .await?
        .json()
        .await
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("decoding JSON from {shown}"))?;
    let raw = extract_key(&value, key)?;
    let ip: IpAddr = raw
        .parse()
        .with_context(|| format!("parsing '{raw}' (key '{key}') from {shown} as an IP"))?;
    check_family(ip, family)
}

/// GET `url` and return the response if its status is a success.
///
/// IP source URLs can carry an API token (e.g. `https://ipinfo.io/json?token=`),
/// so errors name the URL only as `shown`, its redacted form. reqwest's own
/// errors would append the full URL, so it is stripped from them.
async fn get(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    shown: &str,
) -> Result<reqwest::Response> {
    let mut rb = client.get(url);
    for (k, v) in headers {
        rb = rb.header(k, v);
    }
    rb.send()
        .await
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("requesting {shown}"))?
        .error_for_status()
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("non-success status from {shown}"))
}

/// Extract a string value using either a JSON pointer (`/a/b`) or a top-level key.
fn extract_key(value: &serde_json::Value, key: &str) -> Result<String> {
    let node = if key.starts_with('/') {
        value.pointer(key)
    } else {
        value.get(key)
    }
    .ok_or_else(|| anyhow!("key '{key}' not found in JSON response"))?;

    match node {
        serde_json::Value::String(s) => Ok(s.clone()),
        other => Err(anyhow!("value at '{key}' is not a string: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_key() {
        let v: serde_json::Value = serde_json::json!({"ip": "1.2.3.4"});
        assert_eq!(extract_key(&v, "ip").unwrap(), "1.2.3.4");
    }

    #[test]
    fn extracts_pointer() {
        let v: serde_json::Value = serde_json::json!({"data": {"ip": "2001:db8::1"}});
        assert_eq!(extract_key(&v, "/data/ip").unwrap(), "2001:db8::1");
    }

    #[test]
    fn missing_key_errors() {
        let v: serde_json::Value = serde_json::json!({"other": "x"});
        assert!(extract_key(&v, "ip").is_err());
    }

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn plain_resolves_and_trims() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("203.0.113.9\n"))
            .mount(&server)
            .await;
        let ip = resolve_http_plain(&reqwest::Client::new(), &server.uri(), &BTreeMap::new(), Family::V4)
            .await
            .unwrap();
        assert_eq!(ip.to_string(), "203.0.113.9");
    }

    #[tokio::test]
    async fn plain_rejects_wrong_family() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("2001:db8::1"))
            .mount(&server)
            .await;
        let err = resolve_http_plain(&reqwest::Client::new(), &server.uri(), &BTreeMap::new(), Family::V4).await;
        assert!(err.is_err());
    }

    /// Every message in the error chain, as `{e:#}` prints it (`check --resolve`
    /// and the fatal error line do).
    fn chain(e: &anyhow::Error) -> String {
        format!("{e:#}")
    }

    #[tokio::test]
    async fn errors_do_not_leak_url_secrets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        let no_headers = BTreeMap::new();

        // Non-success status.
        let url = format!("{}/ip?token=s3cr3t", server.uri());
        let e = resolve_http_plain(&client, &url, &no_headers, Family::V4).await.unwrap_err();
        assert!(!chain(&e).contains("s3cr3t"), "{e:#}");
        assert!(chain(&e).contains(&format!("non-success status from {}/...", server.uri())), "{e:#}");
        assert!(chain(&e).contains("500"), "{e:#}");

        // Connection failure (nothing listens on port 1).
        let url = "http://127.0.0.1:1/ip/s3cr3t?token=s3cr3t";
        let e = resolve_http_json(&client, url, "ip", &no_headers, Family::V4).await.unwrap_err();
        assert!(!chain(&e).contains("s3cr3t"), "{e:#}");
        assert!(chain(&e).contains("requesting http://127.0.0.1:1/..."), "{e:#}");
    }

    #[tokio::test]
    async fn json_resolves_via_pointer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": {"ip": "203.0.113.7"}})),
            )
            .mount(&server)
            .await;
        let ip = resolve_http_json(&reqwest::Client::new(), &server.uri(), "/data/ip", &BTreeMap::new(), Family::V4)
            .await
            .unwrap();
        assert_eq!(ip.to_string(), "203.0.113.7");
    }
}
