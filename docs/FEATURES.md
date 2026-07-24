# Feature / 插件扩展（首版）

Phoenix-rs 支持两类「Feature」概念，勿混淆：

1. **Cargo features（编译期裁剪）**：控制是否链接 TLS / DB / JWT 等（见 [ADR-042](./DECISIONS.md)）。门面 `phoenixrs` 默认 `default = []`。
2. **Plugin / FeatureSet（本页）**：第三方 crate 显式安装路由、命令、迁移。

## Cargo features（体积相关）

| Feature | 作用 |
| --- | --- |
| `sqlite` / `pgsql` / `mysql` | 数据库（隐含 `database`） |
| `tls` | 进程内 rustls HTTPS |
| `websocket` | WebSocket 门面 |
| `sse` | Server-Sent Events（与 websocket **分开**） |
| `auth` | RBAC/ABAC（`PrincipalFromJwt` 另需 `jwt`） |
| `jwt` | JWT + refresh token 栈 |
| `password` | Argon2 密码哈希 |
| `metrics` | Prometheus 指标 |
| `redis` / `queue` / `mail` / `storage` / `testing` | 既有可选能力 |

```toml
phoenix = { package = "phoenixrs", version = "0.1", default-features = false, features = ["sqlite", "password"] }
```

---

Phoenix-rs 支持第三方以 **Cargo crate** 形式发布 Feature（插件），应用在编译期**显式安装**后使用其路由、控制台命令与迁移。

这不是运行时 `.so` 热加载，也不是 Laravel Package 自动发现——符合 ADR-008（显式组装、无反射 DI）。

## 现在能做什么

| 能力 | 状态 |
| --- | --- |
| `Plugin` trait + `FeatureSet::plugin` 显式安装 | ✅ |
| 贡献路由（可自动加路由名命名空间） | ✅ |
| 贡献 `px` / 应用 console 命令 | ✅ |
| 贡献数据库迁移 | ✅ |
| 能力声明 + 应用侧 allowlist（拒绝未知能力） | ✅ |
| 插件名 / 命令名 / 迁移 ID 冲突诊断 | ✅ |
| 示例插件 `phoenix-plugin-greeter` | ✅ |
| 前端页面 / Island 跨 crate 自动发现 | ❌ 后续（插件可另发 npm 包，应用自行挂载） |
| 运行时动态加载 | ❌ 不做 |

## 应用侧用法

```toml
# Cargo.toml
phoenix = { package = "phoenixrs", version = "0.1" }
phoenix-plugin-greeter = "0.1"   # 第三方 Feature
```

```rust
use phoenix::prelude::*;
use phoenix::plugin::{Capability, FeatureSet};
use phoenix_plugin_greeter::GreeterPlugin;

pub fn features() -> Result<FeatureSet, phoenix::plugin::FeatureError> {
    FeatureSet::new()
        // 只允许这些能力；插件声明了未允许的能力会失败关闭
        .allow([Capability::Routes, Capability::Commands])
        .plugin(GreeterPlugin::new("你好"))
}

pub fn routes(config: &AppConfig) -> Result<Routes, phoenix::plugin::FeatureError> {
    let features = features()?;
    Ok(phoenix::mount_routes!()
        .merge(features.into_routes())
        .with_middleware(/* 安全栈 */))
}

// main / Console
let features = features()?;
Console::new(env!("CARGO_PKG_NAME"))
    .serve(|_| async { /* … */ })
    .commands(
        commands::registry()
            .into_iter()
            .chain(features.into_commands()),
    )
    .run()
    .await?;

// migrate：把 features.migrations() 接到 MigrationRunner
```

## 开发一个 Feature

```rust
use phoenix::plugin::{Capability, Plugin};
use phoenix::prelude::*;

pub struct GreeterPlugin {
    greeting: String,
}

impl GreeterPlugin {
    pub fn new(greeting: impl Into<String>) -> Self {
        Self { greeting: greeting.into() }
    }
}

impl Plugin for GreeterPlugin {
    fn name(&self) -> &'static str {
        "greeter"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Routes, Capability::Commands]
    }

    fn routes(&self) -> Routes {
        let greeting = self.greeting.clone();
        Routes::new()
            .get("/hello", move |_request: Request| {
                let greeting = greeting.clone();
                async move { Json(json!({ "message": greeting })).into_response() }
            })
            .name("hello")
    }

    fn commands(&self) -> Vec<CommandEntry> {
        let greeting = self.greeting.clone();
        vec![CommandEntry::new("greet", move |_ctx| {
            let greeting = greeting.clone();
            Box::pin(async move {
                println!("{greeting}");
                Ok(())
            })
        })]
    }
}
```

安装时默认给路由名加上 `{plugin_name}.` 前缀（上例变成 `greeter.hello`），**不**强制 path 前缀——Webhook 等可继续挂在插件自己选择的路径上。

关闭命名空间：

```rust
FeatureSet::new().namespace_route_names(false).plugin(...)
```

## 分发约定

1. 插件是普通 Rust crate，依赖 `phoenix`（或 `phoenix-plugin` + 所需组件）。
2. 应用 `Cargo.toml` 声明依赖，并在代码里 `.plugin(...)`——**禁止**隐式全局注册。
3. 版本兼容：插件应声明自己的 `version()`；框架主版本不匹配时由应用锁定依赖解决（首版不做 semver 解析器）。
4. 密钥：插件若需要密钥，文档写明；应用用 `AppConfigBuilder::required_secret` 声明，不由插件偷偷读环境。

## 与 Multi-app 的关系

- **Feature**：同一应用内组合能力（路由 / 命令 / 迁移）。
- **ApplicationModule**：按 path/host 隔离整棵应用树。

大功能两者可组合：Feature 提供路由表，再用 `ApplicationModule` 挂到 `/billing`。

## API 入口

- Crate：`phoenix-plugin`（门面 `phoenix::plugin`）
- 文档：本文件；决策：`DECISIONS.md` ADR-031
