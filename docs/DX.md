# 开发者体验草案

本文同时记录已实现 API 与后续目标。标注“当前已实现”的代码由 `examples/blog` 测试验证；React、Toasty、derive 和迁移示例仍是设计草案。

## 1. 目录约定

```text
app/
  controllers/
  middleware/
  models/
  requests/
  resources/
config/
database/
  migrations/
routes/
  api.rs
  web.rs
views/
  components/
  generated/       # 自动生成，不手写、不提交
  islands/
  layouts/
  pages/
public/
storage/
tests/
  feature/
  unit/
```

## 2. 路由

当前已实现：

```rust
use phoenix::prelude::*;

pub fn routes() -> Routes {
    Routes::new()
        .get("/", HomeController::index)
            .name("home")
        .get("/posts", PostController::index)
            .name("posts.index")
        .get("/posts/{post}", PostController::show)
            .name("posts.show")
        .post("/posts", PostController::store)
            .name("posts.store")
            .middleware(Auth::required())
        .group(
            RouteGroup::new()
                .prefix("/admin")
                .name("admin.")
                .middleware(Auth::required()),
            |routes| routes
                .get("/dashboard", AdminController::dashboard)
                .name("dashboard"),
        )
}
```

设计要求：

- 路由 API 必须能在 IDE 中补全，不依赖解析字符串形式的控制器名称。
- 当前路由参数使用 Laravel 风格 `{post}`，并支持 GET、POST、PUT、PATCH、DELETE、HEAD 和 OPTIONS。
- 当前命名 URL 使用 `router.url("posts.show", &[("post", "42")])`；未知名称和缺失参数返回明确错误。
- 重复路由名或冲突模式在 `Routes::build()` 阶段失败，不静默覆盖。
- 模型绑定、正则参数约束和类型安全命名参数仍待实现。

## 3. 控制器与 React 页面

当前控制器使用 `async fn(Request)`，静态关联函数可以直接注册为处理器：

```rust
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

下面是接入 Toasty 和 React 后的目标写法：

```rust
use phoenix::prelude::*;

pub struct PostController;

impl PostController {
    pub async fn index(query: ListPosts, db: Database) -> Result<Response> {
        let posts = Post::query(&db)
            .latest()
            .paginate(query.page, 20)
            .await?;

        render("posts/index", props! {
            "posts" => PostResource::collection(posts),
            "filters" => query,
        })
    }
}
```

`props!` 仅用于快速原型；每个值仍必须实现 `Serialize`。正式页面优先使用实现 `Contract` 的强类型 props struct，构建流程自动生成 TypeScript 类型：

```rust
#[derive(Serialize, Contract)]
#[contract(namespace = "pages.posts", name = "PostIndexProps", direction = "output")]
pub struct PostIndexProps {
    posts: Paginated<PostResource>,
    filters: ListPosts,
}

render_typed("posts/index", PostIndexProps { posts, filters })
```

## 4. React 页面

```tsx
import { Head, Link, usePage } from "@phoenix/react";
import type { PostIndexProps } from "#phoenix/contracts/pages/posts";

export default function PostIndex({ posts }: PostIndexProps) {
  const { flash } = usePage();

  return (
    <main>
      <Head title="Posts" />
      {flash.success && <p role="status">{flash.success}</p>}
      {posts.data.map((post) => (
        <Link key={post.id} href={`/posts/${post.id}`}>
          {post.title}
        </Link>
      ))}
    </main>
  );
}
```

前端没有重复声明 `PostIndexProps`。P0 客户端包至少提供启动器、`Link`、表单提交、页面上下文、`Head`、加载状态、错误处理和资源/契约版本刷新。前端包不负责 UI 组件库。

## 5. 请求与验证

当前已实现无需 derive 的验证器、内置规则、`Rule` trait 和闭包式自定义规则：

```rust
let confirmed = custom_rule("confirmed", |context| {
    if context.value == context.data.get("password_confirmation") {
        Ok(())
    } else {
        Err(format!("The {} confirmation does not match.", context.field))
    }
});

Validator::new(&payload)
    .rule("user", required())
    .rule("user", string())
    .rule("password", min_length(8))
    .rule("password", confirmed)
    .validate()?;
```

下面的 Request derive、契约生成和授权接口仍是后续目标：

```rust
#[derive(FromRequest, Validate, Contract)]
#[contract(namespace = "posts", name = "StorePostInput", direction = "input")]
pub struct StorePostRequest {
    #[validate(length(min = 3, max = 120))]
    pub title: String,

    #[validate(length(min = 1))]
    pub body: String,
}

impl Authorize for StorePostRequest {
    async fn authorize(&self, user: &CurrentUser) -> bool {
        user.can("posts.create")
    }
}
```

设计要求：

- 解析、授权与验证的执行顺序固定并有文档。
- 自定义验证规则可以异步访问数据库，但同步规则不应被迫异步。
- 旧输入默认排除密码、token、文件和显式敏感字段。
- 验证消息允许覆盖字段名和本地化文本，规则标识保持稳定。

### 登录字段只定义一次

```rust
#[derive(FromRequest, Validate, Contract)]
#[contract(namespace = "auth", name = "LoginInput", direction = "input")]
pub struct LoginRequest {
    #[validate(length(min = 3, max = 120))]
    pub user: String,

