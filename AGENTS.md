# AGENTS.md — Phoenix-rs 仓库对 AI / Agent 的强制约定

在本仓库（或基于本框架的应用）里做任何开发、脚手架、修 bug、写文档之前：

## 1. 必须先读 Skill

用 Read 工具完整阅读：

**[`.cursor/skills/phoenix/SKILL.md`](.cursor/skills/phoenix/SKILL.md)**

然后按需再读同目录：

- [`api-rust.md`](.cursor/skills/phoenix/api-rust.md) — Rust 路由 / 控制器 / 契约 / DB
- [`api-react.md`](.cursor/skills/phoenix/api-react.md) — `@phoenix/react` 导航 / 表单 / hooks

不要凭通用 Laravel / Axum / Next 经验硬套；以 Skill + `docs/` 为准。产品品牌为 **Phoenix-rs**；CLI 为 `px`（`cargo install px-cli`）；门面包 crates.io 为 `phoenixrs`。

## 2. 默认工作流

1. 新项目 / 新业务：优先 `px new`、`px make:*`，禁止手搓目录树。
2. 契约只写在 Rust（`#[phoenix::contract]`）；禁止手改 `views/generated/`。
3. React 用生成的 named action / `routes.ts`；破坏性写操作走 typed action，不用 method-spoofing Link。
4. 选库改 `config/database.toml`（sqlite / pgsql / mysql）；密钥放 `.env`。
5. 第三方能力用 `FeatureSet::plugin`（见 `docs/FEATURES.md`），不要隐式全局注册。
6. 上线用 `px release*`（见 `docs/RELEASE_PIPELINE.md`），不要直接覆盖服务器目录。
7. 改完跑对应测试：`cargo test` / `npm test --workspace=@phoenix/react`。

## 3. 人类文档

Skill 不够细时查：

- `docs/BUSINESS_GUIDE.md`、`docs/DX.md`、`docs/CONFIG.md`
- `docs/FEATURES.md`、`docs/RELEASE_PIPELINE.md`
- `docs/CONTRACTS.md`、`docs/RENDERING.md`、`docs/DATABASE.md`
- `docs/工具与约定.md`、`docs/PROGRESS.md`

出品：[极数本源 ApiZero](https://apizero.cn/)
