# 发布候选收口批工作计划（2026-07-24）

目标：在管理后台 / Auth 完整链路开工前，把发布候选（RC）的工程底盘收口。
用户指令（原文）：格式化 Rust / 修 React TypeScript / 补真实 PG/MySQL/Redis CI / 修 crates.io packaging / 拆分提交 / 再开始管理后台和 auth 完整链路。

## 对账结果（动手前）

- `git status` 干净，最新 commit `51f1284 feat(example): start admin auth journey`。
- `cargo fmt --all -- --check` 通过；`npm run typecheck:example` 通过。
- CI 中 `postgres` / `mysql` / `redis` 真实 service job **已存在**（ci.yml），但从未托管首跑确认（见 PROGRESS 2026-07-23「生产工程门禁」）。
- `cargo package --locked --no-verify --list` 抽查通过，但 `--no-verify` 只是文件清单检查，未验证「打包后 crate 能否独立编译」。
- deny.toml 注明：workspace path dependency 在技术预览阶段只告警，**发布 crate 前必须给内部依赖增加明确 version**——这是 crates.io packaging 的最大已知缺口。

## 步骤与验收标准

### 1. 完整本地基线（进行中）

- `cargo clippy --workspace --all-targets --locked -- -D warnings` 全绿
- `cargo test --workspace --locked` 全绿
- `npm run ci:node` 全绿
- `cargo package --locked --no-verify --list` 全 22 个 crate 通过

### 2. 真实 PG / MySQL / Redis 契约验证

- CI 已含三个 service job；本批补齐的是**本地真实复跑证据**（Docker 一次性容器 + 环境变量门控测试），确认 job 定义与测试门控环境变量一致，不需要改代码时只更新文档记录。
- 若本地无法跑 Docker，则记录为「托管 CI 首跑确认」，不虚假宣称。

### 3. crates.io packaging 修复

- 为拟发布的 crate 补齐 crates.io 必需元数据（license/repository/homepage/description/readme 等，缺的才补）。
- 内部 path 依赖补 `version = "0.1.0"`（对齐 deny.toml 要求与 crates.io 发布硬性要求）。
- 用 `cargo publish --dry-run`（或等价的 `cargo package` 带 verify）抽查核心 crate。
- 任何变更用独立 commit 落地（拆分提交）。

### 4. 管理后台 / Auth 完整链路

- 基于 `examples/blog` 已有首版（固定演示账号 fixture）上升为：持久化用户模型（Toasty + Argon2id）+ Session 登录 + `px make:auth` / `px make:admin` 生成器。
- 验收：脚手架新项目跑通注册/登录/登出/受保护 admin 页面；workspace 测试与 ci:node 全绿。
- 该项体量大，单独发起 subagent 并行前先把设计写入 docs。

## 风险与红线

- 不 push、不发版到 crates.io；`cargo publish --dry-run` 只读验证。
- 所有改动小步提交，commit message 遵循仓库 conventional 风格（见 `git log`）。
