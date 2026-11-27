//! Outbound HTTP: client construction, and URL redaction for logs.
//!
//! By default reqwest builds a fresh rustls `ClientConfig` for every `Client`:
//! a root store holding the bundled webpki roots, a crypto provider and a
//! session cache. That is about 28 KB of heap per client, and the daemon holds
//! three clients plus one per profile. Here every client is built from one
//! shared config that matches reqwest's defaults, so each extra client costs
//! about 4 KB.

use std::sync::{Arc, OnceLock};

/// A client builder that uses the shared TLS config and our User-Agent.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        // Cloning only bumps reference counts: the root store, provider and
        // session cache stay shared.
        .use_preconfigured_tls(tls_config().clone())
}

/// Reduce a URL to `scheme://host[:port]` for logs and error messages.
///
/// URLs here often carry secrets. A DDNS update token can sit in the query
/// (`?token=`), a webhook or bot token in the path (Slack, Discord, Telegram),
/// credentials in the userinfo. Anything that gets printed shows only the
/// origin: log lines, `check` output, and the `{{error}}` variable, which
/// failure notifications forward to another service. `/...` marks that
/// something was dropped.
///
/// The URL is parsed with the same parser reqwest uses. So the host shown is
/// the one the request actually goes to, also for the WHATWG forms reqwest
/// accepts, such as `http:/host`, `https:\\host` or a stray tab. Anything that
/// does not parse prints as `<unparsable URL>`, never as text. That covers a
/// missing scheme, an invalid port, or a template with spaces inside `{{ }}`.
pub fn redact(url: &str) -> String {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return "<unparsable URL>".to_string();
    };
    let scheme = parsed.scheme();
    let Some(host) = parsed.host_str() else {
        return format!("{scheme}:...");
    };
    // An '@' that did not become the userinfo delimiter usually means a
    // credential delimiter was not percent-encoded, as in `user:12/pass@host`.
    // The parser may then take part of the credentials for host and port,
    // so show neither. A plain '@' in a path or query loses its host the same
    // way, which costs only some detail.
    let has_userinfo = !parsed.username().is_empty() || parsed.password().is_some();
    if !has_userinfo && url.contains('@') {
        return format!("{scheme}://<hidden>/...");
    }
    let mut shown = format!("{scheme}://{host}");
    if let Some(port) = parsed.port() {
        shown.push_str(&format!(":{port}"));
    }
    let dropped = has_userinfo
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some();
    if dropped {
        shown.push_str("/...");
    }
    shown
}

/// The process-wide rustls config, built on first use.
///
/// It reproduces what reqwest 0.12 builds itself with the `rustls-tls` feature
/// and no HTTP/2: the ring provider, TLS 1.2 and 1.3, the bundled webpki roots,
/// SNI on and ALPN `http/1.1`.
fn tls_config() -> &'static rustls::ClientConfig {
    static CONFIG: OnceLock<rustls::ClientConfig> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut config =
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::ALL_VERSIONS)
                .expect("the ring provider supports every rustls protocol version")
                .with_root_certificates(roots)
                .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_keeps_only_the_origin() {
        let cases = [
            ("https://api.ipify.org", "https://api.ipify.org"),
            ("https://api.ipify.org/", "https://api.ipify.org"),
            ("HTTPS://API.IPIFY.ORG/x", "https://api.ipify.org/..."),
            ("https://ipinfo.io/json?token=s3cr3t", "https://ipinfo.io/..."),
            ("https://www.duckdns.org/update?domains=x&token=s3cr3t&ip=1.2.3.4", "https://www.duckdns.org/..."),
            ("https://api.telegram.org/bots3cr3t/sendMessage", "https://api.telegram.org/..."),
            ("https://hooks.slack.com/services/T0/B0/s3cr3t", "https://hooks.slack.com/..."),
            ("https://dyn.example.net?token=s3cr3t", "https://dyn.example.net/..."),
            ("https://dyn.example.net#s3cr3t", "https://dyn.example.net/..."),
            ("https://user:s3cr3t@dyn.example.net/", "https://dyn.example.net/..."),
            ("https://user:s3cr3t@dyn.example.net", "https://dyn.example.net/..."),
            ("https://user:pa@s3cr3t@dyn.example.net/", "https://dyn.example.net/..."),
            ("http://u:s3cr3t@[2001:db8::1]:8080/x", "http://[2001:db8::1]:8080/..."),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("https://{{zone}}.example.com/u?ip={{ip}}", "https://{{zone}}.example.com/..."),
            // WHATWG forms reqwest accepts and sends: show the real host.
            ("http:/dyn.example.net/u?token=s3cr3t&cb=https://x.example/", "http://dyn.example.net/..."),
            ("http:dyn.example.net/u?token=s3cr3t", "http://dyn.example.net/..."),
            ("https:\\\\dyn.example.net\\update?token=s3cr3t", "https://dyn.example.net/..."),
            ("https://dyn.example.net\\s3cr3t", "https://dyn.example.net/..."),
            ("https:\t//api.ipify.org", "https://api.ipify.org"),
            // Unencoded delimiter in a password: never print host or userinfo.
            ("https://user:ab/s3cr3t@dyn.example.net/u", "<unparsable URL>"),
            ("https://user:12/s3cr3t@dyn.example.net/u", "https://<hidden>/..."),
            ("https://user:12#s3cr3t@dyn.example.net", "https://<hidden>/..."),
            ("https://user:12?s3cr3t@dyn.example.net", "https://<hidden>/..."),
            // Not a usable URL: print nothing of it.
            ("dyn.example.net/update?token=s3cr3t", "<unparsable URL>"),
            ("dyn.example.net/nic/update?token=s3cr3t&cb=https://status.example/", "<unparsable URL>"),
            ("https://{{ zone }}.example.com/x?t=s3cr3t", "<unparsable URL>"),
            ("mailto:s3cr3t@example.com", "mailto:..."),
            ("", "<unparsable URL>"),
        ];
        for (url, want) in cases {
            let got = redact(url);
            assert_eq!(got, want, "{url:?}");
            assert!(!got.contains("s3cr3t") && !got.contains("user"), "{url:?} -> {got}");
        }
    }

    #[test]
    fn reqwest_accepts_the_shared_config() {
        // reqwest takes the config as `impl Any` and downcasts it to its own
        // `rustls::ClientConfig`. If our rustls version ever differs from
        // reqwest's, the downcast fails and `build()` errors with "Unknown TLS
        // backend". This test catches that.
        client_builder().build().expect("client builds with the shared TLS config");
    }

    #[test]
    fn shared_config_matches_reqwest_defaults() {
        let config = tls_config();
        assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
        assert!(config.enable_sni);
        // TLS 1.2 is only in ALL_VERSIONS when rustls's `tls12` feature is on.
        let versions: Vec<_> = rustls::ALL_VERSIONS.iter().map(|v| v.version).collect();
        assert!(versions.contains(&rustls::ProtocolVersion::TLSv1_3));
        assert!(versions.contains(&rustls::ProtocolVersion::TLSv1_2));
    }
}
