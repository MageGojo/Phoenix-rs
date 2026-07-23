# Phoenix Rust API (agent cheat sheet)

Import from `phoenix::prelude::*` unless noted.

## Application

```rust
Application::new(routes())?
    .max_body_size(64 * 1024)
    .header_read_timeout(Duration::from_secs(5));

// Multi-app
Application::multi()
    .module(ApplicationModule::new("admin", admin_routes).prefix("/admin"))
```

Config (Laravel-style):

```text
config/app.toml          # name / env / addr / url
config/database.toml     # default = "sqlite" | "pgsql" | "mysql"
.env                     # DB_PASSWORD, DATABASE_URL overrides
```

See `docs/CONFIG.md`. Load with `AppConfig::load()` / `config::load()`.

Routes entry:

```rust
pub fn routes() -> Routes {
    phoenix::mount_routes!().with_middleware(/* security stack */)
}
```

## Routing

```rust
Routes::new()
    .get("/posts/{post}", PostController::show).name("posts.show")
    .post("/api/posts", typed(PostController::store))
        .name("posts.store")
        .action::<StorePostInput, PostResource>()
    .resource("posts", "/posts", Resource::new()
        .index(PostController::index)
        .store(PostController::store)
        .show(PostController::show)
        .update(PostController::update)
        .destroy(PostController::destroy));

// URL gen
router.url("posts.show", &[("post", "42")])?;
```

Macros: `routes! { ... }`, `applications! { ... }`.

Middleware: global / group / per-route; aliases via DX registry (unknown alias = build fail).

## Controllers & extractors

```rust
pub async fn store(
    State(db): State<Db>,
    Validated(input): Validated<StorePostInput>,
) -> impl IntoResponse { ... }

// Also: Request, Path<T>, Query<T>, Header<T>, Json<T>, Form<T>, Multipart<T>,
// Jwt<T>, CurrentPrincipal, Bound<Model>
```

Responses: `Json(...)`, `Page::...`, `Redirect`, `Download`, status tuples, `IntoResponse`.

Streaming body: only on routes marked `streaming(handler)` — do not use Json/Form extractors there.

## Contracts & validation

```rust
#[phoenix::contract(input)]
#[derive(Deserialize)]
pub struct StorePostInput { pub title: String }

#[phoenix::contract(resource, name = "Post")]
#[derive(Serialize)]
pub struct PostResource { pub id: u32, pub title: String }

#[phoenix::contract(page, page = "posts/index")]
#[derive(Serialize)]
pub struct PostsIndexProps { pub posts: Vec<PostResource> }

#[phoenix::contract(shared)]
#[derive(Serialize)]
pub struct SharedProps { pub app_name: String }
```

Validation: `Validated<T>` + `Validate` / `field("x", rules![required(), ...])` → 422 JSON field errors.

Vite emits `views/generated/contracts.ts` (+ `*Fields` for inputs) and `routes.ts`.

## Pages

```rust
Page::new("posts/index", props)
    .head(PageHead::new().title("Posts"))
    .csrf_token(session.csrf_token());
// Render modes: Islands (default) | Spa | Ssr — same props protocol
```

## Database (Toasty)

- Models via Toasty macros; Phoenix wraps pool/migrations/`TestDatabase`.
- Drivers: **sqlite** / **postgresql** / **mysql** (`config/database.toml` or `DATABASE_URL`).
- Migrations: ordered IDs, up/down, checksums; SQLite `BEGIN IMMEDIATE`; PG advisory lock; MySQL `GET_LOCK`.
- Prefer `px make:migration` / `make:model --migration` so registration stays correct.
- Do not expose DB models as contracts — map to Resource.

## Features (plugins)

```rust
use phoenix::plugin::{Capability, FeatureSet, Plugin};
FeatureSet::new()
    .allow([Capability::Routes, Capability::Commands])
    .plugin(MyPlugin)?
    .into_parts(); // routes / commands / migrations
```

See `docs/FEATURES.md`. Example crate: `examples/plugin-greeter`.

## Release

```bash
px release --version 0.1.0 --tarball
px release:install --tarball ./app.tar.gz --version 0.1.0
px release:rollback --steps 1
px release:status
```

See `docs/RELEASE_PIPELINE.md`. Deploy hook: `deploy/restart.sh.example`.

## Security (common)

- `SessionMiddleware` (+ distributed backend / Redis feature)
- CSRF for cookie session + React actions auto-send `X-CSRF-Token`
- `NonceSecurityPolicy` for CSP nonce (dev Vite origin vs production)
- CORS, rate limit, trusted proxies, Host allowlist, security headers
- Auth: JWT (`phoenix-crypto`) + RBAC/ABAC (`phoenix-auth`)

## Config / console

```rust
AppConfig::builder()... // env + .env + overrides; production validation
// Commands: commands! { "update" => update_cmd }
```

## Testing

- `phoenix-testing` feature: `TestApp` HTTP helpers / cookie / page asserts
- Prefer isolated `TestDatabase` (memory SQLite) per test

## Cargo features（门面可选）

| Feature | Purpose |
| --- | --- |
| `redis` | Session / rate limit / token stores |
| `storage` | LocalDisk uploads |
| `queue` | MemoryQueue + Worker |
| `mail` | Mailer + MemoryTransport |
| `testing` | TestApp |
