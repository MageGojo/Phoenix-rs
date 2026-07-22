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

下面的 React 契约仍是后续目标：

```tsx
import type { UserShowProps } from "#phoenix/contracts/pages/users";

export default function Show({ user }: UserShowProps) {
  return <h1>{user.name}</h1>;
}
```

## 当前状态

当前已实现第一版底层垂直切片：

- Hyper HTTP/1.1 服务启动、请求 body 限制、优雅关闭和测试用临时端口。
- Phoenix Request、Response、Handler、JSON 响应和异步控制器。
- GET、POST、PUT、PATCH、DELETE、HEAD/OPTIONS、路径参数和 404/405。
- Laravel 风格 `.name()`、名称前缀、路径前缀、命名 URL 生成和重复名称检查。
- 全局、单路由和路由组中间件。
- `required`、`string`、`min_length` 和 trait/闭包两种自定义验证规则。
- `examples/blog` 可运行案例及启动、路由、中间件、控制器、路由名和验证测试。

Toasty、React 契约、SPA/SSR/Islands 和迁移尚未实现。

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

运行完整检查：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
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

第一版聚焦常规服务端网站应用：控制器、路由、请求、验证、Rust 到 TypeScript 的自动数据契约、Toasty 模型与迁移、React SPA 页面响应、中间件、会话、CSRF、错误处理和测试工具。SSR 与 Islands 属于首个稳定版目标，但排在 SPA 垂直切片之后；CLI 代码生成、管理后台、队列、邮件、WebSocket 与插件市场不进入首个可用版本。
