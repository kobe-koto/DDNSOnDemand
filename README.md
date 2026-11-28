# DDNSOnDemand

**English** | [简体中文](README.zh-CN.md)

A dynamic-DNS (DDNS) client in Rust. It resolves your current public IPv4/IPv6
addresses from configurable sources and keeps DNS records up to date, either as
a long-running **daemon** or as a **one-shot** run driven by an external
trigger such as a systemd timer.

## Features

- **Two run modes**: `daemon` (periodic) and `oneshot` (run once, exit).
- **YAML config, multiple profiles**, with `${ENV}` / `${ENV:-default}` expansion.
- **Per-family IP source pools** (`v4` / `v6`), each a declaration-order
  fallback chain. HTTP sources are requested bound to the matching stack,
  equivalent to `curl -4` / `curl -6`.
- **IP sources**: a local network interface, an HTTP endpoint returning a bare
  IP, or an HTTP endpoint returning JSON (value selected by key or JSON pointer).
- **Providers**: native **Cloudflare**, plus a **generic** templated GET/POST
  provider for any other DDNS HTTP API.
- **Targets per profile** split into `ipv4` (A), `ipv6` (AAAA) and `dual`
  (both), each accepting multiple domains.
- **Differential updates**: last-known IPs are stored in a state file; providers
  and notifications fire only when something changes.
- **Templated notifications** (GET/POST) on configurable events.

## Install

Each tagged release publishes prebuilt x86_64 Linux artifacts:

- `…-x86_64-unknown-linux-gnu.tar.gz` — dynamically linked (glibc) binary.
- `…-x86_64-unknown-linux-musl.tar.gz` — fully static (musl) binary, portable
  across distros.
- `ddnsondemand_<version>_amd64.deb` — Debian/Ubuntu package (systemd units).
- `ddnsondemand-<version>-r0.apk` — Alpine package (OpenRC service); install with
  `apk add --allow-untrusted ddnsondemand-*.apk`.

Each file has a `.sha256` sidecar.

## Build

Requires Rust 1.88 or newer.

```sh
cargo build --release
# binary at target/release/ddnsondemand
```

## Usage

```
ddnsondemand [--config PATH] [--state PATH] [--profile NAME]... [--dry-run] [--force] [-v] <COMMAND>

Commands:
  daemon            Run continuously, updating each profile on its interval
  oneshot           Process every selected profile once, then exit
  check [--resolve] Validate config; with --resolve, also resolve IP sources

Global options:
  -c, --config <PATH>    Config file. Default search: ./config.yaml,
                         ./ddnsondemand.yaml, $XDG_CONFIG_HOME/ddnsondemand/config.yaml,
                         /etc/ddnsondemand/config.yaml  (or $DDNSONDEMAND_CONFIG)
      --state <PATH>     State file. Default: $XDG_STATE_HOME/ddnsondemand/state.json
      --profile <NAME>   Restrict to named profile(s); repeatable
      --dry-run          Resolve + log intended actions; no provider/notify calls
      --force            Update even when the state shows no change
  -v, --verbose          -v = debug, -vv = trace (overrides config log level)
```

Examples:

```sh
ddnsondemand -c examples/config.yaml check --resolve
ddnsondemand -c examples/config.yaml oneshot --dry-run
CF_API_TOKEN=... CF_ZONE_ID=... ddnsondemand -c examples/config.yaml oneshot
ddnsondemand -c /etc/ddnsondemand/config.yaml daemon
```

## Configuration

See [`examples/config.yaml`](examples/config.yaml) for a complete, commented
example. A JSON Schema is provided at
[`schema/config.schema.json`](schema/config.schema.json) for editor validation
and autocompletion — the example config already references it via a
`# yaml-language-server: $schema=...` modeline on its first line. Key points:

- `profiles.<name>.ip_sources.v4` / `.v6` are **ordered pools**. The required
  families are **derived from the target domain lists** (`ipv4`/`dual` → v4,
  `ipv6`/`dual` → v6); a required family with an empty pool is a config error.
