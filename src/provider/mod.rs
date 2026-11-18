//! DNS provider abstraction and dispatch.

mod cloudflare;
mod generic;

use std::net::IpAddr;

use anyhow::Result;

use crate::config::ProviderConfig;
use crate::ip::Family;
use crate::template::RenderCtx;

pub use cloudflare::CloudflareProvider;
pub use generic::GenericProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    A,
    Aaaa,
}

impl RecordType {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Aaaa => "AAAA",
        }
    }

    pub fn family(self) -> Family {
        match self {
            RecordType::A => Family::V4,
            RecordType::Aaaa => Family::V6,
        }
    }
}

/// Outcome of ensuring a single record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The record was created or its content changed.
    Changed,
    /// The provider already had the desired content.
    Unchanged,
}

/// A ready-to-use DNS provider for one profile.
pub enum Provider {
    Cloudflare(CloudflareProvider),
    Generic(GenericProvider),
}

impl Provider {
    /// Build a provider from config. May perform network I/O (e.g. Cloudflare
    /// zone-id resolution).
    pub async fn build(cfg: &ProviderConfig) -> Result<Self> {
        match cfg {
            ProviderConfig::Cloudflare {
                api_token,
                zone_id,
                zone,
            } => Ok(Provider::Cloudflare(
                CloudflareProvider::new(api_token, zone_id.as_deref(), zone.as_deref()).await?,
            )),
            ProviderConfig::Generic { update } => {
                Ok(Provider::Generic(GenericProvider::new(update.clone())?))
            }
        }
    }

    /// Zone name/id used for template context (best-effort; may be empty).
    pub fn zone_label(&self) -> String {
        match self {
            Provider::Cloudflare(p) => p.zone_id().to_string(),
            Provider::Generic(_) => String::new(),
        }
    }

    /// Ensure `domain` has an `rtype` record pointing at `ip`.
    pub async fn ensure(
        &self,
        domain: &str,
        rtype: RecordType,
        ip: IpAddr,
        ttl: u32,
        proxied: bool,
        ctx: &RenderCtx,
    ) -> Result<Applied> {
        match self {
            Provider::Cloudflare(p) => p.ensure(domain, rtype, ip, ttl, proxied).await,
            Provider::Generic(p) => p.ensure(ctx).await,
        }
    }
}
