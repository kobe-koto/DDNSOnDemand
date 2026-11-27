//! Configuration model, YAML loading, `${ENV}` expansion and validation.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Top-level configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub defaults: Defaults,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_level")]
    pub level: String,
    #[serde(default)]
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_level(),
            format: LogFormat::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    #[serde(with = "humantime_serde", default = "default_interval")]
    pub interval: Duration,
    #[serde(with = "humantime_serde", default = "default_timeout")]
    pub http_timeout: Duration,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            interval: default_interval(),
            http_timeout: default_timeout(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Daemon loop interval; overrides `defaults.interval`.
    #[serde(default, with = "humantime_serde")]
    pub interval: Option<Duration>,
    pub ip_sources: IpSources,
    pub targets: Targets,
    #[serde(default)]
    pub notify: Option<NotifyConfig>,
}

impl Profile {
    /// Which address families this profile needs to resolve, derived from the
    /// configured domain lists.
    pub fn required_families(&self) -> (bool, bool) {
        let need_v4 = !self.targets.ipv4.is_empty() || !self.targets.dual.is_empty();
        let need_v6 = !self.targets.ipv6.is_empty() || !self.targets.dual.is_empty();
        (need_v4, need_v6)
    }
}

/// Per-family ordered pools of IP sources (declaration-order fallback).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IpSources {
    #[serde(default)]
    pub v4: Vec<IpSourceConfig>,
    #[serde(default)]
    pub v6: Vec<IpSourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpSourceConfig {
    /// Read an address off a local network interface.
    Interface {
        interface: String,
        #[serde(default)]
        select: SelectConfig,
    },
    /// GET a URL that returns a bare IP address as plain text.
    HttpPlain {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// GET a URL that returns JSON; extract the IP from `key`.
    HttpJson {
        url: String,
        /// Simple key (`ip`) or a JSON pointer (`/data/ip`).
        key: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SelectConfig {
    #[serde(default)]
    pub scope: Scope,
    /// Best-effort exclusion of IPv6 temporary/privacy addresses.
    #[serde(default)]
    pub exclude_temporary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Skip loopback, link-local and (for IPv6) unique-local addresses.
    #[default]
    Global,
    /// Accept any address except loopback/unspecified.
    Any,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Targets {
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    #[serde(default)]
    pub proxied: bool,
    pub provider: ProviderConfig,
    /// A records.
    #[serde(default)]
    pub ipv4: Vec<String>,
    /// AAAA records.
    #[serde(default)]
    pub ipv6: Vec<String>,
    /// Both A and AAAA records.
    #[serde(default)]
    pub dual: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    Cloudflare {
        api_token: String,
        #[serde(default)]
        zone_id: Option<String>,
        #[serde(default)]
        zone: Option<String>,
    },
    Generic {
        update: GenericRequest,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenericRequest {
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub success: SuccessSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SuccessSpec {
    #[serde(default = "default_success_status")]
    pub status: Vec<u16>,
    #[serde(default)]
    pub body_contains: Option<String>,
}

impl Default for SuccessSpec {
    fn default() -> Self {
        Self {
            status: default_success_status(),
            body_contains: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotifyConfig {
    #[serde(rename = "on", default)]
    pub on: Vec<Event>,
    #[serde(default)]
    pub targets: Vec<NotifyTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Changed,
    Success,
    Error,
    Always,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotifyTarget {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

// ---- serde defaults ----

fn default_level() -> String {
    "info".to_string()
}
fn default_interval() -> Duration {
    Duration::from_secs(300)
}
fn default_timeout() -> Duration {
    Duration::from_secs(10)
}
fn default_true() -> bool {
    true
}
fn default_ttl() -> u32 {
    300
}
fn default_method() -> String {
    "GET".to_string()
}
fn default_success_status() -> Vec<u16> {
    vec![200]
}

// ---- loading ----

/// Load, expand `${ENV}` references and validate a configuration file.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    load_str(&text)
}

/// Same as [`load`] but from an in-memory string (used by tests).
pub fn load_str(text: &str) -> Result<Config> {
    let mut value: serde_norway::Value =
        serde_norway::from_str(text).context("parsing YAML config")?;
    expand_env(&mut value)?;
    let config: Config =
        serde_norway::from_value(value).context("deserializing config structure")?;
    config.validate()?;
    Ok(config)
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.profiles.is_empty() {
            bail!("config has no profiles");
        }
        for (name, profile) in &self.profiles {
            let (need_v4, need_v6) = profile.required_families();
            if !need_v4 && !need_v6 {
                bail!("profile '{name}' has no target domains (ipv4/ipv6/dual all empty)");
            }
            if need_v4 && profile.ip_sources.v4.is_empty() {
                bail!("profile '{name}' needs IPv4 (A/dual domains) but ip_sources.v4 is empty");
            }
            if need_v6 && profile.ip_sources.v6.is_empty() {
                bail!("profile '{name}' needs IPv6 (AAAA/dual domains) but ip_sources.v6 is empty");
            }
            if let ProviderConfig::Cloudflare { zone_id, zone, .. } = &profile.targets.provider {
                if zone_id.is_none() && zone.is_none() {
                    bail!("profile '{name}' cloudflare provider needs either 'zone_id' or 'zone'");
                }
            }
            if let ProviderConfig::Generic { update } = &profile.targets.provider {
                validate_request_templates(&update.url, &update.headers, update.body.as_deref())
                    .with_context(|| format!("profile '{name}' generic provider"))?;
            }
            for target in profile.notify.iter().flat_map(|n| &n.targets) {
                validate_request_templates(&target.url, &target.headers, target.body.as_deref())
                    .with_context(|| {
                        let label = crate::notify::target_label(target);
                        format!("profile '{name}' notify target '{label}'")
                    })?;
            }
        }
        Ok(())
    }
}

/// Check every templated field of an outbound request. Header names are not
/// templated, only their values.
fn validate_request_templates(
    url: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&str>,
) -> Result<()> {
    crate::template::validate(url).context("url")?;
    for (key, value) in headers {
        crate::template::validate(value).with_context(|| format!("header '{key}'"))?;
    }
    if let Some(body) = body {
        crate::template::validate(body).context("body")?;
    }
    Ok(())
}

/// Recursively expand `${VAR}` and `${VAR:-default}` inside every string value.
fn expand_env(value: &mut serde_norway::Value) -> Result<()> {
    use serde_norway::Value;
    match value {
        Value::String(s) => {
            *s = expand_str(s)?;
        }
        Value::Sequence(seq) => {
            for v in seq {
                expand_env(v)?;
            }
        }
        Value::Mapping(map) => {
            for (_k, v) in map.iter_mut() {
                expand_env(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Expand `${VAR}` / `${VAR:-default}` occurrences in a single string.
fn expand_str(input: &str) -> Result<String> {
    // Copy the text between references as whole &str slices. "${" and "}" are
    // ASCII, so every slice boundary falls on a char boundary and non-ASCII
    // text passes through intact.
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            bail!("unterminated '${{' in config value: {input}");
        };
        let inner = &after[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        match std::env::var(name) {
            Ok(v) => out.push_str(&v),
            Err(_) => match default {
                Some(d) => out.push_str(d),
                None => bail!("environment variable '{name}' is not set (referenced in config)"),
            },
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_env_with_default() {
        std::env::remove_var("DDNS_TEST_MISSING");
        assert_eq!(expand_str("x=${DDNS_TEST_MISSING:-fallback}").unwrap(), "x=fallback");
    }

    #[test]
    fn expands_env_present() {
        std::env::set_var("DDNS_TEST_PRESENT", "abc");
        assert_eq!(expand_str("t=${DDNS_TEST_PRESENT}").unwrap(), "t=abc");
    }

    #[test]
    fn missing_env_without_default_errors() {
        std::env::remove_var("DDNS_TEST_NO_DEFAULT");
        assert!(expand_str("${DDNS_TEST_NO_DEFAULT}").is_err());
    }

    #[test]
    fn expands_env_keeps_non_ascii_text() {
        std::env::set_var("DDNS_TEST_UTF8", "值");
        std::env::remove_var("DDNS_TEST_UTF8_MISSING");
        assert_eq!(
            expand_str("域名=${DDNS_TEST_UTF8}，缺省=${DDNS_TEST_UTF8_MISSING:-默认}。").unwrap(),
            "域名=值，缺省=默认。"
        );
        assert_eq!(expand_str("é ü ✓ 🙂").unwrap(), "é ü ✓ 🙂");
    }

    #[test]
    fn load_keeps_non_ascii_config_strings() {
        // Every string value (mapping keys are not expanded) goes through
        // ${ENV} expansion, not only the ones that contain a reference.
        let yaml = r#"
profiles:
  home:
    ip_sources:
      v4: [{ type: http_plain, url: "https://example.com/ip" }]
    targets:
      provider:
        type: generic
        update:
          url: "https://example.com/update?ip={{ip}}"
      ipv4: [home.example.com]
    notify:
      on: [changed]
      targets:
        - name: 通知
          url: "https://example.com/hook"
          body: "域名={{domain}}，IP={{ip}}"
"#;
        let cfg = load_str(yaml).unwrap();
        let target = &cfg.profiles["home"].notify.as_ref().unwrap().targets[0];
        assert_eq!(target.name.as_deref(), Some("通知"));
        assert_eq!(target.body.as_deref(), Some("域名={{domain}}，IP={{ip}}"));
    }

    #[test]
    fn derives_required_families_and_validates_pools() {
        let yaml = r#"
profiles:
  home:
    ip_sources:
      v4:
        - type: http_plain
          url: https://example.com/ip
    targets:
      provider:
        type: generic
        update:
          url: "https://example.com/update?ip={{ip}}"
      ipv4:
        - home.example.com
"#;
        let cfg = load_str(yaml).unwrap();
        let p = &cfg.profiles["home"];
        assert_eq!(p.required_families(), (true, false));
    }

    #[test]
    fn unsupported_template_fails_validation() {
        let provider = r#"
profiles:
  home:
    ip_sources:
      v4: [{ type: http_plain, url: "https://example.com/ip" }]
    targets:
      provider:
        type: generic
        update:
          url: "https://example.com/update?ip={{ ip | trim }}"
      ipv4: [home.example.com]
"#;
        let err = load_str(provider).unwrap_err();
        assert!(format!("{err:#}").contains("generic provider: url"), "{err:#}");

        let notify = r#"
profiles:
  home:
    ip_sources:
      v4: [{ type: http_plain, url: "https://example.com/ip" }]
    targets:
      provider:
        type: generic
        update:
          url: "https://example.com/update?ip={{ip}}"
      ipv4: [home.example.com]
    notify:
      on: [changed]
      targets:
        - name: hook
          url: "https://example.com/hook"
          headers: { X-Ip: "{{ ipaddr }}" }
"#;
        let err = load_str(notify).unwrap_err();
        assert!(format!("{err:#}").contains("notify target 'hook': header 'X-Ip'"), "{err:#}");
    }

    #[test]
    fn template_errors_do_not_echo_expanded_secrets() {
        std::env::set_var("DDNS_TEST_TOKEN", "s3cr3t-token");
        let yaml = r#"
profiles:
  home:
    ip_sources:
      v4: [{ type: http_plain, url: "https://example.com/ip" }]
    targets:
      provider:
        type: generic
        update:
          url: "https://dyn.example.net/u?token=${DDNS_TEST_TOKEN}&ip={{ ip | trim }}"
      ipv4: [home.example.com]
"#;
        let err = format!("{:#}", load_str(yaml).unwrap_err());
        assert!(!err.contains("s3cr3t-token"), "{err}");
        // Still says where the problem is and what it is.
        assert!(err.contains("profile 'home' generic provider: url: invalid template"), "{err}");
        assert!(err.contains("{{ ip | trim }}"), "{err}");

        // A brace typo makes the reported placeholder run over the token.
        let typo = yaml.replace("&ip={{ ip | trim }}", "&ip={{ip}&t=${DDNS_TEST_TOKEN}&h={{domain}}");
        let err = format!("{:#}", load_str(&typo).unwrap_err());
        assert!(!err.contains("s3cr3t-token"), "{err}");
        assert!(err.contains("generic provider: url: invalid template"), "{err}");

        // A notify target without a name is labelled by its redacted URL.
        let yaml = r#"
profiles:
  home:
    ip_sources:
      v4: [{ type: http_plain, url: "https://example.com/ip" }]
    targets:
      provider:
        type: generic
        update:
          url: "https://dyn.example.net/u?ip={{ip}}"
      ipv4: [home.example.com]
    notify:
      on: [error]
      targets:
        - url: "https://hooks.example.com/services/${DDNS_TEST_TOKEN}"
          body: "{{ nope }}"
"#;
        let err = format!("{:#}", load_str(yaml).unwrap_err());
        assert!(!err.contains("s3cr3t-token"), "{err}");
        assert!(err.contains("notify target 'https://hooks.example.com/...': body"), "{err}");
    }

    #[test]
    fn missing_pool_fails_validation() {
        let yaml = r#"
profiles:
  home:
    ip_sources: {}
    targets:
      provider:
        type: generic
        update:
          url: "https://example.com/update?ip={{ip}}"
      ipv6:
        - home.example.com
"#;
        assert!(load_str(yaml).is_err());
    }
}
