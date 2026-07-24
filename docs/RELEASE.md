# 公开托管清单（GitHub + GitCode · 勿自动 push）

整理仓库、准备同步到 **GitHub** 与 **GitCode** 时使用。执行 `git push` / 创建远端仓库需**人工确认**后再动手。

权威产品仓名：**Phoenix-rs**  
组织：**ApiZero**

| 平台 | 仓库 URL | 建议 remote 名 |
| --- | --- | --- |
| GitHub | https://github.com/MageGojo/Phoenix-rs | `origin`（或 `github`） |
| GitCode | https://gitcode.com/Roufsi/Phoenix-rs | `gitcode` |

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

### GitHub（已于 2026-07-24 创建并 push）

实际仓库：`MageGojo/Phoenix-rs`（个人账号；ApiZero 组织不存在，无法在其下建仓）。当时执行：

`gh repo create MageGojo/Phoenix-rs --public --description "…" --homepage "https://apizero.cn/" --source=. --remote=origin --push`

若重建或换账号：

```bash
git remote add origin https://github.com/MageGojo/Phoenix-rs.git
git push -u origin main
```

### GitCode

1. 在 GitCode 组织 **ApiZero** 下创建空仓库 `Phoenix-rs`（不要勾选自动生成 README，避免首推冲突）。
2. 添加第二 remote 并 push：

```bash
git remote add gitcode https://gitcode.com/Roufsi/Phoenix-rs.git
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
| CLI | `px-cli` | `cargo install px-cli` → `px new` |
| 框架门面 | `phoenixrs` | `phoenix = { package = "phoenixrs", version = "…" }`；`use phoenix::` |

## npm 发布名（`px new` Registry 依赖）

`@phoenix` / `@phoenixrs` scope 当前不可用。前端包发在 **`@apizero/*`**（账号 `apizero`）：

| 角色 | 包名 | 目录 | 可安装 tarball |
| --- | --- | --- | --- |
| React 客户端 | `@apizero/react` | `packages/phoenix-react` | `…/@apizero/react/-/react-0.1.2.tgz` |
| SSR renderer | `@apizero/react-ssr` | `packages/phoenix-react-ssr` | `…/@apizero/react-ssr/-/react-ssr-0.1.2.tgz` |
| Vite 插件 | `@apizero/vite` | `packages/phoenix-vite` | `…/@apizero/vite/-/vite-0.1.3.tgz` |

说明：优先用 tarball URL 安装（历史 packument 偶发 404）。`@apizero/vite@0.1.3` 起默认 `resolve.dedupe` 消除生产双份 React。

```bash
npm run build:react
npm publish -w @apizero/react --access public
npm publish -w @apizero/vite --access public
npm publish -w @apizero/react-ssr --access public
curl -I https://registry.npmjs.org/@apizero/vite/-/vite-0.1.3.tgz
```

用户已授权实时同步时：`git push` 双远端 + `npm publish` / `cargo publish --registry crates-io` 按本清单执行。

说明：`phoenix`、`phoenix-rs`、`phoenix-cli`、`px`、`phoenix-core` 在 crates.io 已被无关项目占用，故不采用（`phoenix-core` → `phoenix-runtime`，`px` → `px-cli`）。详见 `docs/DECISIONS.md` ADR-009。

## crates.io 发布顺序（2026-07-24 验证）

所有 24 个拟发布 crate 已具备完整元数据（license/repository/description/keywords/categories），内部 path 依赖已全部带 `version = "0.1.0"`（对齐 `deny.toml` 要求）。
`cargo package --locked -p <crate>` 的 verify 阶段需要上游内部 crate **已经发布在 crates.io**——path 只用于本地解析，verify 时按 registry 版本解析。因此**本仓库内无法完整预演 `cargo publish --dry-run`**；验证手段为逐 crate `cargo package --locked --no-verify --list`（文件清单检查，全部通过）。

> **2026-07-24 更新**：`phoenix-core` 在 crates.io 被他方占用，已改名 **`phoenix-runtime`**（见下）；`px` 打包修复为内置 schemas。全部 24 个 crate 0.1.0 已发布完成（GitHub `MageGojo/Phoenix-rs`、GitCode `Roufsi/Phoenix-rs`）。

按内部依赖拓扑排序的发布顺序（同层可任意顺序；`cargo publish` 后需等索引进账再发下一层）：

1. 叶子层：`phoenix-config`、`phoenix-console`、`phoenix-database`、`phoenix-dx-macros`、`phoenix-http`、`phoenix-logging`、`phoenix-macros`、`phoenix-mail`、`phoenix-release`、`phoenix-routing`、`phoenix-storage`、`phoenix-validation`、`px`
2. 二层：`phoenix-crypto`、`phoenix-dx`、`phoenix-metrics`、`phoenix-plugin`、`phoenix-queue`、`phoenix-security`、`phoenix-view`
3. 三层：`phoenix-auth`、`phoenix-runtime`、`phoenix-redis`、`phoenix-testing`
4. 门面（最后）：`phoenixrs`

发布前仍红线：**不在未获用户明确确认前执行 `cargo publish` / `git push`**。

从 Git 安装 CLI（镜像任选）：

```bash
cargo install --git https://github.com/MageGojo/Phoenix-rs px-cli
cargo install --git https://gitcode.com/Roufsi/Phoenix-rs px-cli
```
