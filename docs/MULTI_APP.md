# 多应用项目

Phoenix 可以在一个 Rust 项目和一个服务进程中组织官网、用户前台、管理后台或独立 API。每个应用拥有自己的路由目录、路径或 Host、命名空间、中间件和强类型 State；公共模型与服务仍可放在项目共享模块。

## 推荐目录

```text
src/
├── apps/
│   ├── website.rs
│   ├── frontend.rs
│   └── admin.rs
├── domain/              # 跨应用共享的模型与业务服务
├── lib.rs
└── main.rs
```

完整可运行代码见 `examples/multi-app`。

## 组装三个应用

```rust
use phoenix::prelude::*;

#[derive(Clone)]
struct AppState {
    label: &'static str,
}

fn application() -> Result<Application, MultiApplicationError> {
    Application::multi()
        .mount(
            ApplicationModule::new("website", website_routes())
                .root()
                .state(AppState { label: "Website" }),
        )
        .mount(
            ApplicationModule::new("frontend", frontend_routes())
                .prefix("/app")
                .state(AppState { label: "Frontend" }),
        )
        .mount(
            ApplicationModule::new("admin", admin_routes())
                .state(AppState { label: "Admin" }),
        )
        .build()
}
# fn website_routes() -> Routes { Routes::new() }
# fn frontend_routes() -> Routes { Routes::new() }
# fn admin_routes() -> Routes { Routes::new() }
```

约定结果：

| 模块 | 路径 | 路由名示例 |
| --- | --- | --- |
| `website` | `/` | `website.home` |
| `frontend` | `/app` | `frontend.account` |
| `admin` | `/admin` | `admin.users.index` |

模块内仍按根路径声明路由。后台的 `Routes::new().get("/users", ...)` 最终地址是 `/admin/users`；`.name("users.index")` 最终名称是 `admin.users.index`。

## Host 绑定

```rust
ApplicationModule::new("admin", admin_routes())
    .root()
    .host("admin.APP_HOST")
```

绑定 Host 的模块优先于无 Host fallback；只写主机名时接受任意端口，写 `admin.APP_HOST:8443` 时要求精确端口。Host 选择用于虚拟站点分派，不替代 `HostAllowlist`、可信代理配置、认证或授权。

## 当前应用与隔离 State

```rust
async fn dashboard(
    State(state): State<AppState>,
    State(app): State<ApplicationContext>,
) -> String {
    format!("{} ({})", state.label, app.name())
}
```

`ApplicationContext` 提供当前应用的 `name()`、`path_prefix()` 和可选 `host()`。它存放在每个 Request 的 extensions 中，不使用进程全局变量，因此并发请求不会互相覆盖。

同一个 State 类型可以在不同应用中保存不同值。模块 `.middleware(...)` 与 `.state(...)` 只包裹该模块的路由；跨所有应用的安全策略应在每个模块显式复用同一构造函数，保持装配可见。

## 选择规则与启动失败

请求选择顺序为：

1. Host-bound 模块优先于无 Host 模块。
2. 带显式端口的 Host 绑定更精确。
3. 最长路径前缀优先，且必须处于 segment 边界。

重复应用名、重复 Host/path selector、无效 Host、无效应用名或跨应用重复命名路由都会在 `.build()` 时失败，不会在运行期静默覆盖。

## 单应用兼容

现有项目无需迁移：

```rust
let application = Application::new(routes())?;
```

只有需要多应用时才改用 `Application::multi()`；生成的仍是同一个 `Application`，HTTP/2、body 限制、优雅关闭、日志与测试调用方式保持一致。
