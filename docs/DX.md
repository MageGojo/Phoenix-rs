# 开发者体验

本文同时记录已实现 API 与后续目标。标注“当前已实现”的代码由 crate 测试或 `examples/blog` 功能测试验证。

## 1. 目录约定

```text
app/
  commands/
  controllers/
  middleware/
  models/
  requests/
  resources/
config/
  app.toml
  database.toml
  schemas/           # JSON Schema（Taplo / Even Better TOML）
  mod.rs
database/
  migrations/
  seeders/
deploy/
  restart.sh.example # 发版切换后重启钩子
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
dist/              # px release 制品（gitignore）
taplo.toml
tests/
  feature/
  unit/
```

配置优先级与选库说明见 [CONFIG.md](CONFIG.md)。

## 2. 路由

每个标准路由文件导出 `pub fn routes() -> Routes`；应用入口使用约定扫描，无需逐个声明 module：

```rust
pub fn routes() -> Routes {
    phoenix::mount_routes!() // 按文件名排序挂载 routes/*.rs
}
```

路由文件也可以使用 resource 声明：

```rust
use phoenix::prelude::*;

pub fn routes() -> Routes {
    Routes::new().resource(
        "posts",
        "/posts",
        Resource::new()
            .index(PostController::index)
            .create(PostController::create)
            .store(PostController::store)
            .show(PostController::show)
            .edit(PostController::edit)
            .update(PostController::update)
            .destroy(PostController::destroy),
    )
}
```

它生成 Laravel 风格的 `posts.index/create/store/show/edit/update/destroy` 名称；update 同时接受 PUT/PATCH。`only([...])` 与 `except([...])` 可裁剪集合，`parameter("article")` 可覆盖默认单数参数名。

手工声明路由同样可用：

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
- 正则参数约束和类型安全命名参数仍待实现。

### 中间件与请求上下文

可复用中间件使用 `Middleware` trait；一次性中间件使用 `middleware_fn`。认证等中间件可以把强类型、请求局部状态写入 extensions，控制器无需再次解析 Header：

```rust
pub struct AuthorizedAdmin;

impl Middleware for RequireExampleToken {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let authorized = request
                .headers()
                .get("x-example-token")
                .is_some_and(|value| value == "secret");
            if authorized {
                request.extensions_mut().insert(AuthorizedAdmin);
                next.run(request).await
            } else {
                Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED)
            }
        })
    }
}
```

`examples/blog` 同时使用全局安全头、全局响应中间件、管理路由组中间件和单路由闭包中间件。

重复使用的中间件可以注册短别名；未知别名会在构建路由阶段返回错误：

```rust
let mut aliases = MiddlewareAliases::new();
aliases.register("auth", RequireLogin);

let routes = aliases.apply(
    Routes::new().get("/account", AccountController::show),
    &["auth"],
)?;
```

`ModelBinding<T>` 在业务 handler 前异步解析路径参数，并以 `Bound<T>` 写入请求 extensions。`Ok(None)` 固定为 404，resolver 错误固定为不泄露内部细节的 500：

```rust
let binding = ModelBinding::new("post", |key| load_post(key));
let post = Bound::<Post>::from_request(&request).expect("binding middleware ran");
```

其中应用提供的 `load_post` 返回 `Result<Option<Post>, E>`。把 binding 作为目标路由中间件挂载后再读取 `Bound<Post>`。

## 2.1 项目与业务代码生成

安装 CLI 后创建新项目：

```bash
# 安装 CLI（推荐：crates.io 发布后）
cargo install px-cli

# 或从本仓库 / GitHub
cargo install --path crates/phoenix-cli
# cargo install --git https://github.com/MageGojo/Phoenix-rs px-cli
# cargo install --git https://gitcode.com/Roufsi/Phoenix-rs px-cli

px new my-app
cd my-app
px dev
```

`px` 是 crates.io 包名与二进制名。`px new` 在无本地框架检出时依赖门面包 `phoenixrs`（应用仍 `use phoenix::`）。详见根 README「命名」一节。

`px new` 可交互选择 Islands（默认）/ SPA / SSR、无数据库（默认）或 SQLite/PostgreSQL/MySQL/全部驱动、Tailwind CSS、是否初始化 Git（默认否）和 TSX（默认）/ JSX；相同选项也可用 `--render-mode`、`--database`、`--tailwind`、`--git` 与 `--frontend` 传入。未选择数据库时不会写入 Toasty 依赖、数据库配置或迁移目录。应用入口是 `phoenix-console`：`cargo run -- serve` 启动 HTTP，`px make:command Update` 生成并注册自定义子命令。`px dev` 自动构建 client 与 SSR renderer，并在 Rust/React 源码变化后先重建产物再重启后端；开发模式使用 Vite dev client，前端走 HMR，后端 Rust/路由变化触发浏览器 full reload，开发与 `npm run build` 使用相同 manifest 和渲染器输出。

