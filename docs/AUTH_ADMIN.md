# 管理后台 / Auth 完整链路设计（2026-07-24）

## 快速开始：px make:auth

在任何 `px new` 应用里运行 `px make:auth`，一键生成认证起步套件：

- `routes/auth.rs`：`login.show/store`、`logout.store`、`register.show/store`、`password-reset.show/store` 命名路由，POST 接口带 `.action` 契约并叠加每 IP 2 req/s 的独立 `RateLimit`；
- `AuthController` + `LoginInput` / `RegisterInput` / `PasswordResetInput`（`Validate`）+ `AuthSessionResource` / `AuthMessageResource`；
- `views/pages/auth/{login,register,forgot-password}` React 表单页（`Form` + `form.field` + `FieldError`，中文文案）。

生成后立即可注册→登录（演示用进程内用户存储，明文比较）；上线前按控制器注释替换为
真实用户表（`px make:model User --all`）+ `password` feature（Argon2id），即本文的完整链路。
图形验证码非零配置（`phoenix-captcha` 是独立 crate），默认以注释交付，启用步骤见
`routes/auth.rs` 顶部与 [CAPTCHA.md](CAPTCHA.md)。已存在同名文件时拒绝覆盖，重建用 `--force`。

---

承接 `51f1284 feat(example): start admin auth journey` 的示例首版（固定演示账号 + token fixture），
本设计把链路上升为**持久化用户模型 + Session 登录 + `px make:auth` 生成器**，作为 crates.io RC 前的最后一块功能拼图。

## 目标与非目标

**目标（本批交付）**

1. `examples/blog`：Auth 示例从 fixture token 升级为真实持久化链路——
   - `users` 表（Toasty 模型 + 迁移），密码用 `phoenix-crypto::Password`（Argon2id）哈希；
   - 登录 / 登出 / 密码重置请求走 `SessionMiddleware`（Cookie Session + CSRF），不再用示例 token 中间件；
   - `/admin/dashboard` 由 Session 认证中间件保护，未登录 401/重定向；
   - 用户清单与审计事件改为读库（seeder 提供演示数据）。
2. `px make:auth` 生成器：在 `px new` 项目里一键生成上述完整骨架
   （User 模型 + 迁移 + Auth controller + Session 中间件 + 登录/注册页面 + 命名路由 + 契约 + React 页面）。

**非目标（明确不做，避免范围蔓延）**

- 邮箱真实发送（密码重置只记录 token，`phoenix-mail` MemoryTransport 展示）。
- RBAC/ABAC 与 admin 权限矩阵的深度集成（`phoenix-auth` 已存在，后续切片接入）。
- OAuth / 社交登录、MFA。
- `px make:admin` 独立生成器（管理后台壳子随 `make:auth` 的 admin 页面示例一并给出即可）。

## 关键决策

| 决策点 | 选择 | 理由 |
| --- | --- | --- |
| 凭证存储 | Argon2id PHC string（`phoenix-crypto::Password`） | 框架已有成熟门面，不引入新依赖 |
| 会话 | `SessionMiddleware` Cookie Session + CSRF | 与 SKILL 安全默认一致；React action 自动带 `X-CSRF-Token` |
| 中间件形态 | 示例 `RequireAuth`（读 Session 中 `user_id`，查库注入 `CurrentUser` extension） | 对齐现有「管理中间件通过 extensions 传递强类型上下文」约定 |
| 生成器落点 | `px make:auth`（新增子命令，复用 scaffold 的 `<phoenix:...>` 区块注册机制） | 与 `make:model --all` 一致的 DX |
| 数据库 | 示例默认 sqlite；迁移对三库通用 | 符合 DATABASE.md 约定 |

## 验收标准

- `cargo test --workspace --locked`、严格 Clippy、Rustfmt、`npm run ci:node` 全绿。
- blog 新增回归：注册→登录→访问 admin 200→登出→admin 401/重定向；错误密码 401；CSRF 缺失 403/419。
- `px make:auth` 在临时新项目生成后 `cargo check` + 前端 typecheck 通过，路由表含 `login.*` / `logout` / `admin.*`。
- 文档：本文件 + PROGRESS/NEXT 回填 + SKILL/DX 提及 `px make:auth`。
