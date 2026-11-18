//! Read an address off a local network interface via `if-addrs`.

use std::net::IpAddr;

use anyhow::{anyhow, Result};
use tracing::debug;

use super::Family;
use crate::config::{Scope, SelectConfig};

pub fn resolve_interface(name: &str, family: Family, select: &SelectConfig) -> Result<IpAddr> {
    if select.exclude_temporary {
        debug!(
            interface = name,
            "exclude_temporary is best-effort: if-addrs does not expose IFA_F_TEMPORARY, so it is not enforced"
        );
    }
    let ifaces = if_addrs::get_if_addrs()?;
    let mut found_iface = false;

    let candidate = ifaces
        .into_iter()
        .filter(|i| {
            if i.name == name {
                found_iface = true;
                true
            } else {
                false
            }
        })
        .map(|i| i.ip())
        .filter(|ip| family.matches(ip))
        .find(|ip| is_acceptable(ip, select));

    match candidate {
        Some(ip) => {
            debug!(%ip, interface = name, "selected interface address");
            Ok(ip)
        }
        None if !found_iface => Err(anyhow!("interface '{name}' not found")),
        None => Err(anyhow!(
            "interface '{name}' has no acceptable {family} address"
        )),
    }
}

/// Decide whether an address passes the selection filter.
fn is_acceptable(ip: &IpAddr, select: &SelectConfig) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_unspecified() {
                return false;
            }
            if v4.is_link_local() {
                return false; // 169.254.0.0/16
            }
            true
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return false;
            }
            // Link-local fe80::/10
            let seg0 = v6.segments()[0];
            if (seg0 & 0xffc0) == 0xfe80 {
                return false;
            }
            if select.scope == Scope::Global {
                // Unique-local fc00::/7
                if (seg0 & 0xfe00) == 0xfc00 {
                    return false;
                }
            }
            // `exclude_temporary` cannot be honoured through if-addrs, which does
            // not expose IFA_F_TEMPORARY flags; documented as best-effort.
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn sel(scope: Scope) -> SelectConfig {
        SelectConfig {
            scope,
            exclude_temporary: false,
        }
    }

    #[test]
    fn rejects_v4_link_local_and_loopback() {
        assert!(!is_acceptable(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)), &sel(Scope::Global)));
        assert!(!is_acceptable(&IpAddr::V4(Ipv4Addr::LOCALHOST), &sel(Scope::Global)));
        assert!(is_acceptable(&IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)), &sel(Scope::Global)));
    }

    #[test]
    fn v6_global_rejects_ula_and_link_local() {
        let ula: Ipv6Addr = "fd00::1".parse().unwrap();
        let ll: Ipv6Addr = "fe80::1".parse().unwrap();
        let global: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert!(!is_acceptable(&IpAddr::V6(ula), &sel(Scope::Global)));
        assert!(!is_acceptable(&IpAddr::V6(ll), &sel(Scope::Global)));
        assert!(is_acceptable(&IpAddr::V6(global), &sel(Scope::Global)));
        // Scope::Any still accepts ULA but not link-local.
        assert!(is_acceptable(&IpAddr::V6(ula), &sel(Scope::Any)));
        assert!(!is_acceptable(&IpAddr::V6(ll), &sel(Scope::Any)));
    }
}