默认 Web 栈按请求顺序装配可信代理、request ID、访问日志、Host allowlist、限流、nonce CSP、安全 Session、CSRF 与强类型 `AppConfig` State。开发环境使用内存 Session/限流后端；多实例生产部署需要替换为共享后端。默认执行 `npm install` 和刷新 `views/generated`，Git 初始化需在交互菜单中选择或传递 `--git`；可用 `--no-install`。在框架源码之外开发时，可用 `--framework-path <path>` 显式绑定本地 Phoenix-rs。

数据库命令：

```bash
px status
px migrate
px rollback --step 2
px fresh --seed
px seed
```

发版（见 [RELEASE_PIPELINE.md](RELEASE_PIPELINE.md)）：

```bash
px release --version 0.1.0 --tarball
px release:install --tarball ./app-0.1.0.tar.gz --version 0.1.0
px release:rollback --steps 1
px release:status
```

第三方 Feature 插件见 [FEATURES.md](FEATURES.md)。

业务生成命令：

```bash
px make:controller ReportController --route
px make:controller Admin/PostController --resource
px make:model Post --migration
px make:model Post --all
px make:migration add_status_to_posts
px make:request StorePostRequest
px make:resource PostResource
px make:middleware RequireLoginMiddleware
px make:page posts/index
px make:island LikeButton
px make:command Update
```

`make:model Post --all` 生成并连接一条可编译的业务切片：Toasty 模型、迁移、验证 Request、公开 Resource、控制器、七条命名 resource 路由、类型化 `store` action、Rust Page Props 和 React index 页面。生成后会自动刷新 Rust→TypeScript contracts/routes；浏览器可以直接调用生成的 `posts.store({ name })`。

`make:command Update` 生成 `app/commands/update.rs` 中的 `async fn update`，并写入 `commands!` 托管区块。运行：`cargo run -- update`。

生成器自动维护 `mod.rs`、模型 `ModelSet`、迁移 `all()` 和 `routes/*.rs`。嵌套名称支持 `/` 或 `::`，例如 `Admin/Post`。托管内容位于明确的 `<phoenix:...>` 标记内，标记外业务代码不改动；同名目标默认拒绝覆盖，确需重建时显式使用 `--force`。迁移中的 SQL 是安全可编译的基础骨架，提交前仍应按真实 schema 调整。

## 2.2 统一开发命令

安装/构建 `px`（原 `phoenix-cli` 目录）后，在应用目录运行：

```bash
px dev
```

该命令先从当前目录向上定位项目根并检查 JavaScript 依赖，再显示即将启动的后端、前端和工作目录，然后同时运行：

- **前端**：`npm run dev -- --strictPort`（Vite HMR）
- **后端**：`cargo run -- serve`，并监听 `app/`、`src/`、`routes/`、`config/`、`database/`、`Cargo.toml`；源码变更后自动杀掉旧进程并重新 `cargo run`（编译失败不会拖垮 Vite，会等下次改动再试）

两者位于独立进程组；Ctrl-C 或 Vite 提前退出会终止并回收另一侧。Rust 在热重载模式下因编译失败退出时，监督器会保持 Vite 并等待下一次源码变更。Vite 使用 strict port，确保 Rust 输出的默认 `VITE_DEV_URL` 不会因自动换端口而指向错误服务。

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

数据库查询使用 Phoenix 的 Toasty 门面。当前可运行 API 见[数据库与迁移](DATABASE.md)，控制器只把显式 Resource 交给 React：

```rust
let posts = Post::all()
    .order_by(Post::fields().id().asc())
    .paginate(20)
    .exec(database.toasty_mut())
    .await?;

let props = PostIndexProps {
    posts: posts.into_iter().map(PostResource::from).collect(),
};

let page = Page::new("posts/index", props);
```

正式页面使用强类型 props struct，构建流程自动生成 TypeScript 类型：

```rust
#[phoenix::contract(page, page = "posts/index")]
#[derive(Serialize)]
pub struct PostIndexProps {
    pub posts: Vec<PostResource>,
}
```

## 4. React 页面

