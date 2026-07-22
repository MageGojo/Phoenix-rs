# Phoenix

Phoenix 是一个早期开发阶段、以 Hyper 为 HTTP 核心的 Rust 网站应用框架。项目目标是在 Rust 的类型安全与性能基础上，提供接近 Laravel 的开发体验，并默认集成 React + TypeScript 视图。

> `Phoenix` 目前是工作名称，与 Elixir 生态中的 Phoenix Framework 存在重名风险。正式发布前必须完成命名与 crate 可用性评估。

## 目标体验

开发者负责路由、控制器、模型和 `views/` 下的 `.tsx` / `.jsx` 文件。框架负责请求解析、验证、数据库访问、React 页面响应、错误处理和安全默认值。

底层控制器和路由已经可以运行：

```rust
pub struct UserController;

impl UserController {
    pub async fn show(request: Request) -> Response {
        let user = request.param("user").unwrap_or("unknown");
        Json(json!({ "user": user })).into_response()
    }
}

Routes::new()
    .get("/users/{user}", UserController::show)
    .name("users.show")
```

React 页面通过同一个后端协议支持 Islands、SPA 与 SSR；默认使用 Islands：

```tsx
import MemberCreator from "../../islands/member-creator";

export default function Members({ members }: MembersProps) {
  return (
    <main>
      <MemberTable members={members} />
      <MemberCreator client:load initialTotal={members.length} />
    </main>
  );
}
```

`MemberTable` 只输出 SSR HTML；Vite 在编译期识别 `client:load`，SSR renderer 自动收集 `MemberCreator` 的 props，浏览器只动态加载页面实际出现的 island。业务代码不写 island 注册表、浏览器入口或 renderer 入口。

React 调用 Rust action 时使用后端路由名，不写死 URL。框架会把 Rust 命名路由表自动注入页面协议：

```rust
Routes::new()
    .post("/api/members", typed(MemberController::store))
    .name("members.store")
    .action::<StoreMemberInput, MemberResource>();
```

```tsx
import { members } from "../generated/routes.js";

const member = await members.store({ name });
```

