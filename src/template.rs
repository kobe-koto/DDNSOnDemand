//! Template rendering for provider and notification requests.
//!
//! Templates use plain `{{ name }}` substitution. Whitespace inside the braces
//! is optional, and one trailing newline is dropped, as in Jinja. Values render
//! as Jinja rendered them: `proxied` comes out as `True` / `False`. There are
//! no filters, expressions, `{% %}` blocks or `{# #}` comments. These, and any
//! placeholder that is not a known variable name, are errors rather than empty
//! or literal output. That way a Jinja-style template cannot silently send,
//! say, an empty IP to a provider.

use std::fmt::Write as _;

use anyhow::{bail, Context, Result};

/// Names accepted inside `{{ }}`, in the order the README lists them.
const VARIABLES: &[&str] = &[
    "profile", "domain", "record_type", "family", "ip", "old_ip", "ttl", "proxied", "zone",
    "event", "error", "timestamp",
];

/// Variables exposed to provider/notification templates.
#[derive(Debug, Clone, Default)]
pub struct RenderCtx {
    pub profile: String,
    pub domain: String,
    /// `A` or `AAAA`.
    pub record_type: String,
    /// `ipv4` or `ipv6`.
    pub family: String,
    pub ip: String,
    pub old_ip: String,
    pub ttl: u32,
    pub proxied: bool,
    pub zone: String,
    /// Notification event label (`changed` / `unchanged` / `failed`); empty for providers.
    pub event: String,
    /// Error message for failure notifications; empty otherwise.
    pub error: String,
    pub timestamp: String,
}

impl RenderCtx {
    /// Append the value of variable `name` to `out`. Returns false, appending
    /// nothing, if `name` is not a known variable.
    fn write_var(&self, name: &str, out: &mut String) -> bool {
        let value = match name {
            "profile" => &self.profile,
            "domain" => &self.domain,
            "record_type" => &self.record_type,
            "family" => &self.family,
            "ip" => &self.ip,
            "old_ip" => &self.old_ip,
            "zone" => &self.zone,
            "event" => &self.event,
            "error" => &self.error,
            "timestamp" => &self.timestamp,
            "ttl" => {
                let _ = write!(out, "{}", self.ttl);
                return true;
            }
            // Jinja's spelling, kept so existing templates render the same.
            "proxied" => {
                out.push_str(if self.proxied { "True" } else { "False" });
                return true;
            }
            _ => return false,
        };
        out.push_str(value);
        true
    }
}

/// Render a single template string against the context.
///
/// Errors never quote `source`. By now `${ENV}` references in it are expanded,
/// so it can hold a token. The callers' context names the field instead.
pub fn render(source: &str, ctx: &RenderCtx) -> Result<String> {
    render_inner(source, ctx).context("rendering template")
}

/// Check that `source` would render. Used at config load, so that `check`
/// and startup reject a bad template instead of the first update failing.
pub fn validate(source: &str) -> Result<()> {
    render_inner(source, &RenderCtx::default())
        .map(drop)
        .context("invalid template")
}

