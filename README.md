<div align="center">

# RESTie

> A transparent RESTful API client. The client is a transparent proxy of the server API, not another layer of abstraction to learn.

</div>

Users only need to read the official API documentation to use RESTie directly. The client introduces no extra naming conventions or parameter style changes.

**English below · 中文见下方 ↓**

---

# English

RESTie is a transparently proxied RESTful API client. What the API calls something, the command calls the same; what a parameter is named, the `--param` uses the same name.

## Features

- **Transparent proxy** — What the API calls something, the command calls the same; what a parameter is named, the `--param` writes the same
- **Zero-config run** — Works without a config file: just `restie GET /path`
- **OpenAPI one-click generation** — Auto-generate command config from an official OpenAPI spec, or from a platform preset fetched from the presets repository
- **Three-level structure** — `restie <module> <command> [--params]`, commands grouped by module for clear browsing
- **Multi-site support** — One client manages all your REST APIs (GitHub, GitLab, Stripe, OpenAI...)
- **Flexible auth** — Bearer Token, API Key, custom Header, external signing scripts (AWS SigV4, Alibaba Cloud, Tencent Cloud...)
- **Presets are data** — The platform list lives in a separate repository, so it can change without a RESTie release
- **Config-as-docs** — The config file is also the data source for `--help`
- **Single binary** — Compiled with Rust, cross-platform, dependency-free

## Quick Start

```bash
# 1. Generate config (the preset catalog is fetched on first run)
restie generate github

# 2. Set your token
export GITHUB_TOKEN="ghp_xxxxxxxxxx"

# 3. Use it directly
restie repos get --owner rust-lang --repo rust
restie repos list-mine --per_page 10
```

## Platform Presets

