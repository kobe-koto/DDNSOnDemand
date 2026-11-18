//! IP address resolution: per-family source pools with declaration-order fallback.

mod http;
mod interface;

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use anyhow::{anyhow, Result};
use tracing::{debug, warn};

use crate::config::IpSourceConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::V4 => "ipv4",
            Family::V6 => "ipv6",
        }
    }

    fn matches(self, ip: &IpAddr) -> bool {
        matches!(
            (self, ip),
            (Family::V4, IpAddr::V4(_)) | (Family::V6, IpAddr::V6(_))
        )
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// HTTP clients bound to a specific stack, mimicking `curl -4` / `curl -6`.
///
/// Setting the connector's local address to the unspecified address of a
/// family forces hyper to only establish connections over that family.
#[derive(Clone)]
pub struct StackClients {
    v4: reqwest::Client,
    v6: reqwest::Client,
}

impl StackClients {
    pub fn new(timeout: Duration) -> Result<Self> {
        let v4 = reqwest::Client::builder()
            .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
            .timeout(timeout)
            .build()?;
        let v6 = reqwest::Client::builder()
            .local_address(IpAddr::V6(Ipv6Addr::UNSPECIFIED))
            .timeout(timeout)
            .build()?;
        Ok(Self { v4, v6 })
    }

    fn for_family(&self, family: Family) -> &reqwest::Client {
        match family {
            Family::V4 => &self.v4,
            Family::V6 => &self.v6,
        }
    }
}

/// Resolve an address for `family` by trying each source in order and
/// returning the first success.
pub async fn resolve_pool(
    sources: &[IpSourceConfig],
    family: Family,
    clients: &StackClients,
) -> Result<IpAddr> {
    let mut last_err: Option<anyhow::Error> = None;
    for source in sources {
        match resolve_one(source, family, clients).await {
            Ok(ip) => {
                debug!(%family, %ip, "resolved IP");
                return Ok(ip);
            }
            Err(e) => {
                warn!(%family, error = %e, "IP source failed, trying next");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no {family} IP sources configured")))
}

async fn resolve_one(
    source: &IpSourceConfig,
    family: Family,
    clients: &StackClients,
) -> Result<IpAddr> {
    match source {
        IpSourceConfig::Interface { interface, select } => {
            interface::resolve_interface(interface, family, select)
        }
        IpSourceConfig::HttpPlain { url } => {
            http::resolve_http_plain(clients.for_family(family), url, family).await
        }
        IpSourceConfig::HttpJson { url, key } => {
            http::resolve_http_json(clients.for_family(family), url, key, family).await
        }
    }
}

/// Reject an address whose family does not match what was requested.
fn check_family(ip: IpAddr, family: Family) -> Result<IpAddr> {
    if family.matches(&ip) {
        Ok(ip)
    } else {
        Err(anyhow!("source returned {ip}, which is not {family}"))
    }
}
