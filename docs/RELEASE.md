# 公开托管清单（GitHub + GitCode · 勿自动 push）

整理仓库、准备同步到 **GitHub** 与 **GitCode** 时使用。执行 `git push` / 创建远端仓库需**人工确认**后再动手。

权威产品仓名：**Phoenix-rs**  
组织：**ApiZero**

| 平台 | 仓库 URL | 建议 remote 名 |
| --- | --- | --- |
| GitHub | https://github.com/ApiZero/Phoenix-rs | `origin`（或 `github`） |
| GitCode | https://gitcode.com/ApiZero/Phoenix-rs | `gitcode` |

crates.io 元数据 `repository` 继续指向 GitHub（见根 `Cargo.toml`），与国际包索引惯例一致；README 同时列出 GitCode 镜像。

## 应纳入 Git

- `crates/`、`packages/`、`examples/blog`、`examples/multi-app`、`examples/plugin-greeter`
- `schemas/`、`taplo.toml`（config JSON Schema）
- `fuzz/`（框架 fuzz 目标与空 corpus 占位）、`benchmarks/`
- `.github/workflows/`、`deny.toml`、`.gitleaks.toml`
- `.cursor/skills/phoenix/`（**官方 AI Skill，必须入库**）、根目录 `AGENTS.md`
- `docs/`（产品与架构文档）
- `README.md`、`LICENSE`、`Cargo.toml` / lock、根 `package.json`

## 不应纳入 Git

| 路径 / 模式 | 原因 |
| --- | --- |
| `.cursor/*`（**除** `skills/phoenix/`） | 本地 Agent 配置；Skill 已强制跟踪 |
| `examples/zhizaojia/` | 业务私有示例（非框架演示） |
| `**/config/credentials.toml` | 密钥 |
| `**/data/requests/`、captures | 抓包 / 运行时数据 |
| `fuzz/target`、`artifacts`、corpus 样本 | fuzz 产物 |
| `target/`、`node_modules/`、`dist/`、生成 `views/generated/*` | 构建产物 |

## 发布前检查

```bash
# 1. 确认无密钥
rg -n "sk_live_|BEGIN (RSA |OPENSSH )?PRIVATE|password\s*=\s*['\"][^'\"]{8}" \
  --glob '!.git' --glob '!**/target/**' --glob '!**/node_modules/**' \
  --glob '!examples/zhizaojia/**' || true

# 2. 状态干净且忽略路径未被 force-add
git status
git check-ignore -v .cursor examples/zhizaojia 2>/dev/null || true

# 3. 测试（按环境选跑）
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

# 4. 人工确认后：创建空仓库 → 添加 remote → 双端 push
```

## 建议仓库元数据（两端保持一致）

- **仓库名**：`Phoenix-rs`
- **Description**：`Phoenix-rs — Rust full-stack web framework by ApiZero (Laravel-inspired DX + React)`
- **Homepage**：`https://apizero.cn/`
- **Topics / 标签**：`rust`, `web-framework`, `hyper`, `react`, `typescript`, `ssr`, `phoenix-rs`
- **License**：MIT（与 `LICENSE` 一致）
- **可见性**：Public（框架开源）

### GitHub

- 用 `gh repo create ApiZero/Phoenix-rs --public --source=. --remote=origin --push`  
  **或** 网页创建空仓后：

```bash
git remote add origin https://github.com/ApiZero/Phoenix-rs.git
git push -u origin main
```

### GitCode

1. 在 GitCode 组织 **ApiZero** 下创建空仓库 `Phoenix-rs`（不要勾选自动生成 README，避免首推冲突）。
2. 添加第二 remote 并 push：

```bash
git remote add gitcode https://gitcode.com/ApiZero/Phoenix-rs.git
git push -u gitcode main
```

之后每次发布同步：

```bash
git push origin main
git push gitcode main
```

> Agent **不得**在未获用户明确确认前执行 push。确认话术示例：「确认上传 GitHub + GitCode」。

## crates.io 发布名（与托管品牌对照）

| 角色 | 包名 | 安装 / 依赖 |
| --- | --- | --- |
| CLI | `px` | `cargo install px` → `px new` |
| 框架门面 | `phoenixrs` | `phoenix = { package = "phoenixrs", version = "…" }`；`use phoenix::` |

说明：`phoenix`、`phoenix-rs`、`phoenix-cli` 在 crates.io 已被无关项目占用，故不采用。详见 `docs/DECISIONS.md` ADR-009。

从 Git 安装 CLI（镜像任选）：

```bash
cargo install --git https://github.com/ApiZero/Phoenix-rs px
cargo install --git https://gitcode.com/ApiZero/Phoenix-rs px
```
