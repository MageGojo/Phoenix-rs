# Phoenix 业务开发指南

本文只说明如何使用 Phoenix 编写网站业务代码。示例均来自仓库中的
`examples/blog`，并且只使用当前已经实现的公开接口。

当前可以编写：

- 应用配置与服务启动；
- GET、POST、PUT、PATCH、DELETE 路由；
- 路径参数、命名路由和路由分组；
- 异步控制器；
- Query、Path、Header、JSON、Form、Multipart 强类型请求提取；
- 验证后的 DTO、字段错误和 JSON 响应；
- 全局、单路由和路由组中间件；
- Toasty SQLite/PostgreSQL CRUD、关系、游标分页与事务；
- 带 checksum、锁和回滚的迁移；
- Session、CSRF、CORS、限流、可信代理、Host allowlist 和安全响应头；
- React Islands、SPA 与 SSR 页面；
- 版本化生产资源、renderer worker 池和流式 SSR；
- 明文或可选 AES-256-GCM 页面协议；
- 业务单元测试和功能测试。

Vite 自动页面/Island 发现、Rust/TypeScript 字段契约、可调用命名 action、多 worker renderer 池和生产资源版本校验均已实现。本文只使用当前仓库中已经通过 Rust、TypeScript 和 React 测试的接口；各领域的完整配置分别链接到专项文档。

## 1. 推荐的业务目录

当前示例采用下面的目录组织业务代码：

```text
examples/blog/
├── app/
│   ├── controllers/mod.rs   # 控制器
│   ├── middleware/mod.rs    # 业务中间件
│   ├── props/mod.rs         # 页面与共享 Props
│   ├── requests/mod.rs      # 请求 DTO 与验证规则
│   └── resources/mod.rs     # 对浏览器公开的 Resource
├── routes/
│   └── web.rs               # Web 路由
├── src/
│   ├── lib.rs               # 组装应用
│   └── main.rs              # 启动服务
└── tests/
    ├── feature/             # HTTP 业务流程测试
    └── unit/                # 验证规则等单元测试
```

业务代码通常从 `phoenix::prelude` 导入常用类型：

```rust
use phoenix::prelude::{
    Application, IntoResponse, Json, Request, Response, Routes, StatusCode,
};
```

新业务优先从 CLI 生成，避免手动创建目录、`mod.rs`、模型/迁移注册表和前端契约入口：

```bash
px new my-app
cd my-app
px make:model Post --all
px dev
```

