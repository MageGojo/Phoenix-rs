# 开发者体验草案

本文定义期望中的应用写法，用于指导 API 设计与技术验证。所有代码均为草案，尚不可运行。

## 1. 目录约定

```text
app/
  controllers/
  middleware/
  models/
  requests/
config/
database/
  migrations/
routes/
  api.rs
  web.rs
views/
  components/
  layouts/
  pages/
public/
storage/
tests/
  feature/
  unit/
```

## 2. 路由

```rust
use phoenix::prelude::*;

pub fn routes() -> Routes {
    Routes::new()
        .get("/", HomeController::index)
        .get("/posts", PostController::index)
            .name("posts.index")
        .get("/posts/:post", PostController::show)
            .name("posts.show")
        .post("/posts", PostController::store)
            .middleware(Auth::required())
            .name("posts.store")
}
```

设计要求：

- 路由 API 必须能在 IDE 中补全，不依赖解析字符串形式的控制器名称。
- 模型绑定失败默认返回 404；请求解析失败返回 400；验证失败返回 422 或 Web 回跳响应。
- 命名路由支持类型安全参数的方向应先做可行性验证，P0 可接受运行时校验参数名。

## 3. 控制器与 React 页面

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

`props!` 仅用于易读的命名映射；每个值仍必须实现 `Serialize`。稳定版应支持使用一个强类型 props struct，以便后续生成 TypeScript 类型：

```rust
#[derive(Serialize)]
pub struct PostIndexProps {
    posts: Paginated<PostResource>,
    filters: ListPosts,
}

render_typed("posts/index", PostIndexProps { posts, filters })
```

## 4. React 页面

```tsx
import { Head, Link, usePage } from "@phoenix/react";

type Props = {
  posts: Paginated<Post>;
  filters: { search?: string };
};

export default function PostIndex({ posts }: Props) {
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

P0 客户端包至少提供启动器、`Link`、表单提交、页面上下文、`Head`、加载状态、错误处理和资源版本刷新。前端包不负责 UI 组件库。

## 5. 请求与验证

```rust
#[derive(FromRequest, Validate)]
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

## 6. 模型与查询

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

## 7. 迁移

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

## 8. 响应与错误

```rust
render("posts/show", props)
json(value)
redirect_to("posts.show", route_params! { "post" => post.id })
back().with_errors(errors).with_input(input)
download(path)
abort(StatusCode::NOT_FOUND)
```

应用错误映射为稳定的 HTTP 语义。开发环境可显示带请求 ID 的诊断页；生产环境只显示安全错误页，完整错误进入结构化日志。

## 9. 测试体验

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