    #[sensitive]
    #[validate(length(min = 8, max = 128))]
    pub password: Secret<String>,
}
```

React 直接使用生成的类型和运行时契约：

```tsx
import {
  LoginInputContract,
} from "#phoenix/contracts/auth";
import { useForm } from "@phoenix/react";

export default function Login() {
  const form = useForm(LoginInputContract);

  return (
    <form onSubmit={form.submit("auth.login")}>
      <input {...form.field("user")} autoComplete="username" />
      <input
        {...form.field("password")}
        type="password"
        autoComplete="current-password"
      />
      <button type="submit">Login</button>
    </form>
  );
}
```

`useForm()` 会从契约推导类型，`form.field("usr")` 会在 TypeScript 检查时报错。`password` 字段名可以正常生成，但敏感标记会阻止用户输入值进入旧输入、日志和输出 Props。完整冲突与类型映射规则见 [CONTRACTS.md](CONTRACTS.md)。

## 6. React 渲染模式

应用设置默认模式，路由可以覆盖：

```rust
Routes::new()
    .get("/dashboard", DashboardController::show)
        .render_mode(RenderMode::Spa)
    .get("/articles/{article}", ArticleController::show)
        .render_mode(RenderMode::Ssr)
    .get("/docs/{page}", DocsController::show)
        .render_mode(RenderMode::Islands)
```

- SPA 渲染完整客户端应用，适合后台和复杂交互。
- SSR 先在持久 JS renderer 中生成完整 HTML，再 hydrate 整个页面。
- Islands 生成完整 HTML，只为标记的交互组件加载浏览器代码。

三种模式共用控制器、Props 和页面协议。SSR/Islands 默认需要生产环境运行 renderer，不能被描述为纯单 Rust 二进制部署。完整规则见 [RENDERING.md](RENDERING.md)。

Islands 页面仍是普通 TSX：

```tsx
import { island } from "@phoenix/react/islands";
import { SearchBox } from "../../components/search-box";
import type { DocsPageProps } from "#phoenix/contracts/pages/docs";

const SearchIsland = island(SearchBox);

export default function DocsPage({ article }: DocsPageProps) {
  return (
    <main>
      <article>{article.body}</article>
      <SearchIsland source="docs" />
    </main>
  );
}
```

## 7. 模型与查询

下面表达的是目标体验，不保证与 Toasty 当前 derive/API 完全一致：

```rust
#[derive(Model)]
pub struct Post {
    #[key]
    pub id: Id<Post>,
    pub user_id: Id<User>,
    pub title: String,
    pub body: String,
    pub created_at: DateTime,
}

let post = Post::query(&db)
    .where_user_id(user.id)
    .where_title_contains(search)
    .latest()
    .first()
    .await?;
```

框架必须优先保留 Toasty 的编译期字段与关系检查。如果 Laravel 风格名称与 Toasty 的可实现 API 冲突，应选择类型安全，并通过短方法、prelude 和文档恢复易用性。

## 8. 迁移

```rust
pub struct CreatePosts;

impl Migration for CreatePosts {
    const ID: &'static str = "20260722_000001_create_posts";

    async fn up(schema: &mut Schema) -> Result<()> {
        schema.create_table("posts", |table| {
            table.id();
            table.foreign_id("user_id").references("users", "id");
            table.string("title", 120);
            table.text("body");
            table.timestamps();
        }).await
    }

    async fn down(schema: &mut Schema) -> Result<()> {
        schema.drop_table("posts").await
    }
}
```

API 需要在 Toasty migration spike 后调整。迁移执行可以先由项目内 `migrate` 二进制或测试入口调用；“生成迁移文件”的 CLI 明确延后。

## 9. 响应与错误

```rust
render("posts/show", props)
json(value)
redirect_to("posts.show", route_params! { "post" => post.id })
back().with_errors(errors).with_input(input)
download(path)
abort(StatusCode::NOT_FOUND)
```

应用错误映射为稳定的 HTTP 语义。开发环境可显示带请求 ID 的诊断页；生产环境只显示安全错误页，完整错误进入结构化日志。

## 10. 测试体验

当前案例测试位于 `examples/blog/tests/`，可以直接运行：

```bash
cargo test -p phoenix-blog-example
```

测试覆盖真实 TCP 启动和关闭、控制器、HTTP 方法、路径参数、404/405、全局/组中间件、命名 URL、重复路由名以及 trait/闭包自定义规则。

下面的 `TestApp` 断言 DSL 是后续目标：

```rust
#[phoenix::test]
async fn guest_can_view_posts(app: TestApp) {
    let post = PostFactory::new().create(&app.db).await;

    app.get(route("posts.show", post.id))
        .send()
        .await
        .assert_ok()
        .assert_page("posts/show")
        .assert_prop("post.id", post.id);
}
```

测试 API 必须能区分完整 HTML 响应、页面协议响应和 JSON API 响应，并默认隔离数据库状态。
