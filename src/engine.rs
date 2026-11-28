//! Per-profile update orchestration.

use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::config::{Profile, Targets};
use crate::ip::{resolve_pool, Family, StackClients};
use crate::notify::{self, Outcome};
use crate::provider::{Applied, Provider, RecordType};
use crate::state::State;
use crate::template::RenderCtx;

pub struct Engine {
    clients: StackClients,
    notify_client: reqwest::Client,
    state: Arc<Mutex<State>>,
    state_path: std::path::PathBuf,
    dry_run: bool,
    force: bool,
}

impl Engine {
    pub fn new(
        clients: StackClients,
        state: Arc<Mutex<State>>,
        state_path: std::path::PathBuf,
        dry_run: bool,
        force: bool,
    ) -> Result<Self> {
        Ok(Self {
            clients,
            notify_client: crate::http::client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()?,
            state,
            state_path,
            dry_run,
            force,
        })
    }

    /// Run one update cycle for a single profile. `provider` is built once by
    /// the caller and reused across cycles; it is `None` only in dry-run mode.
    /// Errors are contained so a single failure never aborts the whole run.
    pub async fn run_profile(&self, name: &str, profile: &Profile, provider: Option<&Provider>) {
        info!(profile = name, "processing profile");
        let (need_v4, need_v6) = profile.required_families();

        let v4 = if need_v4 {
            Some(resolve_pool(&profile.ip_sources.v4, Family::V4, &self.clients).await)
        } else {
            None
        };
        let v6 = if need_v6 {
            Some(resolve_pool(&profile.ip_sources.v6, Family::V6, &self.clients).await)
        } else {
            None
        };

        // Provider is supplied by the caller (built once per profile).
        let zone = provider.map(|p| p.zone_label()).unwrap_or_default();

        for (domain, rtype) in record_targets(&profile.targets) {
            let family = rtype.family();
            let resolved = match family {
                Family::V4 => v4.as_ref(),
                Family::V6 => v6.as_ref(),
            };
            match resolved {
                Some(Ok(ip)) => {
                    self.process_record(name, profile, provider, &zone, &domain, rtype, *ip)
                        .await;
                }
                Some(Err(e)) => {
                    warn!(profile = name, %domain, rtype = rtype.as_str(), error = %e, "no IP for family; skipping record");
                    let ctx = self
                        .build_ctx(name, &domain, rtype, "", "", profile.targets.ttl, profile.targets.proxied, &zone, Outcome::Failed, &e.to_string())
                        .await;
                    self.maybe_notify(profile, Outcome::Failed, &ctx).await;
                }
                None => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_record(
        &self,
        profile_name: &str,
        profile: &Profile,
        provider: Option<&Provider>,
        zone: &str,
        domain: &str,
        rtype: RecordType,
        ip: IpAddr,
    ) {
        let targets = &profile.targets;
        let new_ip = ip.to_string();
        let old_ip = {
            let state = self.state.lock().await;
            state
                .get(profile_name, domain, rtype)
                .map(|e| e.ip.clone())
                .unwrap_or_default()
        };

        let unchanged = old_ip == new_ip && !self.force;

        if self.dry_run {
            if unchanged {
                info!(profile = profile_name, %domain, rtype = rtype.as_str(), ip = %new_ip, "dry-run: already current, would skip");
            } else {
                info!(profile = profile_name, %domain, rtype = rtype.as_str(), ip = %new_ip, old = %old_ip, "dry-run: would update");
            }
            return;
        }

        if unchanged {
            info!(profile = profile_name, %domain, rtype = rtype.as_str(), ip = %new_ip, "unchanged; skipping provider");
            let ctx = self
                .build_ctx(profile_name, domain, rtype, &new_ip, &old_ip, targets.ttl, targets.proxied, zone, Outcome::Unchanged, "")
                .await;
            self.maybe_notify(profile, Outcome::Unchanged, &ctx).await;
            return;
        }

        let ctx = self
            .build_ctx(profile_name, domain, rtype, &new_ip, &old_ip, targets.ttl, targets.proxied, zone, Outcome::Changed, "")
            .await;

        let provider = provider.expect("provider present when not dry-run");
        match provider
            .ensure(domain, rtype, ip, targets.ttl, targets.proxied, &ctx)
            .await
        {
            Ok(Applied::Changed) | Ok(Applied::Unchanged) => {
                {
                    let mut state = self.state.lock().await;
                    state.set(profile_name, domain, rtype, &new_ip);
                    if let Err(e) = state.save(&self.state_path) {
                        warn!(error = %e, "failed to persist state");
                    }
                }
                self.maybe_notify(profile, Outcome::Changed, &ctx).await;
            }
            Err(e) => {
                error!(profile = profile_name, %domain, rtype = rtype.as_str(), error = %e, "update failed");
                let fail_ctx = RenderCtx {
                    event: Outcome::Failed.as_str().to_string(),
                    // The error may hold a response body that echoes a token we
                    // sent; this reaches the outside world as `{{error}}`.
                    error: crate::secret::mask(&e.to_string()),
                    ..ctx
                };
                self.maybe_notify(profile, Outcome::Failed, &fail_ctx).await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_ctx(
        &self,
        profile: &str,
        domain: &str,
        rtype: RecordType,
        ip: &str,
        old_ip: &str,
        ttl: u32,
        proxied: bool,
        zone: &str,
        outcome: Outcome,
        error: &str,
    ) -> RenderCtx {
        RenderCtx {
            profile: profile.to_string(),
            domain: domain.to_string(),
            record_type: rtype.as_str().to_string(),
            family: rtype.family().as_str().to_string(),
            ip: ip.to_string(),
            old_ip: old_ip.to_string(),
            ttl,
            proxied,
            zone: zone.to_string(),
            event: outcome.as_str().to_string(),
            // `{{error}}` can be forwarded to another service; a resolver error
            // may echo back a token we sent. Empty strings mask to empty.
            error: crate::secret::mask(error),
            timestamp: crate::now_rfc3339(),
        }
    }

    async fn maybe_notify(&self, profile: &Profile, outcome: Outcome, ctx: &RenderCtx) {
        notify::fire(&self.notify_client, profile.notify.as_ref(), outcome, ctx).await;
    }
}

/// Expand the three domain lists into concrete (domain, record-type) targets.
fn record_targets(targets: &Targets) -> Vec<(String, RecordType)> {
    let mut out = Vec::new();
    for d in &targets.ipv4 {
        out.push((d.clone(), RecordType::A));
    }
    for d in &targets.ipv6 {
        out.push((d.clone(), RecordType::Aaaa));
    }
    for d in &targets.dual {
        out.push((d.clone(), RecordType::A));
        out.push((d.clone(), RecordType::Aaaa));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfig, GenericRequest, SuccessSpec};
    use std::collections::BTreeMap;

    fn targets(ipv4: &[&str], ipv6: &[&str], dual: &[&str]) -> Targets {
        Targets {
            ttl: 300,
            proxied: false,
            provider: ProviderConfig::Generic {
                update: GenericRequest {
                    method: "GET".into(),
                    url: "https://x/update".into(),
                    headers: BTreeMap::new(),
                    body: None,
                    success: SuccessSpec::default(),
                },
            },
            ipv4: ipv4.iter().map(|s| s.to_string()).collect(),
            ipv6: ipv6.iter().map(|s| s.to_string()).collect(),
            dual: dual.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn expands_record_targets() {
        let t = targets(&["a.example.com"], &["b.example.com"], &["c.example.com"]);
        let recs = record_targets(&t);
        assert_eq!(recs.len(), 4);
        assert!(recs.contains(&("a.example.com".into(), RecordType::A)));
        assert!(recs.contains(&("b.example.com".into(), RecordType::Aaaa)));
        assert!(recs.contains(&("c.example.com".into(), RecordType::A)));
        assert!(recs.contains(&("c.example.com".into(), RecordType::Aaaa)));
    }
}