RESTie ships **no** presets of its own. The platform list lives in a separate
data repository, [samxxu/restie-presets](https://github.com/samxxu/restie-presets),
and is fetched on demand:

```bash
restie presets           # browse what is available
restie presets update    # refresh from the presets repository
```

The first time you run a command that needs presets, RESTie fetches the catalog
automatically, so a fresh install needs no setup step. Everything is cached under
`~/.config/restie/presets/`.

That split is deliberate: platform APIs and their auth change far more often than
the client does. AWS SigV4, Alibaba Cloud and Tencent Cloud request signing, for
instance, should never require shipping a new binary. Platforms are added,
re-pointed or removed by editing one YAML file in the presets repository.

Once fetched, generating a config is one command:

```bash
restie generate github
restie generate stripe
```

Use `--filter` to include only the endpoints under a path:

```bash
restie generate github --filter /repos/          # only repos-related endpoints
restie generate digitalocean --filter droplets   # only droplet-related endpoints
```

Anything you can point at an OpenAPI spec works with no preset at all:

```bash
restie generate --from https://api.example.com/openapi.json
```

### Preset resolution

Two layers, highest first:

| # | Layer | Location | Written by |
|---|-------|----------|------------|
| 1 | **Local override** | `~/.config/restie/presets/catalog.local.yaml` | you, by hand |
| 2 | **Remote cache** | `~/.config/restie/presets/catalog.yaml` | `restie presets update` |

The remote cache is the base; the local override is merged on top of it. A
same-named preset in the override replaces the base entry wholesale, and new
names are appended, so you can keep private or patched platforms locally without
a refresh overwriting them. With neither present the catalog is simply empty, and
`restie` says so and tells you how to fetch one.

Use a different repository (a fork, a branch, an internal mirror):

```bash
restie presets update --repo https://raw.githubusercontent.com/<user>/restie-presets/<branch>
export RESTIE_PRESETS_REPO=https://raw.githubusercontent.com/<user>/restie-presets/<branch>  # default for future runs
```

Behind a proxy? `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` are honored, including
`socks5h://` for a SOCKS proxy.

What `presets update` does:

- Downloads `catalog.yaml` and every signing script it references (the `scripts:`
  manifest, plus each `script`-auth preset's `auth.command`).
- Validates the catalog (schema version, unique names, safe relative script
  paths, URL rules) **before** writing anything, and stages files with atomic
  writes, so a partial fetch never leaves a broken cache.
- Writes into `~/.config/restie/presets/` — and **never** touches
  `catalog.local.yaml`, so your own presets survive every refresh.

A catalog this build cannot read is rejected with an explicit error rather than
silently misread: presets are data, and the data format is versioned.

#### Local override example

```yaml
# ~/.config/restie/presets/catalog.local.yaml
schema_version: 1
presets:
  - name: internal          # new platform: appended to the catalog
    display_name: Internal API
    openapi_url: https://internal.example.com/openapi.json
    description: Company-internal REST API
    auth:
      type: bearer_token
      token_env: INTERNAL_API_TOKEN
  - name: github            # existing name: replaces the fetched entry
    display_name: GitHub (mirror)
    openapi_url: https://mirror.internal/gh.json
    description: internal GitHub mirror
    auth:
      type: bearer_token
      token_env: MY_GH_TOKEN
```

`restie presets` reports the effective source and how many overrides applied:

```
Catalog: schema v1 — remote cache (~/.config/restie/presets/catalog.yaml)
        + 2 preset(s) from local override (~/.config/restie/presets/catalog.local.yaml) (18 presets)
```

The override file is validated exactly like a fetched catalog (schema version,
unique names, no absolute or `..` script paths), so a typo degrades to a warning
instead of breaking `restie generate`.

### Raw-only presets (Alibaba / Tencent Cloud / AWS)

Alibaba Cloud, Tencent Cloud and AWS have no single unified OpenAPI spec — their
APIs are split per product and sign each request instead of taking a bearer token.
They are catalog entries like any other, marked **raw-only**:
`restie generate aliyun` fetches no spec, it just writes a site config (base URL +
auth, no commands). The signing scripts are staged by `presets update` and run at
request time, reading the `RESTIE_*` context variables RESTie injects
(`RESTIE_METHOD`, `RESTIE_PATH`, `RESTIE_QUERY`, `RESTIE_BODY`).

```bash
restie presets update           # fetch the catalog + its signing scripts
restie generate aliyun          # writes site + auth config (raw-only, no commands)

# set credentials (names come from the preset's auth.env)
export ALIBABA_CLOUD_ACCESS_KEY_ID=...   ALIBABA_CLOUD_ACCESS_KEY_SECRET=...
export TENCENTCLOUD_SECRET_ID=...        TENCENTCLOUD_SECRET_KEY=...

# call in raw mode — the matching signing script runs automatically
restie --site aliyun GET /?Action=DescribeInstances&Format=JSON
restie --site tencent POST / '{"Version":"2017-03-12","Action":"DescribeInstances"}'
```

The generated config carries a default `site.base_url` (e.g. `https://ecs.aliyuncs.com`,
`https://cvm.tencentcloudapi.com`). Each product has its own host, so edit
`base_url` in `~/.config/restie/<name>.yaml` — or the script itself under
`~/.config/restie/presets/auth/` — to match the product you are calling. The
scripts are plain bash you can read and customize; they are fetched from the
presets repository, not compiled into the binary.

## Usage

### Config-driven (recommended)

```bash
# List modules
restie --site github

# List commands in a module
restie repos --help

# Command help
restie repos get --help

# Invoke
restie repos get --owner rust-lang --repo rust
restie repos create-mine --name my-project --description "test"
restie issues create --owner o --repo r --title "bug" --body "description"
```

### Raw HTTP calls

```bash
restie GET /user/repos
restie POST /repos/o/r/issues '{"title":"bug"}'
restie DELETE /repos/o/r
```

### Specify a site

```bash
restie --site github repos get --owner o --repo r
restie --site digitalocean droplets list
restie --site stripe customers list
```

### Active site (persisted)

When you work with more than one site, remember which one you're talking to with a
persisted "active site". Once set, omit `--site` to use it; pass `--site <name>` to
temporarily hit another site. Output is always pure JSON on stdout, so pipelines
(`jq`, etc.) work unchanged.

```bash
restie site                 # show the active site
restie site set github      # make github the active site
restie site set gitlab      # switch to gitlab
restie site clear           # clear the active site (back to auto-selection)

restie repos get --owner o --repo r | jq '.full_name'
```

### Specify a config file

```bash
restie --config ./my-site.yaml repos get --owner o --repo r
```

### List generated sites

```bash
restie sites
```

## Project Structure

```
src/
├── main.rs          — CLI entry point, first-run preset bootstrap
├── http_client.rs   — HTTP client + call() + GET/POST/PUT/PATCH/DELETE
├── auth.rs          — Auth module (bearer/api_key/custom_header/script/none)
├── config.rs        — YAML config schema, loading, discovery, help generation
├── cli.rs           — CLI engine (param distribution, path replacement)
├── generator.rs     — OpenAPI spec → YAML config converter
├── presets.rs       — Preset mechanism: fetch, validate, cache, two-layer resolve
└── lib.rs           — Module declarations
```

There is no `presets/` or `auth/` directory here. Preset data (the platform list
and the signing scripts) lives in the separate
[presets repository](https://github.com/samxxu/restie-presets); this repository
holds only the code that fetches and uses it. A unit test
(`test_core_repo_ships_no_catalog_or_scripts`) fails if that separation is ever
broken by accident.

Presets are data, and the data format is versioned: a catalog declares a
`schema_version`, and one this build cannot read is rejected with an explicit
error instead of being silently misread. Unknown fields are errors too, so a typo
in the catalog surfaces immediately rather than half-applying.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    User terminal (CLI)                   │
│  restie <module> <command> --param value                  │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│               Layer 2: CLI engine                        │
│  Parse commands, assign param positions, replace paths,  │
│  generate help                                           │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│               Layer 1: HTTP client                       │
│  GET / POST / PUT / PATCH / DELETE                       │
│  Auth injection (Bearer / API Key / custom Header / script)│
│  Timeout, User-Agent, JSON response parsing              │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│               Layer 3: YAML config (optional)            │
│  Powers --help output + tab completion + precise args    │
│  Auto-generated from OpenAPI spec, zero manual upkeep    │
└──────────────────────────────────────────────────────────┘
```

## Authentication

### Simple auth (config-only)

```yaml
site:
  name: GitHub
  base_url: https://api.github.com
  auth:
    type: bearer_token
    token_env: GITHUB_TOKEN
```

| Type | Description |
|------|-------------|
| `bearer_token` | `Authorization: Bearer <token>` |
| `api_key` | custom header + prefix |
| `custom_header` | custom header, no prefix |
| `none` | no auth |

### Complex auth (external script)

```yaml
site:
  name: AWS EC2
  base_url: https://ec2.us-east-1.amazonaws.com
  auth:
    type: script
    command: auth/aws-sigv4.sh      # relative: resolved under ~/.config/restie/
    env:
      AWS_ACCESS_KEY_ID: ${AWS_ACCESS_KEY_ID}
      AWS_SECRET_ACCESS_KEY: ${AWS_SECRET_ACCESS_KEY}
      AWS_REGION: ${AWS_REGION}
      AWS_SERVICE: ${AWS_SERVICE}
```

The script receives `RESTIE_METHOD`, `RESTIE_PATH`, `RESTIE_QUERY`, `RESTIE_BODY`
environment variables, and outputs one `Header: value` per line.

A bare relative `command` resolves against `~/.config/restie/auth/` first (your
own edits, if any), then the presets cache `~/.config/restie/presets/` — which is
where the signing scripts from `restie presets update` land. No scripts are
compiled into the binary.

## Config Files

Config files live in `~/.config/restie/` by default, one `.yaml` per site.

```bash
~/.config/restie/
├── github.yaml
├── gitlab.yaml
└── presets/
    ├── catalog.yaml         ← layer 2: remote cache (written by `presets update`)
    ├── catalog.local.yaml   ← layer 1: your local override (optional, never overwritten)
    └── auth/                ← signing scripts staged by `presets update`
        ├── aws-sigv4.sh
        ├── aliyun-sign.sh
        └── tencent-sign.sh
```

## Generate from a Custom OpenAPI

```bash
# From a URL
restie generate --from https://api.example.com/openapi.json

# From a local file
restie generate --from ./my-api.yaml

# Specify an output path
restie generate --from ./my-api.yaml --output ./my-site.yaml

# Specify a site name
restie generate --from ./my-api.yaml --name my-api
```

## Development

```bash
# Build
cargo build --release

# Run tests
cargo test

# Run
./target/release/restie presets
```

## License

MIT

---

# 中文

RESTie 是一个透明的 RESTful API 客户端。API 叫什么名，命令就叫什么名；参数叫什么名，`--param` 就写什么名。

## 特性

- **透明代理** — API 叫什么名，命令就叫什么名；参数叫什么名，`--param` 就写什么名
- **零配置运行** — 没有配置文件也能用，直接 `restie GET /path`
- **OpenAPI 一键生成** — 从官方 OpenAPI spec 自动生成命令配置，或直接用预设仓库里的平台预设
- **三级结构** — `restie <module> <command> [--params]`，命令按模块分组，清晰好浏览
- **多站点支持** — 一套客户端管理所有 REST API（GitHub, GitLab, Stripe, OpenAI...）
- **灵活鉴权** — 支持 Bearer Token、API Key、自定义 Header、外部签名脚本（AWS SigV4、阿里云、腾讯云…）
- **预设即数据** — 平台清单放在独立仓库，增删改无需发布 RESTie 新版本
- **配置即文档** — 配置文件同时是 `--help` 的数据源
- **单二进制** — Rust 编译，跨平台，无依赖

## 快速开始

```bash
# 1. 生成配置（首次运行会自动拉取预设目录）
restie generate github

# 2. 设置 Token
export GITHUB_TOKEN="ghp_xxxxxxxxxx"

# 3. 直接用
restie repos get --owner rust-lang --repo rust
restie repos list-mine --per_page 10
```

## 平台预设

RESTie 自身**不带任何预设**。平台清单放在独立的数据仓库
[samxxu/restie-presets](https://github.com/samxxu/restie-presets) 里，按需拉取：

```bash
restie presets           # 浏览可用平台
restie presets update    # 从预设仓库刷新
```

第一次运行需要预设的命令时，RESTie 会自动把目录拉下来，所以全新安装不需要任何初始化步骤。
所有内容缓存于 `~/.config/restie/presets/`。

这样拆分是有意为之：平台 API 及其鉴权的变化频率远高于客户端本身。例如 AWS SigV4、
阿里云与腾讯云的请求签名，不应该为了改它们而发一个新二进制。平台的新增、改址、下架，
都只是改预设仓库里的一个 YAML 文件。

拉取之后，生成配置只需一行：

```bash
restie generate github
restie generate stripe
```

生成时可以用 `--filter` 只包含特定路径的端点：

```bash
restie generate github --filter /repos/          # 只生成 repos 相关
restie generate digitalocean --filter droplets   # 只生成实例相关
```

任何能指向 OpenAPI spec 的接口，不用预设也能生成：

```bash
restie generate --from https://api.example.com/openapi.json
```

### 预设解析

两层，优先级由高到低：

| 层级 | 名称 | 位置 | 由谁写入 |
|------|------|------|---------|
| 1 | **本地覆盖** | `~/.config/restie/presets/catalog.local.yaml` | 你自己手写 |
| 2 | **远端缓存** | `~/.config/restie/presets/catalog.yaml` | `restie presets update` |

远端缓存是*基础*目录，本地覆盖再**叠加**在最上面：覆盖文件里同名的预设整体替换基础条目，
新名字追加到末尾。这样你可以把私有或改过的平台留在本地，而不必担心被一次刷新覆盖掉。
两者都不存在时目录就是空的，`restie` 会明确告知并给出拉取方法。

换用其它仓库（fork、分支、内网镜像）：

```bash
restie presets update --repo https://raw.githubusercontent.com/<user>/restie-presets/<branch>
export RESTIE_PRESETS_REPO=https://raw.githubusercontent.com/<user>/restie-presets/<branch>  # 之后作为默认
```

需要走代理？`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 都会被识别，SOCKS 代理的
`socks5h://` 也支持。

`presets update` 做了什么：

- 下载 `catalog.yaml` 及其中引用的每个签名脚本（`scripts:` 清单，以及每个 `script` 鉴权预设的 `auth.command`）。
- **先**校验目录（schema 版本、名字唯一、相对安全脚本路径、URL 规则）再落盘，并用原子写分阶段写入，
  部分拉取失败不会留下损坏的缓存。
- 写入 `~/.config/restie/presets/`，且**绝不**触碰 `catalog.local.yaml`，你的自有预设不会被刷新覆盖。

当前构建无法读取的目录会被明确报错拒绝，而不是被静默误读：预设是数据，而数据格式是带版本号的。

#### 本地覆盖示例

```yaml
# ~/.config/restie/presets/catalog.local.yaml
schema_version: 1
presets:
  - name: internal          # 新平台：追加进目录
    display_name: Internal API
    openapi_url: https://internal.example.com/openapi.json
    description: 公司内部 REST API
    auth:
      type: bearer_token
      token_env: INTERNAL_API_TOKEN
  - name: github            # 与已有名字相同：替换拉取来的条目
    display_name: GitHub (mirror)
    openapi_url: https://mirror.internal/gh.json
    description: 内部 GitHub 镜像
    auth:
      type: bearer_token
      token_env: MY_GH_TOKEN
```

`restie presets` 会显示当前生效来源以及应用了多少条覆盖：

```
Catalog: schema v1 — remote cache (~/.config/restie/presets/catalog.yaml)
        + 2 preset(s) from local override (~/.config/restie/presets/catalog.local.yaml) (18 presets)
```

覆盖文件与拉取来的目录走同一套校验（schema 版本、名字唯一、禁止绝对路径与 `..` 脚本路径），
写错只会降级为一条告警，而不会让 `restie generate` 挂掉。

### 仅 raw 模式的预设（阿里云 / 腾讯云 / AWS）

阿里云、腾讯云和 AWS 都没有统一的 OpenAPI Spec，它们的 API 按产品拆分，用请求签名
而非 Bearer Token。它们与其它平台一样是目录里的条目，只是标记为**仅 raw 模式**：
`restie generate aliyun` 不会抓取任何 Spec，只写一份站点配置（base URL + 鉴权，无命令）。
签名脚本由 `presets update` 拉取，在请求时运行，读取 RESTie 注入的 `RESTIE_*` 上下文变量
（`RESTIE_METHOD`、`RESTIE_PATH`、`RESTIE_QUERY`、`RESTIE_BODY`）生成鉴权头。

```bash
restie presets update           # 拉取目录及其签名脚本
restie generate aliyun          # 写站点 + 鉴权配置（仅 raw，无命令）

# 设置凭据（变量名来自该预设的 auth.env）
export ALIBABA_CLOUD_ACCESS_KEY_ID=...   ALIBABA_CLOUD_ACCESS_KEY_SECRET=...
export TENCENTCLOUD_SECRET_ID=...        TENCENTCLOUD_SECRET_KEY=...

# 在 raw 模式下调用 —— 会自动运行对应的签名脚本
restie --site aliyun GET /?Action=DescribeInstances&Format=JSON
restie --site tencent POST / '{"Version":"2017-03-12","Action":"DescribeInstances"}'
```

生成的配置带有一个默认的 `site.base_url`（例如 `https://ecs.aliyuncs.com`、
`https://cvm.tencentcloudapi.com`）。每个产品的入口域名不同，请按需修改
`~/.config/restie/<name>.yaml` 中的 `base_url`，或直接调整脚本本身
（位于 `~/.config/restie/presets/auth/`）。这些脚本就是可读可改的 bash，
它们来自预设仓库，并不编译进二进制。

## 使用方式

### 配置驱动（推荐）

```bash
# 模块列表
restie --site github

# 模块内命令列表
restie repos --help

# 命令帮助
restie repos get --help

# 调用
restie repos get --owner rust-lang --repo rust
restie repos create-mine --name my-project --description "test"
restie issues create --owner o --repo r --title "bug" --body "描述"
```

### 原始 HTTP 调用

```bash
restie GET /user/repos
restie POST /repos/o/r/issues '{"title":"bug"}'
restie DELETE /repos/o/r
```

### 指定站点

```bash
restie --site github repos get --owner o --repo r
restie --site digitalocean droplets list
restie --site stripe customers list
```

### 当前站点（持久化）

同时使用多个站点时，可用持久化的"当前站点"记住当前操作对象。设置后省略
`--site` 即使用它；临时访问其他站点时用 `--site <name>` 指定。stdout 始终输出
纯净 JSON，管道（`jq` 等）可正常使用。

```bash
restie site                 # 查看当前站点
restie site set github      # 设置 github 为当前站点
restie site set gitlab      # 切换到 gitlab
restie site clear           # 清除当前站点（回到自动选择）

restie repos get --owner o --repo r | jq '.full_name'
```

### 指定配置文件

```bash
restie --config ./my-site.yaml repos get --owner o --repo r
```

### 查看已生成的站点

```bash
restie sites
```

## 项目结构

```
src/
├── main.rs          — CLI 入口，首次运行的预设引导
├── http_client.rs   — HTTP 客户端 + call() + GET/POST/PUT/PATCH/DELETE
├── auth.rs          — 鉴权模块（bearer/api_key/custom_header/script/none）
├── config.rs        — YAML 配置 schema、加载、查找、帮助生成
├── cli.rs           — CLI 引擎（参数分配、路径替换）
├── generator.rs     — OpenAPI spec → YAML 配置转换器
├── presets.rs       — 预设机制：拉取、校验、缓存、两层解析
└── lib.rs           — 模块声明
```

这里没有 `presets/` 或 `auth/` 目录。预设数据（平台清单与签名脚本）住在独立的
[预设仓库](https://github.com/samxxu/restie-presets) 里；本仓库只有拉取和使用它的代码。
一旦这个边界被意外打破，单元测试 `test_core_repo_ships_no_catalog_or_scripts` 会失败。

预设是数据，而数据格式带版本号：目录里声明 `schema_version`，当前构建无法识别的版本会被
明确报错拒绝，而不是被静默误读。未知字段同样报错，所以目录里的笔误会立刻暴露，而不是只生效一半。

## 架构设计

```
┌─────────────────────────────────────────────────────────┐
│                     用户终端（CLI）                       │
│  restie <module> <command> --param value                  │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│              第二层：CLI 引擎                              │
│  解析命令、分配参数位置、替换路径模板、生成帮助              │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│              第一层：HTTP 客户端                          │
│  GET / POST / PUT / PATCH / DELETE                       │
│  鉴权注入（Bearer / API Key / 自定义 Header / 脚本）       │
│  超时、User-Agent、JSON 响应解析                          │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│              第三层：YAML 配置（可选）                     │
│  驱动 --help 输出 + Tab 补全 + 参数位置精确分配              │
│  从 OpenAPI spec 自动生成，零手工维护                       │
└──────────────────────────────────────────────────────────┘
```

## 鉴权方式

### 简单鉴权（纯配置）

```yaml
site:
  name: GitHub
  base_url: https://api.github.com
  auth:
    type: bearer_token
    token_env: GITHUB_TOKEN
```

| 类型 | 说明 |
|------|------|
| `bearer_token` | `Authorization: Bearer <token>` |
| `api_key` | 自定义 header + 前缀 |
| `custom_header` | 自定义 header，无前缀 |
| `none` | 不鉴权 |

### 复杂鉴权（外部脚本）

```yaml
site:
  name: AWS EC2
  base_url: https://ec2.us-east-1.amazonaws.com
  auth:
    type: script
    command: auth/aws-sigv4.sh      # 相对路径：在 ~/.config/restie/ 下解析
    env:
      AWS_ACCESS_KEY_ID: ${AWS_ACCESS_KEY_ID}
      AWS_SECRET_ACCESS_KEY: ${AWS_SECRET_ACCESS_KEY}
      AWS_REGION: ${AWS_REGION}
      AWS_SERVICE: ${AWS_SERVICE}
```

脚本接收 `RESTIE_METHOD`、`RESTIE_PATH`、`RESTIE_QUERY`、`RESTIE_BODY` 环境变量，输出每行一个 `Header: value`。

裸相对路径的 `command` 会先在 `~/.config/restie/auth/` 下解析（你自己改过的脚本优先），
再查预设缓存 `~/.config/restie/presets/`，也就是 `restie presets update` 落地的位置。
二进制里不编译任何脚本。

## 配置文件

配置文件默认放在 `~/.config/restie/` 目录，每个站点一个 `.yaml` 文件。

```bash
~/.config/restie/
├── github.yaml
├── gitlab.yaml
└── presets/
    ├── catalog.yaml         ← 第 2 层：远端缓存（由 `presets update` 写入）
    ├── catalog.local.yaml   ← 第 1 层：你的本地覆盖（可选，永不被覆盖）
    └── auth/                ← `presets update` 拉取的签名脚本
        ├── aws-sigv4.sh
        ├── aliyun-sign.sh
        └── tencent-sign.sh
```

## 从自定义 OpenAPI 生成

```bash
# 从 URL
restie generate --from https://api.example.com/openapi.json

# 从本地文件
restie generate --from ./my-api.yaml

# 指定输出路径
restie generate --from ./my-api.yaml --output ./my-site.yaml

# 指定站点名
restie generate --from ./my-api.yaml --name my-api
```

## 开发

```bash
# 构建
cargo build --release

# 运行测试
cargo test

# 运行
./target/release/restie presets
```

## License

MIT