```tsx
import type { PostIndexProps } from "../generated/contracts.js";

export default function PostIndex({ posts }: PostIndexProps) {
  return (
    <main>
      {posts.map((post) => (
        <article key={post.id}>{post.title}</article>
      ))}
    </main>
  );
}
```

前端不重复声明 `PostIndexProps`。生成文件只读且不提交；Vite 启动或构建时重新生成。前端包负责页面启动、hydration、island 和 Rust action 传输，不提供 UI 组件库。

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
    .field("user", rules![required(), string(), NotReservedUser])
    .field(
        "password",
        rules![required(), string(), min_length(8), confirmed],
    )
    .validate()?;
```

JSON 控制器通过 `request.json()` 同时检查 MIME 与 payload：缺少/错误 Content-Type 返回 415，JSON 语法错误返回 400。`application/*+json` 被接受。

强类型 extractor、验证和契约可以组合，控制器只接收验证成功的 DTO：

```rust
#[phoenix::contract(input)]
#[derive(Deserialize)]
pub struct StorePostInput {
    pub title: String,
    pub body: String,
}

impl Validate for StorePostInput {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({
            "title": self.title,
            "body": self.body,
        });
        Validator::new(&data)
            .field("title", rules![required(), string(), max_length(120)])
            .field("body", rules![required(), string()])
            .validate()
    }
}

pub async fn store(
    Validated(Json(input)): Validated<Json<StorePostInput>>,
) -> (StatusCode, Json<PostResource>) {
    let post = create_post(input).await;
    (StatusCode::CREATED, Json(PostResource::from(post)))
}
```

路由用 `typed(store)` 挂载 handler，并可通过 `.action::<StorePostInput, PostResource>()` 生成 TypeScript 调用函数。授权仍由 Session/认证上下文和路由中间件负责，不要把“通过字段验证”等同于“有权限执行”。密码、token 和文件值不得进入输出 Resource、页面 Props 或日志。完整字段映射见 [CONTRACTS.md](CONTRACTS.md)。

## 6. React 渲染模式

应用设置默认模式，路由可以覆盖：

```rust
Page::new("dashboard/show", props).spa();
Page::new("articles/show", props).ssr();
Page::new("docs/show", props) // 默认 Islands
```

- SPA 渲染完整客户端应用，适合后台和复杂交互。
- SSR 先在持久 JS renderer 中生成完整 HTML，再 hydrate 整个页面。
- Islands 生成完整 HTML，只为标记的交互组件加载浏览器代码。

三种模式共用控制器、Props 和页面协议。SSR/Islands 默认需要生产环境运行 renderer，不能被描述为纯单 Rust 二进制部署。完整规则见 [RENDERING.md](RENDERING.md)。

新项目首页已注入 `NodeRenderer`，控制器统一调用 `respond_with_renderer`。所以将首页的 `.spa()` 改为 `.ssr()`、`.islands()`，或直接删掉 `.spa()`，即可切换页面模式；路由和响应调用保持不变。启用 SSR/Islands 前先构建 client 与 renderer bundle。

Islands 页面仍是普通 TSX，组件本身不需要 Phoenix 包装：

```tsx
import SearchBox from "../../islands/search-box";
import type { DocsPageProps } from "#phoenix/contracts/pages/docs";

export default function DocsPage({ article }: DocsPageProps) {
  return (
    <main>
      <article>{article.body}</article>
      <SearchBox client:load source="docs" />
    </main>
  );
}
```

`phoenix-vite` 自动发现页面和 islands、生成浏览器动态加载器与服务端 renderer 入口。开发者不维护注册表；没有 `client:load` 的组件只参与 SSR。

## 7. 模型与查询

模型保持 Toasty 的编译期字段和关系检查：

```rust
use phoenix::database::{Deferred, Model};

#[derive(Debug, Model)]
pub struct Post {
    #[key]
    #[auto]
    pub id: u64,
    #[index]
    pub author_id: u64,
    #[belongs_to]
    pub author: Deferred<Author>,
    pub title: String,
}

let post = Post::filter_by_id(id)
    .get(database.toasty_mut())
    .await?;
```

SQLite、PostgreSQL 与 MySQL 使用相同模型/CRUD/关系/游标分页接口。Phoenix-rs 不复制 Toasty 查询构建器，只补充连接配置、后端元数据、迁移和测试隔离。完整示例见 [DATABASE.md](DATABASE.md)；选库见 [CONFIG.md](CONFIG.md)。

## 8. 迁移

```rust
use phoenix::database::{Migration, MigrationRunner};

let migrations = [
    Migration::new("202607220001", "create posts")
        .up("CREATE TABLE posts (id INTEGER PRIMARY KEY, title TEXT NOT NULL)")
        .down("DROP TABLE posts"),
];

let mut runner = MigrationRunner::new(&mut database, migrations)?;
let plan = runner.plan().await?;
let applied = runner.up().await?;
let rolled_back = runner.down(1).await?;
```

runner 自动维护 `phoenix_migrations`，验证 checksum，并按后端加锁：SQLite `BEGIN IMMEDIATE`、PostgreSQL advisory lock、MySQL `GET_LOCK`。已应用迁移不能原地改写；不可逆迁移必须显式 `.irreversible()`。迁移骨架可以通过 `px make:migration` 或 `px make:model --migration` 生成并自动注册。

## 9. 响应与错误

```rust
Response::text("Not Found").with_status(StatusCode::NOT_FOUND);
Json(value).into_response();
(StatusCode::CREATED, Json(resource)).into_response();
Page::new("posts/show", props).respond_to(&request, None)?;
```

`Response::with_header` 用于经过验证的动态 Header，并返回 `Result`。handler panic 和模型绑定失败只向客户端返回通用 500；业务错误应映射为稳定状态码，把内部原因和 request ID 写入结构化日志。`Redirect` 与 `Download` 已提供安全门面；带验证错误和 flash 的 `back()` 仍待实现。

## 10. 测试体验

案例级回归仍位于 `examples/blog/tests/`：

```bash
cargo test -p phoenix-blog-example
```

应用测试客户端由 `phoenix-testing` 提供。它在 `127.0.0.1:0` 上启动真实 `Application`，自带 Cookie jar 与页面协议断言：

```bash
cargo test -p phoenix-testing --locked
```

```rust
use phoenix_testing::TestApp;
use phoenix_http::StatusCode;

let app = TestApp::spawn(routes).await;

app.get("/health")
    .send()
    .await
    .assert_ok()
    .assert_status(StatusCode::OK)
    .assert_json_path("status", "healthy");

app.post_json("/login", &body).send().await.assert_ok();

app.get("/members")
    .page_protocol()
    .send()
    .await
    .assert_page("members");

app.shutdown().await?;
```

已实现：`get` / `post_json` / `post_form` / 自定义 Header、`assert_ok` / `assert_status` / `assert_body_contains` / `assert_json` / `assert_json_path` / `assert_page`、Cookie 往返、`Drop`/`shutdown` 停服。完整 HTML、页面协议 JSON（`X-Phoenix-Page: 1`）与普通 JSON API 可按 Content-Type 与断言区分。

尚未提供：`#[phoenix::test]` 自动注入、Factory DSL、默认 `TestDatabase` 隔离（见 `docs/TESTING_AND_STORAGE.md`）。

## 11. 应用状态与安全响应

应用级配置、数据库和外部客户端通过显式状态中间件进入强类型控制器：

```rust
let routes = routes.with_middleware(StateMiddleware::new(app_state));

async fn index(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.catalog.summary().await)
}
```

`AppState` 必须可克隆且线程安全；通常内部持有连接池或 `Arc`，不要把逐请求可变数据放进全局状态。缺失状态会在控制器执行前返回通用 500，不暴露类型或内部配置。

控制器可用 `Redirect::see_other(...)` 处理 POST 后跳转，用 `Download::attachment(...)` 返回受控文件。下载响应默认 `private, no-store` 与 `nosniff`，并同时生成安全 ASCII 回退文件名和 UTF-8 `filename*`；不要绕过该 API 手写来自用户输入的 `Content-Disposition`。

## 12. 快速声明宏

`routes!` 适合重复的静态路由声明：

```rust
let routes = routes! {
    GET "/posts" => typed(PostController::index), name = "posts.index";
    POST "/posts" => typed(PostController::store),
        name = "posts.store", middleware = [RequireLogin];
    PATCH "/posts/{post}" => typed(PostController::update), name = "posts.update";
    DELETE "/posts/{post}" => typed(PostController::destroy), name = "posts.destroy";
};
```

支持 `GET`、`POST`、`PUT`、`PATCH`、`DELETE`、`HEAD` 和 `OPTIONS`。`name` 与 `middleware` 都可省略；多个中间件按数组中的声明顺序应用。

`applications!` 用于官网/前台/后台的静态组装，详见[多应用项目](MULTI_APP.md)。两个宏都只生成现有 builder 调用，不隐藏动态控制流；需要循环、条件或运行时配置时直接使用 `Routes`、`ApplicationModule` 和 `Application::multi()`。
