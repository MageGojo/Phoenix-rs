# Phoenix-rs

[![Crates.io](https://img.shields.io/crates/v/phoenixrs.svg)](https://crates.io/crates/phoenixrs)
[![CI](https://github.com/MageGojo/Phoenix-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MageGojo/Phoenix-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.95%2B-orange.svg)](https://www.rust-lang.org/)
[![GitHub](https://img.shields.io/badge/GitHub-MageGojo%2FPhoenix--rs-181717?logo=github)](https://github.com/MageGojo/Phoenix-rs)
[![GitCode](https://img.shields.io/badge/GitCode-Roufsi%2FPhoenix--rs-C71D23)](https://gitcode.com/Roufsi/Phoenix-rs)

**Phoenix-rs** 是由 [极数本源（ApiZero）](https://apizero.cn/) 打造的 Rust 全栈 Web 框架：以 [Hyper](https://hyper.rs/) 为 HTTP 核心，提供接近 Laravel 的开发体验，并默认集成 React + TypeScript（Islands / SPA / SSR）。一套约定，从路由、契约到前端 action 全链路类型安全。


## 源码镜像

| 平台 | 仓库 | 说明 |
| --- | --- | --- |
| **GitHub** | [github.com/MageGojo/Phoenix-rs](https://github.com/MageGojo/Phoenix-rs) | 国际协作 / Actions CI / crates.io 元数据 `repository` |
| **GitCode** | [gitcode.com/Roufsi/Phoenix-rs](https://gitcode.com/Roufsi/Phoenix-rs) | 国内镜像，内容与 GitHub 同步 |

克隆任选其一即可：

```bash
git clone https://github.com/MageGojo/Phoenix-rs.git
# 或
git clone https://gitcode.com/Roufsi/Phoenix-rs.git
```

## AI / Agent 开发（默认必读）

**凡在本仓库或基于 Phoenix-rs 的应用里写代码，AI 必须先加载官方 Skill，再动手。**

| 文件 | 用途 |
| --- | --- |
| [`.cursor/skills/phoenix/SKILL.md`](.cursor/skills/phoenix/SKILL.md) | **主 Skill**：新项目清单、`px` 工作流、铁律、反模式 |
| [`.cursor/skills/phoenix/api-rust.md`](.cursor/skills/phoenix/api-rust.md) | Rust API 速查 |
| [`.cursor/skills/phoenix/api-react.md`](.cursor/skills/phoenix/api-react.md) | `@apizero/react` 速查 |
| [`AGENTS.md`](AGENTS.md) | 仓库级 Agent 约定（指向上述 Skill） |

Cursor 会从 `.cursor/skills/phoenix/` 自动发现 Skill；其它 Agent 请按 [`AGENTS.md`](AGENTS.md) 用 Read 打开 `SKILL.md`。

## 特性一览

- **Laravel 风格 DX**：命名路由、Resource、中间件别名、`px new` / `px make:*` / `px migrate` / `px dev` / `px release*`
- **类型安全全链路**：Rust Request / Resource / Page Props 契约自动生成 TypeScript 与可调用 action
- **React 一等公民**：Islands（默认）、SPA、SSR；页面协议局部导航；表单 / prefetch / partial reload
- **安全默认开启**：Session、CSRF、CSP nonce、Host allowlist、限流、JWT + RBAC/ABAC；Argon2id 密码哈希（`phoenix-crypto`）
- **数据与运维**：Toasty（SQLite / PostgreSQL / MySQL）+ 迁移、Prometheus 指标、可选 Redis / Queue / Mail / Storage
- **扩展与发版**：编译期 Feature 插件；制品校验 + `current` 原子切换与回滚

## 快速开始

要求：Rust **1.95+**、Node.js（Vite / React）、可选 SQLite（默认）/ PostgreSQL / MySQL。数据库驱动通过 Cargo feature 按需编译；新应用默认只链接 SQLite。

### 安装 `px`

从 crates.io 安装（推荐）：

```bash
cargo install px-cli
```

或从 Git 安装（镜像任选）：

```bash
cargo install --git https://github.com/MageGojo/Phoenix-rs px-cli
# 或
cargo install --git https://gitcode.com/Roufsi/Phoenix-rs px-cli
```

本仓库内开发时也可用 `cargo install --path crates/phoenix-cli`。

`px-cli` 是 crates.io **包名**（安装后命令为 `px`）；安装后 PATH 里就是二进制 `px`。`px new` 生成的应用依赖 crates.io 上的门面包 `phoenixrs`（代码里仍写 `use phoenix::…`）。

### 创建并运行

```bash
px new my-app
cd my-app
cp .env.example .env   # 默认使用 SQLite，无需额外配置即可启动
px migrate
px dev
```

切换 PostgreSQL 或 MySQL 时，同时把应用 `Cargo.toml` 的默认 feature 与 `config/database.toml` 对齐；例如一次性 PostgreSQL 构建使用 `cargo build --no-default-features --features pgsql`。

启动成功后访问 **http://127.0.0.1:3000**。切换到 PostgreSQL / MySQL 见 [docs/CONFIG.md](docs/CONFIG.md)。

生成完整 CRUD 骨架：

```bash
px make:model Post --all
```

### 运行官方示例

```bash
cd examples/blog
px migrate
px dev
# http://127.0.0.1:3000
```

blog 示例包含**真实 Auth 链路**：Toasty 持久化用户、Argon2id 密码哈希、Cookie Session 登录 / 登出与受保护的 `/admin` 后台（设计见 [docs/AUTH_ADMIN.md](docs/AUTH_ADMIN.md)）。

多应用挂载示例见 `examples/multi-app`；Feature 插件示例见 `examples/plugin-greeter`。

## 最小代码

```rust
use phoenix::prelude::*;

pub struct UserController;

impl UserController {
    pub async fn show(request: Request) -> Response {
        let user = request.param("user").unwrap_or("unknown");
        Json(json!({ "user": user })).into_response()
    }
}

Routes::new()
    .get("/users/{user}", UserController::show)
    .name("users.show");
```

```tsx
import { members } from "../generated/routes.js";

const member = await members.store({ name });
```

更多：[业务开发指南](docs/BUSINESS_GUIDE.md) · [开发者体验](docs/DX.md) · [React 渲染](docs/RENDERING.md)

## 命名（crates.io / 托管）

| 用途 | 名称 | 说明 |
| --- | --- | --- |
| CLI | **[`px-cli`](https://crates.io/crates/px-cli)** | `cargo install px-cli` → 得到命令 `px` |
| 应用依赖门面 | **[`phoenixrs`](https://crates.io/crates/phoenixrs)** | `phoenix` / `phoenix-rs` / `phoenix-cli` 均已被占用；应用写 `phoenix = { package = "phoenixrs", … }`，Rust 仍 `use phoenix::` |
| 产品 / GitHub / GitCode | **Phoenix-rs** | 对外品牌与仓库名 |

## 仓库结构

```text
.cursor/skills/phoenix/  # AI 官方 Skill（入库，默认先读）
AGENTS.md                # Agent 强制约定入口
crates/                  # Rust 框架组件与统一入口 phoenixrs（lib = phoenix）
packages/                # @apizero/react、@apizero/vite、SSR 包
schemas/                 # config/*.toml JSON Schema（Taplo）
examples/blog            # 参考应用
examples/multi-app
examples/plugin-greeter  # Feature 插件示例
fuzz/                    # cargo-fuzz 目标（HTTP / 密码学边界）
benchmarks/              # Criterion 基准
docs/                    # 产品、架构与领域文档
.github/workflows        # CI（GitHub Actions）
```

### 关于 `fuzz/`

`fuzz/` **属于本框架质量门禁**，不是无关目录。它使用 [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) / libFuzzer，对 `phoenix-http` 边界与 `phoenix-crypto` 盲索引信封等做模糊测试。日常开发不必运行；CI / 安全流水线会做 smoke。产物目录（`fuzz/target`、`artifacts`、corpus 样本）已在 `.gitignore` 中排除。

## 开发与检查

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
npm run ci:node
```

质量门禁说明见 [docs/QUALITY_GATES.md](docs/QUALITY_GATES.md)。公开托管与 crates.io 发布规划见 [docs/RELEASE.md](docs/RELEASE.md)。

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [`.cursor/skills/phoenix/SKILL.md`](.cursor/skills/phoenix/SKILL.md) | **AI 默认入口** |
| [AGENTS.md](AGENTS.md) | Agent 强制约定 |
| [FEATURES.md](docs/FEATURES.md) | Feature / 插件扩展（第三方 crate） |
| [RELEASE_PIPELINE.md](docs/RELEASE_PIPELINE.md) | 打版本包 / 安装 / 回滚 |
| [CONFIG.md](docs/CONFIG.md) | Laravel 风格 `config/*.toml` 与选库 |
| [PRODUCT.md](docs/PRODUCT.md) | 产品定位与范围 |
| [PROJECT.md](docs/PROJECT.md) | 架构与模块边界 |
| [BUSINESS_GUIDE.md](docs/BUSINESS_GUIDE.md) | 业务开发主路径 |
| [DX.md](docs/DX.md) | CLI、路由约定、生成器 |
| [CONTRACTS.md](docs/CONTRACTS.md) | Rust ↔ TypeScript 契约 |
| [DATABASE.md](docs/DATABASE.md) | Toasty 与迁移 |
| [SECURITY.md](docs/SECURITY.md) | 安全栈 |
| [RENDERING.md](docs/RENDERING.md) | React 页面协议 |
| [REACT_DX_*.md](docs/REACT_DX_HOOKS.md) | 前端 hooks / 表单 / 性能 DX |
| [AUTH_ADMIN.md](docs/AUTH_ADMIN.md) | 管理后台 / Auth 完整链路设计（Session + Argon2id） |
| [AUTHORIZATION.md](docs/AUTHORIZATION.md) | 授权（RBAC / ABAC） |
| [MAIL.md](docs/MAIL.md) / [QUEUE.md](docs/QUEUE.md) | 邮件与队列 |
| [METRICS.md](docs/METRICS.md) / [TLS.md](docs/TLS.md) | Prometheus 指标 / TLS |
| [MULTI_APP.md](docs/MULTI_APP.md) / [REALTIME.md](docs/REALTIME.md) | 多应用挂载 / 实时能力 |
| [REDIS.md](docs/REDIS.md) | Redis Session / 限流后端 |
| [TESTING_AND_STORAGE.md](docs/TESTING_AND_STORAGE.md) | 测试与存储 |
| [工具与约定.md](docs/工具与约定.md) | 命令、依赖、断点续作 |
| [PROGRESS.md](docs/PROGRESS.md) | 进度表（对账线索） |
| [NEXT.md](docs/NEXT.md) | 下一阶段优先级 |
| [RC_CLOSURE_PLAN.md](docs/RC_CLOSURE_PLAN.md) | 发布候选收口批计划与验收记录 |
| [RELEASE.md](docs/RELEASE.md) | GitHub / GitCode 公开托管与 crates.io 发布顺序 |

## 当前状态

早期开发阶段（`0.1.0`）。**全部 24 个 crate 已发布至 crates.io**（2026-07-24，含门面 [`phoenixrs`](https://crates.io/crates/phoenixrs) 与 CLI [`px-cli`](https://crates.io/crates/px-cli)），GitHub / GitCode 双镜像已公开。核心垂直切片（HTTP、路由、契约、React、安全、CLI、迁移）、TOML 配置、MySQL 驱动、Feature 插件与发版流水线 MVP 已可运行；blog 示例 Auth 为真实持久化链路（Toasty 用户 + Argon2id + Cookie Session，见 [docs/AUTH_ADMIN.md](docs/AUTH_ADMIN.md)）。仍在演进：邮件真实 SMTP、队列生产驱动、`px make:auth` 生成器、服务端 partial props 求值。

## 公司与许可

- **出品**：极数本源（ApiZero）— [https://apizero.cn/](https://apizero.cn/)
- **联系**：api@zerois.cn
- **源码**：[GitHub](https://github.com/MageGojo/Phoenix-rs) · [GitCode](https://gitcode.com/Roufsi/Phoenix-rs)
- **许可**：[MIT](LICENSE)

---

© 2026 极数本源 ApiZero. All rights reserved.
