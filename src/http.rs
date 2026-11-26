//! Outbound HTTP client construction.
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
