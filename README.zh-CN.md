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

## 安装

每个打了 tag 的 release 都会发布预编译的 x86_64 Linux 产物：

- `…-x86_64-unknown-linux-gnu.tar.gz` —— 动态链接（glibc）二进制。
- `…-x86_64-unknown-linux-musl.tar.gz` —— 完全静态（musl）二进制，跨发行版可移植。
- `ddnsondemand_<版本>_amd64.deb` —— Debian/Ubuntu 包（含 systemd unit）。
- `ddnsondemand-<版本>-r0.apk` —— Alpine 包（含 OpenRC 服务）；安装：
  `apk add --allow-untrusted ddnsondemand-*.apk`。

每个文件都附带 `.sha256` 校验文件。

## 构建

需要 Rust 1.88 或更高版本。

```sh
cargo build --release
# 产物位于 target/release/ddnsondemand
```

## 占用

面向长期运行的小型部署：

- **体积**（release 配置：`opt-level = "z"`、LTO、`codegen-units = 1`、
  `panic = "abort"`、已 strip）：约 **2.7 MiB**。musl 版本是完全静态的位置无关
  可执行文件——目标机上无需 libc 或其他共享库——重定位以 RELR 打包；glibc 版本
  为动态链接。
- **内存**：异步运行时为单线程（`current_thread`），因此守护进程只占用
  **一个系统线程**。在 musl 静态版本上，一个空闲 profile 在完成一次解析（加载
  TLS 栈）后约占 **1.5 MiB PSS**（≈3–4 MiB RSS）。每多一个 profile 只是多一个
  轻量异步任务，而非线程。

以上数据基于 x86-64、关闭透明大页（THP）；若 THP 保持 `always`，RSS 会增加约
1 MiB。可用 `cat /proc/$(pidof ddnsondemand)/smaps_rollup` 测量你自己的目标。

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

完整且带注释的示例见 [`examples/config.yaml`](examples/config.yaml)。另提供 JSON
Schema [`schema/config.schema.json`](schema/config.schema.json) 供编辑器校验与
自动补全——示例配置首行已通过 `# yaml-language-server: $schema=...` modeline 引用
它。要点：

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

模板只做简单替换：`{{ name }}`（花括号内的空格可有可无）会被替换为对应变量的值，
末尾的一个换行会被去掉（因此 YAML `|` 块不会多发一个换行）。`{{proxied}}` 渲染为
`True` / `False`。其他写法，包括未知变量名、`{{ ip | upper }}` 这类过滤器，以及
`{% %}` / `{# #}` 标签，都会在加载配置时报错，用 `ddnsondemand check` 即可发现。

## systemd

将二进制安装到 `/usr/bin/ddnsondemand`，配置放在 `/etc/ddnsondemand/config.yaml`，
密钥放在 `/etc/ddnsondemand/env`（每行 `KEY=VALUE`）。随后任选其一：

- **Daemon**（所有 profile 跑在同一进程）：`examples/ddnsondemand.service`
  ```sh
  systemctl enable --now ddnsondemand.service
  ```
- **Daemon，每个 profile 一个服务**：`examples/ddnsondemand@.service`
  ```sh
  systemctl enable --now ddnsondemand@home ddnsondemand@work
  ```
  每个实例只运行 `@` 后面那个 profile（`--profile`），因此可以各自独立地启动、
  停止、重启与查看日志（`journalctl -u ddnsondemand@home`）。各实例共用配置文件，
  但状态文件相互独立（`state-<profile>.json`）。
- **Oneshot + timer**（所有 profile）：`examples/ddnsondemand-oneshot.service`
  与 `examples/ddnsondemand.timer`
  ```sh
  systemctl enable --now ddnsondemand.timer
  ```
- **Oneshot + timer，每个 profile 一个 timer**：
  `examples/ddnsondemand-oneshot@.service` 与
  `examples/ddnsondemand-oneshot@.timer`
  ```sh
  systemctl enable --now ddnsondemand-oneshot@home.timer
  ```
  与 `ddnsondemand@.service` 一样按 profile 拆分，只是由 timer 触发而非常驻进程：
  每个 timer 只更新自己的 profile，并各自保留独立的状态文件。

这些 unit 使用 `DynamicUser=yes` + `StateDirectory=ddnsondemand`，因此状态文件
位于 `/var/lib/ddnsondemand/` 下。

> 通过 `.deb` 安装时，上述 unit 已随包安装到 `/usr/lib/systemd/system/`。

## 说明与限制

- IPv6 的 `exclude_temporary` 为**尽力而为**：`if-addrs` 不暴露 netlink 的
  `IFA_F_TEMPORARY` 标志，因此无法可靠区分临时/隐私地址，该选项目前未强制生效。
- 通用提供方无法区分“已变化”与“本就一致”；引擎仅在状态显示发生变化时才调用它，
  并按成功处理。
- 每个 profile 的各协议栈相互独立解析；某一协议栈失败不会阻塞其他协议栈或其他 profile。
- 请求失败时，日志、错误信息和模板变量 `{{error}}` 中的 URL 只显示为
  `scheme://host[:port]/...`。这样，查询参数、webhook 路径或 `user:password@`
  里的 token 不会写进日志，也不会随失败通知发出。模板报错同样不会引用模板原文。
  可以给通知目标设置 `name`，以便在日志中区分它们。
- 凡是通过 `${VAR}` 从环境变量取得的值，在任何可能被打印的地方都会被遮成
  `***`：日志、错误信息、`check` 输出，以及转发到通知里的 `{{error}}`。这能挡住
  服务器在响应正文里回显的 token，以及类型错误里被引用的配置值。**直接写在配置
  文件里**（而非经由 `${VAR}`）的密钥，以及响应正文里的其他内容，不会被遮蔽——
  所以请把密钥放在环境变量里，用 `${VAR}` 引入。

## 许可证

MIT —— 见 [LICENSE](LICENSE)。