路由属性由 `phoenix-vite` 自动生成，详细用法见 [业务开发指南](docs/BUSINESS_GUIDE.md#从-react-调用-rust-action)。

## 当前状态

当前已实现第一版底层垂直切片：

- Hyper HTTP/1.1 + HTTP/2 自动识别、协议限制、请求 body 限制、优雅关闭和测试用临时端口。
- Phoenix Request、Response、Handler、JSON 响应和异步控制器。
- Query、Path、Header、JSON、Form、Multipart 与 `State<T>` extractor，以及验证后的强类型 DTO handler。
- GET、POST、PUT、PATCH、DELETE、HEAD/OPTIONS、路径参数和 404/405。
- Laravel 风格 `.name()`、名称前缀、路径前缀、命名 URL 生成和重复名称检查。
- `routes/*.rs` 自动挂载、REST resource routes、中间件别名与异步模型绑定。
- 全局、单路由和路由组中间件。
- `field("user", rules![...])`、内置规则和 trait/闭包两种自定义验证规则。
- Rust Input、Resource、Page Props 与 Shared Props 自动生成 TypeScript，并生成可直接调用的命名 action。
- JSON Content-Type 检查、严格路径解码、panic 隔离和不泄露内部错误的 500 响应。
- 可配置 body、请求头读取和优雅关闭超时，以及基础安全响应头中间件。
- 开发文本/生产 JSON 两种结构化日志格式、`PHOENIX_LOG` 过滤和脱敏访问日志。
- Prometheus exporter：HTTP 延迟/状态、TCP 连接、TLS、renderer、数据库、队列及安全状态指标，标签集合固定且不包含用户输入。
- 版本化共享 Session backend：异步 load/create/save/rotate/delete、CAS 冲突、滑动 TTL、默认失败关闭、成功后 Cookie 提交和双实例共享测试。
- HS256 JWT、refresh rotation、reuse detection、access/family 撤销和持久化 token store，以及 AES-256-GCM 应用数据加密与 Argon2id 密码哈希。
- 默认拒绝的 RBAC/ABAC：角色继承、直接 allow/deny、资源属性 policy、决策审计、JWT principal 映射和 401/403 权限中间件。
- 每请求 CSP nonce 自动贯穿 Header、React HTML、Vite runtime meta、CSS/模块/hydration script 与流式 renderer；带 nonce 的 HTML 禁止共享缓存。
- 单进程多应用：官网、用户前台、管理后台可按 Host/路径独立挂载，拥有路由命名空间与强类型 State。
- `routes!` 与 `applications!` 声明宏减少路由和多应用组装样板，同时完整保留 builder API。
- Laravel 风格 `px new` 与控制器、模型、迁移、Request、Resource、中间件、页面和 Island 生成器。
- `examples/blog` 可运行案例及启动、路由、中间件、控制器、路由名和验证测试。

React 页面协议、三种渲染模式、自动页面/island 发现、Rust/TypeScript 契约、受控 `PageHead`、版本化生产资源、可配置 Node renderer 池、流式 SSR 和可选 AES-256-GCM 页面信封已经形成完整垂直切片。renderer 提供 deadline、资源/契约握手、健康快照、故障替换与显式关闭；Web 栈已提供 TLS/HTTPS/ALPN、服务端 Session、JWT token 生命周期、RBAC/ABAC、自动 action CSRF、精确 CORS、可信代理、Host allowlist、分布式 Session/限流 contract、指标 exporter、自动 CSP nonce、安全头、request ID、日志脱敏以及安全重定向/下载响应。实时协议、生产共享存储适配器和独立安全评审仍是生产发布前置条件。

- [产品需求](docs/PRODUCT.md)
- [架构设计](docs/PROJECT.md)
- [业务开发指南](docs/BUSINESS_GUIDE.md)
- [开发者体验与路由约定](docs/DX.md)
- [多应用项目](docs/MULTI_APP.md)
- [数据库与迁移](docs/DATABASE.md)
- [Rust/TypeScript 数据契约](docs/CONTRACTS.md)
- [React 渲染模式](docs/RENDERING.md)
- [安全与数据传输](docs/SECURITY.md)
- [认证、授权与 Token 生命周期](docs/AUTHORIZATION.md)
- [Prometheus 指标](docs/METRICS.md)
- [技术决策](docs/DECISIONS.md)
- [当前进度](docs/PROGRESS.md)
- [下一阶段](docs/NEXT.md)

## 快速运行

安装本地 CLI 后，可以直接创建一套已经连接 Rust、React、Vite、Toasty 和类型契约的新应用：

```bash
cargo install --path crates/phoenix-cli
px new my-app
cd my-app
px dev
```

`px new` 默认安装 npm 依赖、初始化本地 Git，并按开发/生产环境装配请求级 CSP nonce；从当前源码构建的 CLI 会自动使用本地 Phoenix crates/packages。生成业务切片可以运行 `px make:model Post --all`，完整命令见[开发者体验与 CLI](docs/DX.md#21-项目与业务代码生成)。

运行仓库内参考应用：

```bash
cd examples/blog
px dev
```
服务默认监听 `http://127.0.0.1:3000`：

```bash
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3000/users/Ada
```

该命令在应用目录同时启动 Rust 与 Vite，并统一处理退出信号。只启动后端也可以运行：

```bash
cargo run -p phoenix-blog-example
```

`build:ssr` 在页面组件变化后重新生成服务端 bundle。成员目录位于 `http://127.0.0.1:3000/members`，每次请求的 100 条初始数据由 Rust 生成；常驻 Node renderer 输出概览与成员表格，浏览器只加载带 `client:load` 的新增成员表单。新增成员通过命名路由 `members.store` 调用 Rust 接口。

运行完整检查：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
npm run build:react
npm run build:client -w phoenix-blog-react-example
npm run build:ssr -w phoenix-blog-react-example
npm run typecheck:example
npm run test:react
```

## 仓库结构

```text
crates/phoenix-http/    请求、响应、Handler 与中间件
crates/phoenix-logging/ tracing 文本/JSON 初始化与日志过滤
crates/phoenix-metrics/ Prometheus registry、HTTP 采集与运行时指标
crates/phoenix-routing/ 路由、分组和命名 URL
crates/phoenix-core/    Hyper 服务与应用生命周期
crates/phoenix-crypto/  JWT、AES-GCM 与 Argon2id 密码学门面
crates/phoenix-auth/    RBAC、ABAC、principal 与授权中间件
crates/phoenix-database/ Toasty、迁移与测试隔离
crates/phoenix-dx/      resource routes、中间件别名与模型绑定
crates/phoenix-cli/     项目/业务代码生成与 Rust + Vite 开发进程监督
crates/phoenix-security/ Session、CSRF 与 Web 安全中间件
crates/phoenix-validation/ 验证规则与错误
crates/phoenix-view/    页面协议、生产资源与 renderer 池
crates/phoenix/         应用使用的统一入口
packages/phoenix-react/ React 客户端适配层
packages/phoenix-vite/  Vite 页面、契约与渲染构建插件
examples/blog/          贯穿开发过程的参考应用
examples/multi-app/     官网、前台与后台共存的最小多应用案例
docs/                   产品、架构与项目记录
```

## 第一版边界

第一版聚焦常规服务端网站应用：CLI 项目/业务代码生成、控制器、路由、请求、验证、Rust 到 TypeScript 的自动数据契约、Toasty 模型与迁移、React 页面响应、中间件、会话、CSRF、错误处理和测试工具。React 默认采用 Islands，并允许页面显式切换 SPA 或 SSR；管理后台、队列、邮件、WebSocket 与插件市场不进入首个可用版本。
