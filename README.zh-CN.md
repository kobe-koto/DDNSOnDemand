# DDNSOnDemand

[English](README.md) | **简体中文**

一个使用 Rust 编写的动态 DNS（DDNS）客户端。它从可配置的来源解析本机当前的
公网 IPv4/IPv6 地址，并据此更新 DNS 记录；既可作为常驻 **daemon** 运行，也可
以 **oneshot**（运行一次即退出）方式由外部（如 systemd timer）触发。

## 特性

- **两种运行模式**：`daemon`（周期执行）与 `oneshot`（执行一次后退出）。
- **YAML 配置、多 profile**，支持 `${ENV}` / `${ENV:-default}` 环境变量展开。
- **按协议栈分离的 IP 来源池**（`v4` / `v6`），每个池按声明顺序做回退（取第一个
  成功的来源）。HTTP 来源会**绑定到对应协议栈发起请求**，等价于 `curl -4` / `curl -6`。
- **IP 来源**：指定网口的地址、返回纯文本 IP 的 HTTP 接口、或返回 JSON 的 HTTP
  接口（通过键名或 JSON 指针取值）。
- **DNS 提供方**：原生 **Cloudflare**，以及可对接任意 DDNS HTTP API 的**通用**
  模板化 GET/POST 提供方。
- **每个 profile 的目标域名**分为 `ipv4`（A 记录）、`ipv6`（AAAA 记录）、`dual`
  （同时 A + AAAA）三组，每组可配置多个域名。
- **差分更新**：最近一次的 IP 记录在状态文件中，仅在发生变化时才调用提供方与通知。
- **模板化通知**（GET/POST），按可配置的事件触发。

## 构建

```sh
cargo build --release
# 产物位于 target/release/ddnsondemand
```

## 使用

```
ddnsondemand [--config PATH] [--state PATH] [--profile NAME]... [--dry-run] [--force] [-v] <COMMAND>

命令：
  daemon            持续运行，按各 profile 的 interval 周期更新
  oneshot           对所选 profile 各执行一次后退出
  check [--resolve] 校验配置；加 --resolve 时同时解析各 IP 来源

全局选项：
  -c, --config <PATH>    配置文件。默认搜索顺序：./config.yaml、
                         ./ddnsondemand.yaml、$XDG_CONFIG_HOME/ddnsondemand/config.yaml、
                         /etc/ddnsondemand/config.yaml（或环境变量 $DDNSONDEMAND_CONFIG）
      --state <PATH>     状态文件。默认：$XDG_STATE_HOME/ddnsondemand/state.json
      --profile <NAME>   仅处理指定 profile（可重复）
      --dry-run          仅解析并打印将要执行的动作，不调用提供方与通知
      --force            即使状态显示无变化也强制更新
  -v, --verbose          -v = debug，-vv = trace（覆盖配置中的日志级别）
```

示例：

```sh
ddnsondemand -c examples/config.yaml check --resolve
ddnsondemand -c examples/config.yaml oneshot --dry-run
CF_API_TOKEN=... CF_ZONE_ID=... ddnsondemand -c examples/config.yaml oneshot
ddnsondemand -c /etc/ddnsondemand/config.yaml daemon
```

## 配置

完整且带注释的示例见 [`examples/config.yaml`](examples/config.yaml)。要点：

- `profiles.<name>.ip_sources.v4` / `.v6` 是**有序来源池**。所需协议栈由目标域名
  列表**自动推导**（`ipv4`/`dual` → 需要 v4，`ipv6`/`dual` → 需要 v6）；若某个被
  需要的协议栈对应的来源池为空，则视为配置错误。
- 每个 IP 来源都有 `type`：
  - `interface` —— `interface: <网口名>`，可选 `select`（`scope: global|any`、
    `exclude_temporary: bool`）。
  - `http_plain` —— `url:` 返回纯文本 IP。
  - `http_json` —— `url:` 与 `key:`（简单键名如 `ip`，或 JSON 指针 `/data/ip`）。
  - HTTP 来源支持可选的 `headers:` 映射。默认总会带上 `User-Agent`
    （`ddnsondemand/<版本>`）；当某服务按 User-Agent 过滤并返回 403 时，可在此覆盖
    它或追加其他请求头。
- `targets.provider.type` 为 `cloudflare`（`api_token`，以及 `zone_id` **或**
  `zone`）或 `generic`（`update:` 含 `method`、`url`、`headers`、可选 `body`，
  以及 `success` = `status: [..]` 加可选 `body_contains`）。
- `notify.on` 选择事件（`changed`、`success`、`error`、`always`；其中 `unchanged`
  归类为 `success`）；`notify.targets` 为模板化的 GET/POST 调用。

### 模板变量

可用于通用提供方请求及通知的 URL/headers/body：

| 变量              | 含义                                   |
|-------------------|----------------------------------------|
| `{{profile}}`     | profile 名称                           |
| `{{domain}}`      | 目标域名                               |
| `{{record_type}}` | `A` 或 `AAAA`                          |
| `{{family}}`      | `ipv4` 或 `ipv6`                       |
| `{{ip}}`          | 新解析出的 IP                          |
| `{{old_ip}}`      | 先前存储的 IP（可能为空）              |
| `{{ttl}}`         | 配置的 TTL                             |
| `{{proxied}}`     | Cloudflare 的 proxied 标志             |
| `{{zone}}`        | Zone id/名称（尽力而为）               |
| `{{event}}`       | `changed` / `unchanged` / `failed`（通知）|
| `{{error}}`       | 错误信息（失败通知）                   |
| `{{timestamp}}`   | RFC 3339 UTC 时间戳                     |

## systemd

将二进制安装到 `/usr/bin/ddnsondemand`，配置放在 `/etc/ddnsondemand/config.yaml`，
密钥放在 `/etc/ddnsondemand/env`（每行 `KEY=VALUE`）。随后二选一：

- **Daemon**：`examples/ddnsondemand.service`
  ```sh
  systemctl enable --now ddnsondemand.service
  ```
- **Oneshot + timer**：`examples/ddnsondemand-oneshot.service` 与
  `examples/ddnsondemand.timer`
  ```sh
  systemctl enable --now ddnsondemand.timer
  ```

这些 unit 使用 `DynamicUser=yes` + `StateDirectory=ddnsondemand`，因此状态文件
位于 `/var/lib/ddnsondemand/state.json`。

> 通过 `.deb` 安装时，上述 unit 已随包安装到 `/usr/lib/systemd/system/`。

## 说明与限制

- IPv6 的 `exclude_temporary` 为**尽力而为**：`if-addrs` 不暴露 netlink 的
  `IFA_F_TEMPORARY` 标志，因此无法可靠区分临时/隐私地址，该选项目前未强制生效。
- 通用提供方无法区分“已变化”与“本就一致”；引擎仅在状态显示发生变化时才调用它，
  并按成功处理。
- 每个 profile 的各协议栈相互独立解析；某一协议栈失败不会阻塞其他协议栈或其他 profile。

## 许可证

MIT —— 见 [LICENSE](LICENSE)。
