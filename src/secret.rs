//! Process-wide masking of secret values in output.
//!
//! Secrets reach the config through `${ENV}` expansion: a Cloudflare API token,
//! a DDNS update token embedded in a URL, a webhook or bot token. Once
//! expanded, the value can surface far from where it was written:
//! - a serde type error that quotes the offending value (a token written where
//!   a struct was expected),
//! - a provider or IP-source error that echoes our own token back out of a
//!   response body, which the engine also forwards as the `{{error}}`
//!   notification variable,
//! - a log line.
//!
//! Rather than redact at each such site, every `${ENV}`-expanded value is
//! registered here once, right after the config loads, and the few real output
//! boundaries mask it: the log writer (`logging`), `check`'s resolver output and
//! the fatal error line (`main`), the `{{error}}` value placed into notification
//! bodies (`engine`), and config-load errors, which are scrubbed at the source
//! with the per-load list before this global set exists.
//!
//! Only environment-sourced values are masked. A secret written literally in the
//! config file, or one a server returns but we never sent, is not registered, so
//! code must still avoid printing whole response bodies (see `redact` in
//! `http`).

use std::sync::RwLock;

/// Values shorter than this are not masked. Real credentials (API tokens, zone
/// ids, webhook tokens, update tokens) are far longer; the floor keeps a short
/// environment value such as `warn`, `true` or a port number from blanking
/// unrelated text wherever it happens to appear as a substring.
const MIN_LEN: usize = 8;

/// What a masked value is replaced with.
const MASK: &str = "***";

static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Record values to mask out of all later output. Called once, right after the
/// config loads, with every value that `${ENV}` expansion substituted in.
/// Values too short to be a credential, and duplicates, are ignored.
pub fn register(values: &[String]) {
    let mut set = SECRETS.write().unwrap_or_else(|e| e.into_inner());
    for v in values {
        if maskable(v) && !set.iter().any(|s| s == v) {
            set.push(v.clone());
        }
    }
    sort_longest_first(&mut set);
}

/// Replace every registered secret in `s` with `***`. A no-op (one allocation)
/// when nothing is registered, which is the case for an env-free config.
pub fn mask(s: &str) -> String {
    let set = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    replace_all(s, set.iter())
}

/// Replace every maskable value of `secrets` in `s` with `***`. Used for
/// config-load errors, which are scrubbed before the global set exists.
pub fn mask_with(s: &str, secrets: &[String]) -> String {
    let mut list: Vec<&String> = secrets.iter().filter(|v| maskable(v)).collect();
    list.sort_by_key(|v| std::cmp::Reverse(v.len()));
    replace_all(s, list.into_iter())
}

fn replace_all<'a, I: Iterator<Item = &'a String>>(s: &str, secrets: I) -> String {
    let mut out = s.to_string();
    for secret in secrets {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), MASK);
        }
    }
    out
}

fn maskable(v: &str) -> bool {
    v.len() >= MIN_LEN
}

/// Longest first, so a value that contains a shorter registered one is replaced
/// whole instead of being left as a fragment around an inner `***`.
fn sort_longest_first(set: &mut [String]) {
    set.sort_by_key(|v| std::cmp::Reverse(v.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_a_registered_value() {
        assert_eq!(mask_with("t=TOKEN-abcdefgh end", &["TOKEN-abcdefgh".into()]), "t=*** end");
    }

    #[test]
    fn leaves_short_values_alone() {
        // `warn` and `info` are below the floor: masking them would blank them
        // out of ordinary log text.
        let secrets = vec!["warn".into(), "info".into(), "8080".into()];
        assert_eq!(mask_with("level warn on :8080 info", &secrets), "level warn on :8080 info");
    }

    #[test]
    fn replaces_the_longer_value_when_one_contains_another() {
        // Replacing the short one first would leave `prefix-***` and expose that
        // the longer secret started with it.
        let secrets = vec!["prefix-longsecret".into(), "longsecret".into()];
        assert_eq!(mask_with("v=prefix-longsecret", &secrets), "v=***");
    }

    #[test]
    fn empty_list_is_a_no_op() {
        assert_eq!(mask_with("nothing to mask here", &[]), "nothing to mask here");
    }

    #[test]
    fn global_register_then_mask() {
        // A value unique to this test, so other tests sharing the global set do
        // not interfere.
        register(&["UNIQ-global-secret-xyz".into()]);
        assert_eq!(mask("a UNIQ-global-secret-xyz b"), "a *** b");
    }
}
