# Phoenix-rs

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.95%2B-orange.svg)](https://www.rust-lang.org/)
[![GitHub](https://img.shields.io/badge/GitHub-ApiZero%2FPhoenix--rs-181717?logo=github)](https://github.com/ApiZero/Phoenix-rs)
[![GitCode](https://img.shields.io/badge/GitCode-ApiZero%2FPhoenix--rs-C71D23)](https://gitcode.com/ApiZero/Phoenix-rs)

**Phoenix-rs** 是由 [极数本源（ApiZero）](https://apizero.cn/) 打造的 Rust 全栈 Web 框架：以 [Hyper](https://hyper.rs/) 为 HTTP 核心，提供接近 Laravel 的开发体验，并默认集成 React + TypeScript（Islands / SPA / SSR）。

> 与 Elixir 的 [Phoenix](https://www.phoenixframework.org/) 无关。本项目在 crates.io / GitHub / GitCode 使用 **Phoenix-rs** 标识，以区分同名生态。

一个 Key 调用全网 API → 见 [ApiZero](https://apizero.cn/)；一套约定写出完整网站 → 用 Phoenix-rs。

## 源码镜像

| 平台 | 仓库 | 说明 |
| --- | --- | --- |
| **GitHub** | [github.com/ApiZero/Phoenix-rs](https://github.com/ApiZero/Phoenix-rs) | 国际协作 / Actions CI / crates.io 元数据 `repository` |
| **GitCode** | [gitcode.com/ApiZero/Phoenix-rs](https://gitcode.com/ApiZero/Phoenix-rs) | 国内镜像，内容与 GitHub 同步 |

克隆任选其一即可：

```bash
git clone https://github.com/ApiZero/Phoenix-rs.git
# 或
git clone https://gitcode.com/ApiZero/Phoenix-rs.git
```

## AI / Agent 开发（默认必读）

**凡在本仓库或基于 Phoenix-rs 的应用里写代码，AI 必须先加载官方 Skill，再动手。**

| 文件 | 用途 |
| --- | --- |
| [`.cursor/skills/phoenix/SKILL.md`](.cursor/skills/phoenix/SKILL.md) | **主 Skill**：新项目清单、`px` 工作流、铁律、反模式 |
| [`.cursor/skills/phoenix/api-rust.md`](.cursor/skills/phoenix/api-rust.md) | Rust API 速查 |
| [`.cursor/skills/phoenix/api-react.md`](.cursor/skills/phoenix/api-react.md) | `@phoenix/react` 速查 |
| [`AGENTS.md`](AGENTS.md) | 仓库级 Agent 约定（指向上述 Skill） |

Cursor 会从 `.cursor/skills/phoenix/` 自动发现 Skill；其它 Agent 请按 [`AGENTS.md`](AGENTS.md) 用 Read 打开 `SKILL.md`。

开发者也可直接读 Skill 代替翻完整 `docs/` 入门。

## 特性一览

- **Laravel 风格 DX**：命名路由、Resource、中间件别名、`px new` / `px make:*` / `px migrate` / `px dev` / `px release*`
- **类型安全全链路**：Rust Request / Resource / Page Props 契约自动生成 TypeScript 与可调用 action
- **React 一等公民**：Islands（默认）、SPA、SSR；页面协议局部导航；表单 / prefetch / partial reload
- **安全默认开启**：Session、CSRF、CSP nonce、Host allowlist、限流、JWT + RBAC/ABAC
- **数据与运维**：Toasty（SQLite / PostgreSQL / MySQL）+ 迁移、Prometheus 指标、可选 Redis / Queue / Mail / Storage
- **扩展与发版**：编译期 Feature 插件；制品校验 + `current` 原子切换与回滚

## 快速开始

要求：Rust **1.95+**、Node.js（Vite / React）、可选 SQLite（默认）/ PostgreSQL / MySQL。

### 安装 `px`（推荐）

发布到 crates.io 之后：

```bash
cargo install px
px new my-app
cd my-app
cp .env.example .env
px migrate
px dev
```

当前（仓库尚未推 crates 时）可用本仓库或 Git：

```bash
# 从本仓库路径安装
cargo install --path crates/phoenix-cli

# 或从 Git（任选镜像）
# cargo install --git https://github.com/ApiZero/Phoenix-rs px
# cargo install --git https://gitcode.com/ApiZero/Phoenix-rs px
```

`px` 是 crates.io **包名**；安装后 PATH 里就是二进制 `px`。`px new` 在无本地框架源码时，会让应用依赖 crates.io 上的门面包 `phoenixrs`（代码里仍写 `use phoenix::…`）。

生成完整 CRUD 骨架：

```bash
px make:model Post --all
```

运行官方示例：

```bash
cd examples/blog
px dev
# http://127.0.0.1:3000
```

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
| CLI | **`px`** | `cargo install px` → 得到命令 `px` |
| 应用依赖门面 | **`phoenixrs`** | `phoenix` / `phoenix-rs` / `phoenix-cli` 均已被占用；应用写 `phoenix = { package = "phoenixrs", … }`，Rust 仍 `use phoenix::` |
| 产品 / GitHub / GitCode | **Phoenix-rs** | 对外品牌与仓库名，区别于 Elixir Phoenix |

## 仓库结构

```text
.cursor/skills/phoenix/  # AI 官方 Skill（入库，默认先读）
AGENTS.md                # Agent 强制约定入口
crates/                  # Rust 框架组件与统一入口 phoenixrs（lib = phoenix）
packages/                # @phoenix/react、@phoenix/vite、SSR 包
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
npm test --workspace=@phoenix/react
```

质量门禁说明见 [docs/QUALITY_GATES.md](docs/QUALITY_GATES.md)。公开托管清单见 [docs/RELEASE.md](docs/RELEASE.md)。

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
| [工具与约定.md](docs/工具与约定.md) | 命令、依赖、断点续作 |
| [PROGRESS.md](docs/PROGRESS.md) | 进度表（对账线索） |
| [NEXT.md](docs/NEXT.md) | 下一阶段优先级 |
| [RELEASE.md](docs/RELEASE.md) | GitHub / GitCode 公开托管清单 |

## 当前状态

早期开发阶段（`0.1.0`）。核心垂直切片（HTTP、路由、契约、React、安全、CLI、迁移）、TOML 配置、MySQL 驱动、Feature 插件与发版流水线 MVP 已可运行；邮件 SMTP、队列生产驱动、管理后台、服务端 partial props 求值等仍在演进。crates.io 正式发布前需一并发布内部 `phoenix-*` 组件 crate（命名冲突需逐个核对）。

## 公司与许可

- **出品**：极数本源（ApiZero）— [https://apizero.cn/](https://apizero.cn/)
- **联系**：api@zerois.cn
- **源码**：[GitHub](https://github.com/ApiZero/Phoenix-rs) · [GitCode](https://gitcode.com/ApiZero/Phoenix-rs)
- **许可**：[MIT](LICENSE)

---

© 2026 极数本源 ApiZero. All rights reserved.