`--all` 会生成模型、迁移、验证 Request、Resource、控制器、命名路由、Page Props 和 React 页面，并刷新 TypeScript action/类型。单项命令和覆盖规则见[项目与业务代码生成](DX.md#21-项目与业务代码生成)。生成的是可运行业务骨架，字段、数据库查询和迁移 SQL 仍应按实际业务修改。

## 2. 创建应用

在 `src/lib.rs` 中集中创建应用，并设置请求限制和超时：

```rust
use std::time::Duration;

use phoenix::prelude::{Application, RouteBuildError, Routes};

#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[path = "../app/requests/mod.rs"]
pub mod requests;
pub fn routes() -> Routes {
    phoenix::mount_routes!()
}

pub fn application() -> Result<Application, RouteBuildError> {
    Application::new(routes()).map(|application| {
        application
            .max_body_size(64 * 1024)
            .header_read_timeout(Duration::from_secs(5))
            .body_read_timeout(Duration::from_secs(10))
            .graceful_shutdown_timeout(Duration::from_secs(5))
    })
}
```

这些限制应根据业务调整：

- `max_body_size`：单次请求 body 的最大字节数；
- `header_read_timeout`：等待客户端发送完整请求头的最长时间；
- `body_read_timeout`：读取请求 body 的最长时间；
- `graceful_shutdown_timeout`：停止服务时等待现有请求完成的最长时间。

不要在公开服务中移除这些限制。文件上传等大请求应使用单独且明确的限制策略。

## 3. 启动服务

在 `src/main.rs` 中绑定监听地址并处理 Ctrl+C：

```rust
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::var("APP_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_owned());

    let server = phoenix_blog_example::application()?
        .bind(&address)
        .await?;

    println!("Listening on http://{}", server.local_addr());

    server
        .run_with_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    Ok(())
}
```

运行示例：

```bash
cargo build -p phoenix-cli
cd examples/blog
../../target/debug/px dev
```

`px dev` 同时启动 `cargo run` 与 `npm run dev -- --strictPort`。Ctrl-C 或任一进程提前退出时，另一侧的整个子进程组也会被回收。只调试后端时仍可单独运行 `cargo run -p phoenix-blog-example`。

也可以通过环境变量修改监听地址：

```bash
APP_ADDR=0.0.0.0:8080 cargo run -p phoenix-blog-example
```

## 4. 定义路由

控制器方法可以直接注册为路由处理器：

```rust
use phoenix::prelude::Routes;

use crate::controllers::{RegistrationController, UserController};

pub fn routes() -> Routes {
    Routes::new()
        .get("/users/{user}", UserController::show)
        .name("users.show")
        .post("/register", RegistrationController::store)
        .name("register.store")
}
```

当前支持：

```rust
Routes::new()
    .get("/articles", ArticleController::index)
    .post("/articles", ArticleController::store)
    .put("/articles/{article}", ArticleController::replace)
    .patch("/articles/{article}", ArticleController::update)
    .delete("/articles/{article}", ArticleController::destroy)
```

框架会自动处理以下情况：

- GET 路由自动支持 HEAD，并返回空 body；
- OPTIONS 返回该路径允许的方法；
- 路径不存在时返回 404；
- 路径存在但请求方法不匹配时返回 405 和 `Allow` 响应头。

### 路径参数

使用 `{参数名}` 声明动态路径：

```rust
.get("/users/{user}", UserController::show)
```

在控制器中读取：

```rust
pub async fn show(request: Request) -> Response {
    let user = request.param("user").unwrap_or("unknown");
    Json(serde_json::json!({ "user": user })).into_response()
}
```

参数会进行严格的 URL 解码。例如 `/users/Ada%20Lovelace` 中的 `user` 是
`Ada Lovelace`。无效编码会被拒绝，不应由业务控制器自行猜测或修复。

### 命名路由和 URL 生成

路由名用于稳定地生成 URL，业务代码不必重复拼接路径：

```rust
let router = routes().build()?;

let url = router.url(
    "users.show",
    &[("user", "Ada Lovelace")],
)?;

assert_eq!(url, "/users/Ada%20Lovelace");
```

缺少路径参数或使用不存在的路由名时，`url` 会返回错误。重复的路由名会在构建路由时返回错误。

### 路由分组

相关业务路由可以共享路径前缀、名称前缀和中间件：

```rust
use phoenix::prelude::{RouteGroup, Routes};

Routes::new().group(
    RouteGroup::new()
        .prefix("/admin")
        .name("admin.")
        .middleware(RequireExampleToken),
    |routes| {
        routes
            .get("/dashboard", AdminController::dashboard)
            .name("dashboard")
    },
)
```

最终路径是 `/admin/dashboard`，完整路由名是 `admin.dashboard`。

### 自动路由文件与 resource routes

`phoenix::mount_routes!()` 按文件名排序挂载 `routes/*.rs`；每个文件统一导出 `pub fn routes() -> Routes`。扫描只覆盖第一层 `.rs` 文件并忽略 `mod.rs`。

常规 CRUD 使用 resource routes：

```rust
use phoenix::prelude::*;

pub fn routes() -> Routes {
    Routes::new().resource(
        "articles",
        "/articles",
        Resource::new()
            .index(ArticleController::index)
            .create(ArticleController::create)
            .store(ArticleController::store)
            .show(ArticleController::show)
            .edit(ArticleController::edit)
            .update(ArticleController::update)
            .destroy(ArticleController::destroy),
    )
}
```

生成的命名路由是 `articles.index/create/store/show/edit/update/destroy`；update 同时注册 PUT 与 PATCH。用 `.only([...])` 或 `.except([...])` 裁剪动作，用 `.parameter("article")` 覆盖路径参数名。只有配置了 handler 的动作才会生成。

## 5. 编写控制器

控制器可以使用结构体组织，每个动作是接收 `Request` 的异步关联函数：

```rust
use phoenix::prelude::{IntoResponse, Json, Request, Response};
use serde_json::json;

pub struct UserController;

impl UserController {
    pub async fn show(request: Request) -> Response {
        let user = request.param("user").unwrap_or("unknown");

        Json(json!({
            "user": user,
            "route": request.route_name(),
        }))
        .into_response()
    }
}
```

控制器目前可以读取：

```rust
let method = request.method();
let uri = request.uri();
let headers = request.headers();
let raw_body = request.body();
let user = request.param("user");
let route_name = request.route_name();
```

请求头读取示例：

```rust
let request_id = request
    .headers()
    .get("x-request-id")
    .and_then(|value| value.to_str().ok());
```

不要在 panic 信息或公开响应中写入密码、令牌、数据库连接信息等敏感数据。

## 6. 提取强类型请求

控制器参数使用 extractor 后，框架会在进入业务代码前完成反序列化。路由用
`typed(...)` 挂载这类 handler：

```rust
use phoenix::prelude::{Query, typed};
use serde::Deserialize;

#[derive(Deserialize)]
struct SearchQuery {
    page: u32,
    term: String,
}

async fn search(Query(query): Query<SearchQuery>) -> Json<SearchResult> {
    // query.page 和 query.term 已经是 Rust 强类型字段。
}

Routes::new().get("/search", typed(search));
```

可用 extractor：

| 类型 | 数据来源 |
| --- | --- |
| `Query<T>` | URL query string |
| `Path<T>` | `{parameter}` 路径参数 |
| `Header<T>` | 请求头，字段通常用 `#[serde(rename = "x-...")]` |
| `Json<T>` | JSON body |
| `Form<T>` | `application/x-www-form-urlencoded` body |
| `Multipart<T>` | `multipart/form-data`；`T: FromMultipart` 时直接形成业务 DTO |

一个 handler 最多可以组合四个 extractor。解析失败时框架直接返回稳定的
400、415 或 422 响应，业务函数不会收到半解析数据。底层 `request.json::<T>()`
仍然保留，适合中间件或需要自行控制错误映射的代码。

Multipart 默认类型是 `MultipartData`，可以按名称读取或移除 `MultipartField`。
业务上传 DTO 实现 `FromMultipart` 后可直接写成 `Multipart<UploadInput>`；如果该
DTO 同时实现 `Validate`，`Validated<Multipart<UploadInput>>` 会复用相同的 422
字段错误流程。框架只返回客户端文件名和字节，不会把文件名直接当成本地路径。

客户端必须发送下列 JSON Content-Type 之一：

```text
application/json
application/json; charset=utf-8
application/vnd.example+json
```

错误映射如下：

| 情况 | 状态码 |
| --- | ---: |
| 缺少 Content-Type | 415 Unsupported Media Type |
| Content-Type 不是 JSON | 415 Unsupported Media Type |
| JSON 格式错误或无法反序列化 | 400 Bad Request |

使用 `Json<T>` 时，这些错误由 typed handler 自动映射为 JSON 响应。

## 7. 验证业务字段

将 DTO 和验证规则放在 `app/requests`，避免控制器同时承担输入规则和业务流程：

```rust
use phoenix::prelude::{Validate, ValidationErrors, Validator, max_length, required, rules, string};
use serde::Deserialize;

#[phoenix::contract(input)]
#[derive(Deserialize)]
pub struct StoreMemberInput {
    pub name: String,
}

impl Validate for StoreMemberInput {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({ "name": self.name });
        Validator::new(&data)
            .field("name", rules![required(), string(), max_length(40)])
            .validate()
    }
}
```

`Validated<Json<T>>` 先提取 JSON，再执行 `T::validate()`。控制器只处理有效 DTO：

```rust
pub async fn store(
    Validated(Json(input)): Validated<Json<StoreMemberInput>>,
) -> (StatusCode, Json<MemberResource>) {
    (
        StatusCode::CREATED,
        Json(MemberResource::new(input.name)),
    )
}
```

验证失败自动返回 422。一次验证会收集所有字段错误，每项包含规则名和消息：

```json
{
  "errors": {
    "password": [
      {
        "rule": "min_length",
        "message": "The password field must be at least 8 characters."
      }
    ]
  }
}
```

服务端验证是最终依据。即使前端已经验证，也必须保留服务端验证。

### 使用 trait 编写可复用规则

```rust
use std::borrow::Cow;

use phoenix::prelude::{Rule, RuleContext};
use serde_json::Value;

pub struct NotReservedUser;

impl Rule for NotReservedUser {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("not_reserved")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        let reserved = context
            .value
            .and_then(Value::as_str)
            .is_some_and(|user| {
                ["admin", "root"]
                    .contains(&user.to_ascii_lowercase().as_str())
            });

        if reserved {
            Err("The user field contains a reserved name.".to_owned())
        } else {
            Ok(())
        }
    }
}
```

然后像内置规则一样使用：

```rust
.field(
    "user",
    rules![required(), string(), NotReservedUser],
)
```

### 使用闭包编写一次性规则

闭包规则可以同时读取当前字段和完整请求数据，适合确认密码等跨字段验证：

```rust
use phoenix::prelude::{custom_rule, required, rules, Validator};

let confirmed = custom_rule("confirmed", |context| {
    let confirmation = context.data.get("password_confirmation");

    if context.value == confirmation {
        Ok(())
    } else {
        Err(format!(
            "The {} confirmation does not match.",
            context.field,
        ))
    }
});

let validator = Validator::new(&payload)
    .field("password", rules![required(), confirmed]);
```

## 8. 编写中间件

中间件适合处理鉴权、请求上下文和统一响应头。可复用中间件实现
`Middleware`：

```rust
use phoenix::{
    http::{BoxFuture, HeaderName, HeaderValue},
    prelude::{Middleware, Next, Request, Response},
};

pub struct PoweredByPhoenix;

impl Middleware for PoweredByPhoenix {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let mut response = next.run(request).await;
            response.headers_mut().insert(
                HeaderName::from_static("x-powered-by"),
                HeaderValue::from_static("Phoenix"),
            );
            response
        })
    }
}
```

调用 `next.run(request).await` 才会继续执行后续中间件和控制器。鉴权失败时可以直接返回响应：

```rust
impl Middleware for RequireExampleToken {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let authorized = request
                .headers()
                .get("x-example-token")
                .is_some_and(|value| value == "secret");

            if authorized {
                next.run(request).await
            } else {
                Response::text("Unauthorized")
                    .with_status(StatusCode::UNAUTHORIZED)
            }
        })
    }
}
```

示例中的硬编码令牌只用于演示。真实业务不能硬编码凭证，也不能把这种检查当作完整的登录或会话方案。

### 挂载中间件

全局中间件作用于所有路由：

```rust
Routes::new()
    .with_middleware(SecurityHeaders)
    .with_middleware(PoweredByPhoenix)
```

单路由中间件只作用于紧邻的那条路由：

```rust
Routes::new()
    .get("/profile", ProfileController::show)
    .middleware(RequireLogin)
```

路由组中间件作用于组内全部路由：

```rust
RouteGroup::new()
    .prefix("/admin")
    .middleware(RequireAdmin)
```

一次性中间件可以使用 `middleware_fn`：

```rust
use phoenix::prelude::{middleware_fn, Next, Request};

Routes::new()
    .get("/reports", ReportController::index)
    .middleware(middleware_fn(|request: Request, next: Next| async move {
        let mut response = next.run(request).await;
        response.headers_mut().insert(
            "x-report-page",
            phoenix::http::HeaderValue::from_static("true"),
        );
        response
    }))
```

建议为所有公开路由启用 `SecurityHeaders`。它提供基础安全响应头，但不会代替业务所需的身份认证和授权。

### 在中间件和控制器之间传递类型化数据

中间件可以把已经认证的用户或权限上下文放入请求 extensions：

```rust
#[derive(Clone, Copy, Debug)]
pub struct AuthorizedAdmin;

impl Middleware for RequireExampleToken {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let authorized = request
                .headers()
                .get("x-example-token")
                .is_some_and(|value| value == "secret");

            if !authorized {
                return Response::text("Unauthorized")
                    .with_status(StatusCode::UNAUTHORIZED);
            }

            request.extensions_mut().insert(AuthorizedAdmin);
            next.run(request).await
        })
    }
}
```

控制器按类型读取，不需要使用易冲突的字符串键：

```rust
pub async fn dashboard(request: Request) -> Response {
    if request.extensions().get::<AuthorizedAdmin>().is_none() {
        return Response::text("Missing authorization context")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Response::text("admin dashboard")
}
```

不同上下文应定义不同 Rust 类型。即使两个中间件的数据字段同名，类型不同也不会互相覆盖。

### 中间件别名与模型绑定

重复使用的中间件可以注册别名。`apply` 只作用于最近声明的路由，未知别名会在构建应用前返回错误：

```rust
let mut aliases = MiddlewareAliases::new();
aliases.register("auth", RequireLogin);

let routes = aliases.apply(
    Routes::new().get("/account", AccountController::show),
    &["auth"],
)?;
```

路径模型由应用提供异步 resolver：

```rust
let binding = ModelBinding::new("article", |key| load_article(key));

let routes = Routes::new()
    .get("/articles/{article}", ArticleController::show)
    .middleware(binding);
```

resolver 返回 `Result<Option<Article>, E>`。`Ok(None)` 映射为 404，`Err(_)` 映射为不暴露内部错误的 500。控制器从 request extensions 读取已经解析的模型：

```rust
pub async fn show(request: Request) -> Response {
    let Some(article) = Bound::<Article>::from_request(&request) else {
        return Response::text("Binding unavailable")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR);
    };

    Response::text(article.title.clone())
}
```

resource 参数名与 binding 的参数名必须一致。数据库错误应在 resolver 内记录 request ID，返回给客户端的响应保持通用。

## 9. 返回响应

### 返回文本

返回值实现 `IntoResponse` 时，控制器可以直接返回它：

```rust
pub async fn index(_request: Request) -> &'static str {
    "Hello, Phoenix"
}
```

需要明确状态码时使用 `Response`：

```rust
Response::text("Unauthorized")
    .with_status(StatusCode::UNAUTHORIZED)
```

### 返回 JSON

```rust
Json(serde_json::json!({
    "id": 42,
    "name": "Ada",
}))
.into_response()
```

### 同时设置状态码和 JSON

```rust
(
    StatusCode::CREATED,
    Json(serde_json::json!({ "created": true })),
)
    .into_response()
```

### 添加响应头

静态或已经验证的响应头可以使用 `with_header`：

```rust
let response = Response::text("created")
    .with_status(StatusCode::CREATED)
    .with_header("location", "/articles/42")?;
```

动态响应头名称和值可能无效，因此 `with_header` 返回 `Result`，业务代码必须处理错误。

## 10. 返回 React 页面

后端只传页面名和业务 props，不为三种渲染模式分别设计 API。页面和共享
Props 也在 Rust 定义一次：

```rust
use phoenix::prelude::{NodeRenderer, Page, Request, Response};

#[phoenix::contract(page, page = "members/index")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersPageProps {
    pub members: Vec<MemberResource>,
    pub generated_by: String,
    pub total: u32,
}

#[phoenix::contract(shared)]
#[derive(Serialize)]
pub struct SharedProps {
    pub framework: String,
}

pub async fn show(request: Request, renderer: NodeRenderer) -> Response {
    Page::new(
        "members/index",
        MembersPageProps {
            members: load_members().await,
            generated_by: "Rust".to_owned(),
            total: 100,
        },
    )
    .shared(SharedProps { framework: "Phoenix".to_owned() })
    .respond_with_renderer(&request, &renderer)
    .await
}
```

`Page::new` 默认使用 Islands。强交互后台页面可调用 `.spa()`，需要整页 hydration 的内容页可调用 `.ssr()`：

```rust
Page::new("dashboard/show", props).spa();
Page::new("articles/show", props).ssr();
Page::new("docs/show", props); // Islands
```

三种模式共享 `PageEnvelope`。完整浏览器请求返回 HTML；带 `X-Phoenix-Page: 1` 的局部导航请求返回 `application/vnd.phoenix.page+json`。状态、页面名、props、共享数据、错误和 flash 不因模式改变。

### 编写 TSX 页面和 Island

页面放在 `views/pages`，交互组件放在 `views/islands`。业务页面只在需要浏览器交互的组件上添加 `client:load`：

```tsx
import MemberCreator from "../../islands/member-creator.js";
import type { MembersPageProps } from "../../generated/contracts.js";

export default function MembersIndex({ members, total }: MembersPageProps) {
  return (
    <main>
      <MemberTable members={members} />
      <MemberCreator client:load initialTotal={total} />
    </main>
  );
}
```

Vite 配置只启用 Phoenix 插件：

```ts
import { defineConfig } from "vite";
import { phoenix } from "@phoenix/vite";

export default defineConfig({ plugins: [phoenix()] });
```

`phoenix-vite` 自动生成页面/island 注册表、浏览器动态入口和服务端 renderer 入口。SSR renderer 从实际渲染树收集组件名、props 和多实例 ID，控制器不再重复声明 island。启动器按 `render_mode` 执行：SPA 使用 `createRoot`，SSR 使用整页 `hydrateRoot`，Islands 只对 `PageEnvelope.islands` 中的节点调用 `hydrateRoot`。

### 从 React 调用 Rust action

先定义 Rust 输入和输出契约。数据库模型不要直接导出，返回浏览器的数据使用
Resource 白名单：

```rust
#[phoenix::contract(input)]
#[derive(Deserialize)]
pub struct StoreMemberInput {
    pub name: String,
}

#[phoenix::contract(resource, name = "Member")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberResource {
    pub id: u32,
    pub name: String,
    pub joined_on: String,
}
```

路由使用稳定名称，并用 `.action::<Input, Output>()` 绑定浏览器 action 契约：

```rust
Routes::new()
    .post("/api/members", typed(MemberController::store))
    .name("members.store")
    .action::<StoreMemberInput, MemberResource>()
```

React 直接调用生成的函数，不传 URL、路由字符串或泛型：

```tsx
import { members } from "../generated/routes.js";

const member = await members.store({ name });
```

Vite 启动或构建时扫描 Rust 路由和带 `#[phoenix::contract(...)]` 的 DTO，生成只读的 `views/generated/routes.ts` 与 `contracts.ts`。点分名称会变成 TypeScript 树，action 叶子是带完整输入输出类型的函数：

```ts
export const routes = {
  members: {
    index: "members.index",
    store: createRustAction<StoreMemberInput, Member>("members.store"),
  },
} as const;

export const members = routes.members;
```

业务代码输入 `members.` 时会看到 `index` 和 `store`。Rust 删除或重命名 `.name("members.store")` 后，`members.store` 会在 TypeScript 检查中报错；生成文件由框架维护，不手写、不提交。`RouteGroup::name("admin.")` 会和组内 `.name("dashboard")` 自动合并为 `admin.dashboard`。命名路由必须使用字符串字面量，动态名称会使生成阶段失败，防止前端提示不完整。

框架同时把 Rust 运行时命名路由表加入 `PageEnvelope.routes`。`members.store()` 在调用时解析当前后端路径并发送 JSON POST。因此修改 `/api/members` 不会改前端代码，也不会在 TypeScript 中复制 URL。

生成器遵守常用 Serde wire 规则，包括方向性的 `rename`、`rename_all`、`default`、`flatten`、`alias`、skip、`skip_serializing_if`、`Option`、集合和 unit enum。输入 alias 作为后端兼容名称参与冲突检查，React 新请求统一使用规范字段名。最终 wire name、flatten 或 alias 冲突会使构建失败；可能超过 JavaScript 安全整数范围的 Rust 整数不会静默转换成 `number`。当前数据 enum、tuple struct、泛型 struct，以及会改变 wire 形态但尚不能准确表达的 Serde 属性会明确拒绝，不能生成一个看似可用但错误的 TypeScript 类型。

`members.store()` 仍然通过 HTTP 调用 Rust，并不是在浏览器进程里执行 Rust。校验、权限、ID 分配和返回数据始终由 Rust 控制器负责。

### 服务端 React HTML

`@phoenix/react-ssr` 在 SPA 模式返回空 shell，在 SSR 和 Islands 模式使用 React `renderToPipeableStream`。`respond_with_renderer` 返回完整缓冲响应；`respond_streaming_with_renderer` 会把 HTML chunk 直接交给 Hyper，并在完成帧后附加 hydration 信封。renderer 超时或退出时返回 503，不会静默切换渲染模式。业务代码不接触 `trusted_server_html`。

生产环境必须先执行 client build，再执行 SSR build；Rust 启动时加载两个 manifest、校验 contract hash、预热 worker，并在退出时调用 `renderer.shutdown().await`。完整配置、健康指标和静态资源解析见 [React 渲染模式](RENDERING.md#7-构建产物)。

### 可选加密页面协议

普通业务应使用 HTTPS 和默认明文页面 JSON。只有可信中间链路还需要额外密文层时，才显式注入 codec：

```rust
use phoenix::prelude::Aes256GcmCodec;

let codec = Aes256GcmCodec::new(key_id, key_from_secret_store);
let response = page.respond_to(&request, Some(&codec))?;
```

内置 codec 使用 AES-256-GCM，默认 60 秒有效，信封包含版本、算法、`key_id`、用途、签发/过期时间、随机 nonce、密文和 tag。密钥必须来自环境或密钥服务，不能写进仓库。浏览器端只有在调用方已经安全获得 `CryptoKey` 时，才使用 `createAes256GcmDecryptor`。

加密只作用于带 `X-Phoenix-Page: 1` 的协议响应。初始 HTML 必须包含浏览器可读的 hydration 数据；因此这项能力不能对最终用户隐藏 props，也不能替代 TLS、权限检查或字段白名单。

## 11. 编写业务测试

业务测试可以直接调用应用，不需要占用真实端口。

### 功能测试

下面的测试模拟带 JSON body 的注册请求：

```rust
use phoenix::{
    http::{Bytes, HeaderMap, HeaderValue, header},
    prelude::{Method, Request, StatusCode, Uri},
};

#[tokio::test]
async fn registration_accepts_valid_data() {
    let application = phoenix_blog_example::application()
        .expect("routes should build");

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );

    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        headers,
        Bytes::from_static(
            br#"{"user":"phoenix-user","password":"correct-horse"}"#,
        ),
    );

    let response = application.handle(request).await;

    assert_eq!(response.status(), StatusCode::CREATED);
}
```

读取并断言 JSON 响应：

```rust
let json: serde_json::Value = serde_json::from_slice(response.body())
    .expect("response should be JSON");

assert_eq!(json["errors"]["user"][0]["rule"], "not_reserved");
```

### 验证规则单元测试

```rust
use serde_json::json;

#[test]
fn reserved_user_is_rejected() {
    let payload = json!({
        "user": "admin",
        "password": "correct-horse"
    });

    let errors = registration_validator(&payload)
        .validate()
        .expect_err("reserved user should fail");

    assert_eq!(errors.get("user").unwrap()[0].rule, "not_reserved");
}
```

运行示例应用的全部业务测试：

```bash
cargo test -p phoenix-blog-example
```

## 12. 完整的强类型 action

一个完整 action 的控制器只接收验证后的 DTO，并返回显式 Resource：

```rust
pub async fn store(
    Validated(Json(input)): Validated<Json<StoreMemberInput>>,
) -> (StatusCode, Json<MemberResource>) {
    let member = create_member(input).await;
    (StatusCode::CREATED, Json(member))
}
```

对应路由绑定输入和输出契约：

```rust
Routes::new()
    .post("/api/members", typed(MemberController::store))
    .name("members.store")
    .action::<StoreMemberInput, MemberResource>()
```

请求示例：

```bash
curl -i http://127.0.0.1:3000/api/members \
  -H 'content-type: application/json' \
  --data '{"name":"Ada"}'
```

## 13. 数据库与迁移

数据库模型、CRUD、关系、游标分页、事务和迁移使用 `phoenix::database`：

```rust
use phoenix::database::{Database, Model, models};

let mut database = Database::builder(models!(Article))
    .max_connections(10)
    .connect(&std::env::var("DATABASE_URL")?)
    .await?;

let articles = Article::all()
    .order_by(Article::fields().id().asc())
    .paginate(20)
    .exec(database.toasty_mut())
    .await?;
```

迁移 runner 提供 `status()`、`plan()`、`up()` 和 `down(steps)`，自动维护状态表并验证 checksum。测试中每个 case 使用独立 `TestDatabase`，不要共享全局 SQLite 文件。完整示例与 SQLite/PostgreSQL 差异见 [数据库与迁移](DATABASE.md)。

## 14. Web 安全与生产边界

公开 Web 应用应按顺序启用可信代理、request ID、访问日志、Host allowlist、CORS、限流、Session、CSRF 和安全策略。具体配置、Cookie 开发/生产差异与控制器用法见 [安全与数据传输](SECURITY.md#32-在应用中启用安全栈)。

Phoenix 已提供这些基础中间件，但业务应用仍需自行完成：

- TLS/HTTPS 终止和可信 scheme 判断；
- 身份认证与授权策略；
- 多实例共享 Session store 和限流后端；
- CSP nonce、上传存储策略和依赖安全 CI；
- 正式上线前的独立安全评审。

不要因为启用了默认中间件就跳过权限检查、Resource 字段白名单或部署层安全配置。
