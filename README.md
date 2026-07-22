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
import type { UserShowProps } from "#phoenix/contracts/pages/users";

export default function Show({ user }: UserShowProps) {
  return <h1>{user.name}</h1>;
}
```

React 调用 Rust action 时使用后端路由名，不写死 URL。框架会把 Rust 命名路由表自动注入页面协议：

```rust
Routes::new()
    .post("/api/members", MemberController::store)
    .name("members.store");
```

```tsx
const member = await callRust<Member>("members.store", { name });
```

## 当前状态

当前已实现第一版底层垂直切片：

- Hyper HTTP/1.1 服务启动、请求 body 限制、优雅关闭和测试用临时端口。
- Phoenix Request、Response、Handler、JSON 响应和异步控制器。
- GET、POST、PUT、PATCH、DELETE、HEAD/OPTIONS、路径参数和 404/405。
- Laravel 风格 `.name()`、名称前缀、路径前缀、命名 URL 生成和重复名称检查。
- 全局、单路由和路由组中间件。
- `field("user", rules![...])`、内置规则和 trait/闭包两种自定义验证规则。
- JSON Content-Type 检查、严格路径解码、panic 隔离和不泄露内部错误的 500 响应。
- 可配置 body、请求头读取和优雅关闭超时，以及基础安全响应头中间件。
- `examples/blog` 可运行案例及启动、路由、中间件、控制器、路由名和验证测试。

React 页面协议、三种渲染模式、浏览器启动器、持久 Node SSR renderer 和可选 AES-256-GCM 页面信封已经完成第一版垂直切片。当前 renderer 使用单 worker、2 秒 deadline、启动握手与一次崩溃恢复；多 worker 池、Vite 自动发现/生产清单、Toasty、迁移、TLS、会话、CSRF、可信代理和限流尚未实现。当前版本不能直接视为完整的生产安全栈。

- [产品需求](docs/PRODUCT.md)
- [架构设计](docs/PROJECT.md)
- [开发者体验草案](docs/DX.md)
- [Rust/TypeScript 数据契约](docs/CONTRACTS.md)
- [React 渲染模式](docs/RENDERING.md)
- [安全与数据传输](docs/SECURITY.md)
- [技术决策](docs/DECISIONS.md)
- [当前进度](docs/PROGRESS.md)
- [下一阶段](docs/NEXT.md)

## 快速运行

```bash
cargo run -p phoenix-blog-example
```

服务默认监听 `http://127.0.0.1:3000`：

```bash
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3000/users/Ada
```

React 案例需要同时启动 Vite：

```bash
npm run build:ssr
npm run dev -w phoenix-blog-react-example
cargo run -p phoenix-blog-example
```

`build:ssr` 在页面组件变化后重新生成服务端 bundle。成员目录位于 `http://127.0.0.1:3000/members`，每次请求的 100 条初始数据由 Rust 生成；常驻 Node renderer 输出完整首屏 HTML，浏览器只加载成员目录 island。新增成员通过命名路由 `members.store` 调用 Rust 接口，搜索、筛选、排序和分页继续在 island 内交互。

运行完整检查：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
npm run build:react
npm run build:ssr
npm run typecheck:example
npm run test:react
```

## 仓库结构

```text
crates/phoenix-http/    请求、响应、Handler 与中间件
crates/phoenix-routing/ 路由、分组和命名 URL
crates/phoenix-core/    Hyper 服务与应用生命周期
crates/phoenix-validation/ 验证规则与错误
crates/phoenix/         应用使用的统一入口
packages/phoenix-react/ React 客户端适配层
packages/phoenix-vite/  Vite 页面、契约与渲染构建插件
examples/blog/          贯穿开发过程的参考应用
docs/                   产品、架构与项目记录
```

## 第一版边界

第一版聚焦常规服务端网站应用：控制器、路由、请求、验证、Rust 到 TypeScript 的自动数据契约、Toasty 模型与迁移、React 页面响应、中间件、会话、CSRF、错误处理和测试工具。React 默认采用 Islands，并允许页面显式切换 SPA 或 SSR；CLI 代码生成、管理后台、队列、邮件、WebSocket 与插件市场不进入首个可用版本。