fn render_inner(source: &str, ctx: &RenderCtx) -> Result<String> {
    // Drop one trailing newline, as Jinja does: first "\n", then "\r". This
    // matters for YAML `|` blocks, which always end in a newline.
    let source = source.strip_suffix('\n').unwrap_or(source);
    let source = source.strip_suffix('\r').unwrap_or(source);

    // Jinja parsed these as block/comment tags, so a working template can never
    // contain them literally. Passing them through would send the raw tag.
    if let Some(tag) = ["{%", "{#"].into_iter().find(|tag| source.contains(tag)) {
        bail!("'{tag}' blocks are not supported; only {{{{ name }}}} substitution is");
    }

    let mut out = String::with_capacity(source.len() + 32);
    let mut rest = source;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            bail!("unterminated '{{{{' (missing '}}}}')");
        };
        let inner = &after[..end];
        if !ctx.write_var(inner.trim(), &mut out) {
            let supported = VARIABLES.join(", ");
            if quotable(inner) {
                bail!("unsupported placeholder '{{{{{inner}}}}}': only {{{{ name }}}} with one of {supported} is supported");
            }
            bail!(
                "unsupported placeholder (not quoted: it runs over other text, so check for a missing \
                 or extra brace): only {{{{ name }}}} with one of {supported} is supported"
            );
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Whether a rejected placeholder may be quoted in an error message.
///
/// The template has `${ENV}` values expanded, and a brace typo makes the
/// "placeholder" run on to the next `}}` across literal text, as in
/// `?ip={{ip}&token=<secret>&host={{domain}}`. Quote only what looks like a
/// Jinja expression (`ip | upper`, `- ip -`, `ipv4`): short, short words, and
/// no braces or URL punctuation.
fn quotable(inner: &str) -> bool {
    inner.len() <= 40
        && inner.split_whitespace().all(|word| word.len() <= 16)
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || " _|.,()~'\"+-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RenderCtx {
        RenderCtx {
            profile: "home".into(),
            domain: "a.example.com".into(),
            record_type: "A".into(),
            family: "ipv4".into(),
            ip: "1.2.3.4".into(),
            old_ip: "1.1.1.1".into(),
            ttl: 300,
            proxied: false,
            zone: "example.com".into(),
            event: "changed".into(),
            error: String::new(),
            timestamp: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn renders_variables() {
        let out = render("host={{domain}}&ip={{ip}}&t={{record_type}}", &ctx()).unwrap();
        assert_eq!(out, "host=a.example.com&ip=1.2.3.4&t=A");
    }

    #[test]
    fn renders_json_body() {
        let out =
            render(r#"{"ip":"{{ip}}","old":"{{old_ip}}","event":"{{event}}"}"#, &ctx()).unwrap();
        assert_eq!(out, r#"{"ip":"1.2.3.4","old":"1.1.1.1","event":"changed"}"#);
    }

    #[test]
    fn renders_every_variable() {
        let mut c = ctx();
        c.proxied = true;
        c.error = "boom".into();
        let out = render(
            "{{profile}}|{{domain}}|{{record_type}}|{{family}}|{{ip}}|{{old_ip}}|{{ttl}}|{{proxied}}|{{zone}}|{{event}}|{{error}}|{{timestamp}}",
            &c,
        )
        .unwrap();
        assert_eq!(
            out,
            "home|a.example.com|A|ipv4|1.2.3.4|1.1.1.1|300|True|example.com|changed|boom|2026-01-01T00:00:00Z"
        );
        c.proxied = false;
        assert_eq!(render("{{proxied}}", &c).unwrap(), "False");
    }

    #[test]
    fn variables_list_matches_context() {
        let c = RenderCtx::default();
        for name in VARIABLES {
            assert!(c.write_var(name, &mut String::new()), "{name} not handled");
        }
    }

    #[test]
    fn allows_whitespace_inside_braces() {
        assert_eq!(render("{{ ip }}-{{ip }}-{{\tip\t}}", &ctx()).unwrap(), "1.2.3.4-1.2.3.4-1.2.3.4");
    }

    #[test]
    fn rejects_unknown_names_and_jinja_syntax() {
        for t in ["{{ nope }}", "{{ ip | upper }}", "{{- ip -}}", "{{ ip ~ 'x' }}", "{{ 'a' }}", "{{{ip}}}", "{{}}"] {
            let err = render(t, &ctx()).unwrap_err();
            assert!(format!("{err:#}").contains("unsupported placeholder"), "{t}: {err:#}");
            assert!(validate(t).is_err(), "{t}");
        }
        for t in ["{{ip}} {% if proxied %}P{% endif %}", "{# note #}{{ip}}"] {
            let err = render(t, &ctx()).unwrap_err();
            assert!(format!("{err:#}").contains("blocks are not supported"), "{t}: {err:#}");
            assert!(validate(t).is_err(), "{t}");
        }
    }

    #[test]
    fn does_not_quote_a_placeholder_that_runs_over_other_text() {
        // A missing '}' after `ip` makes the placeholder reach the next "}}",
        // across the (expanded) token.
        for t in [
            "https://x/u?ip={{ip}&token=S3CR3T&host={{domain}}",
            "https://x/u?ip={{ip&token=S3CR3T}}",
            "Bearer {{ip S3CR3TS3CR3TS3CR3TS3CR3T}}",
        ] {
            let err = format!("{:#}", render(t, &ctx()).unwrap_err());
            assert!(!err.contains("S3CR3T"), "{t}: {err}");
            assert!(err.contains("missing or extra brace"), "{t}: {err}");
        }
        // Expression-like placeholders are still quoted, which helps locate them.
        let err = format!("{:#}", render("x={{ ip | replace('.', '-') }}", &ctx()).unwrap_err());
        assert!(err.contains("'{{ ip | replace('.', '-') }}'"), "{err}");
    }

    #[test]
    fn validates_good_templates() {
        validate("https://x/u?h={{ domain }}&ip={{ip}}&p={{proxied}}\n").unwrap();
        validate("no placeholders { } }}").unwrap();
        assert!(validate("x={{ip").is_err());
    }

    #[test]
    fn keeps_literal_braces() {
        // Nested JSON objects close with "}}", which is only special after "{{".
        let out = render(r#"{"a":{"b":"{{ip}}"}}"#, &ctx()).unwrap();
        assert_eq!(out, r#"{"a":{"b":"1.2.3.4"}}"#);
        assert_eq!(render("{ x } }} %} #}", &ctx()).unwrap(), "{ x } }} %} #}");
    }

    #[test]
    fn drops_one_trailing_newline() {
        assert_eq!(render("{{ip}}\n", &ctx()).unwrap(), "1.2.3.4");
        assert_eq!(render("{{ip}}\r\n", &ctx()).unwrap(), "1.2.3.4");
        assert_eq!(render("{{ip}}\n\n", &ctx()).unwrap(), "1.2.3.4\n");
        assert_eq!(render("a\nb", &ctx()).unwrap(), "a\nb");
    }

    #[test]
    fn keeps_non_ascii_text() {
        assert_eq!(render("域名={{domain}}，完成", &ctx()).unwrap(), "域名=a.example.com，完成");
    }

    #[test]
    fn unterminated_placeholder_errors() {
        let err = render("x={{ip", &ctx()).unwrap_err();
        assert!(format!("{err:#}").contains("unterminated"));
    }
}