- Each IP source has a `type`:
  - `interface` — `interface: <name>`, optional `select` (`scope: global|any`,
    `exclude_temporary: bool`).
  - `http_plain` — `url:` returning a bare IP.
  - `http_json` — `url:` and `key:` (simple key like `ip`, or JSON pointer `/data/ip`).
  - HTTP sources accept an optional `headers:` map. A default `User-Agent`
    (`ddnsondemand/<version>`) is always sent; override it (or add other
    headers) when a service filters by User-Agent and returns 403.
- `targets.provider.type` is `cloudflare` (`api_token`, and `zone_id` **or**
  `zone`) or `generic` (`update:` with `method`, `url`, `headers`, optional
  `body`, and `success` = `status: [..]` + optional `body_contains`).
- `notify.on` selects events (`changed`, `success`, `unchanged` is reported as
  `success`, `error`, `always`); `notify.targets` are templated GET/POST calls.

### Template variables

Available in generic-provider requests and notification URL/headers/body:

| Variable        | Meaning                                   |
|-----------------|-------------------------------------------|
| `{{profile}}`   | Profile name                              |
| `{{domain}}`    | Target domain                             |
| `{{record_type}}` | `A` or `AAAA`                           |
| `{{family}}`    | `ipv4` or `ipv6`                          |
| `{{ip}}`        | Newly resolved IP                         |
| `{{old_ip}}`    | Previously stored IP (may be empty)       |
| `{{ttl}}`       | Configured TTL                            |
| `{{proxied}}`   | Cloudflare proxied flag                   |
| `{{zone}}`      | Zone id/name (best-effort)                |
| `{{event}}`     | `changed` / `unchanged` / `failed` (notifications) |
| `{{error}}`     | Error message (failure notifications)     |
| `{{timestamp}}` | RFC 3339 UTC timestamp                    |

Templates are plain substitution. `{{ name }}` is replaced by the variable's
value (spaces inside the braces are optional), and one trailing newline is
dropped, so a YAML `|` block does not send it. `{{proxied}}` renders as
`True` / `False`. Anything else is rejected when the config is loaded, so
`ddnsondemand check` reports it: unknown names, filters such as
`{{ ip | upper }}`, and `{% %}` / `{# #}` tags.

## systemd

Install the binary to `/usr/bin/ddnsondemand`, put your config at
`/etc/ddnsondemand/config.yaml` and secrets in `/etc/ddnsondemand/env`
(`KEY=VALUE` lines). Then use either:

- **Daemon**: `examples/ddnsondemand.service`
  ```sh
  systemctl enable --now ddnsondemand.service
  ```
- **One-shot + timer**: `examples/ddnsondemand-oneshot.service` and
  `examples/ddnsondemand.timer`
  ```sh
  systemctl enable --now ddnsondemand.timer
  ```

The units use `DynamicUser=yes` + `StateDirectory=ddnsondemand`, so state lives
at `/var/lib/ddnsondemand/state.json`.

## Notes & limitations

- IPv6 `exclude_temporary` is **best-effort**: `if-addrs` does not expose
  netlink `IFA_F_TEMPORARY` flags, so temporary/privacy addresses cannot be
  reliably distinguished and the option is currently not enforced.
- The generic provider cannot distinguish "changed" from "already current"; the
  engine only calls it when the state indicates a change, and reports success.
- Each profile's families are resolved independently; a failure in one family
  does not block the others or other profiles.
- When a request fails, its URL is shown only as `scheme://host[:port]/...` in
  log lines, error messages and the `{{error}}` template variable. That keeps
  tokens in query strings, webhook paths or `user:password@` out of logs and
  out of failure notifications. Template errors do not quote the template
  either. Give notify targets a `name` to tell them apart in logs.
- Any value pulled in from the environment through `${VAR}` is masked to `***`
  wherever it would otherwise be printed: logs, error messages, `check` output,
  and the `{{error}}` forwarded to notifications. This catches a token a server
  echoes back inside a response body, and a config value repeated in a type
  error. A secret written **literally** in the config file (not via `${VAR}`),
  and any other response-body content, is **not** masked, so keep secrets in the
  environment and `${VAR}` them in.

## License

MIT — see [LICENSE](LICENSE).
