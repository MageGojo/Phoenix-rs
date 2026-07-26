# 应用配置

Phoenix 的配置分两层，职责清晰：

- **`.env` = 运行时**：前后端启动 + 数据库连接的一切（地址、URL、`DATABASE_URL`、日志、限流、信任代理）以及所有**密钥**。复制 `.env.example` 即可本地起服务。
- **`config/` = Feature 配置（TOML）**：只存放官方 / 第三方 Feature 的结构化参数（如 `config/pay.toml`）。非敏感参数进 TOML，敏感值通过 `${VAR}` 占位符从 `.env` 注入。

脚手架不再生成 `config/app.toml` / `config/database.toml`：应用名、监听地址、数据库连接统一由 `.env` 提供（历史项目里这两个 TOML 仍被 `phoenix-config` 兼容读取，优先级低于 `.env`）。

## 文件

```text
.env.example        # 复制为 .env：应用 / 前端 / 数据库 / 日志 / 限流 全在这里
.env                # 本地运行时配置与密钥（勿提交）
config/
  mod.rs            # AppConfig::load() 入口（re-export load_feature_config）
  <feature>.toml    # 按 `px new --feature` 选择生成，如 captcha.toml / pay.toml / notify.toml
```

## 优先级（低 → 高）

1. `.env`
2. 进程环境变量
3. `AppConfigBuilder::override_value`（测试 / 显式启动）

（兼容层：存在旧式 `config/app.toml` / `config/database.toml` 时，其默认值排在 `.env` 之前。）

## 运行时键（`.env`）

| 分组 | 键 |
| --- | --- |
| 应用 | `APP_ENV` / `APP_ADDR` / `APP_URL` /（可选 `APP_NAME`） |
| 前端 | `VITE_DEV_URL`（`px dev` 的 Vite 开发服务器地址） |
| 数据库 | `DATABASE_URL` |
| 日志 | `PHOENIX_LOG` |
| 限流 | `RATE_LIMIT_REQUESTS` / `RATE_LIMIT_WINDOW_SECONDS` |
| 代理与 Host | `TRUSTED_PROXIES` / `ALLOWED_HOSTS` |

## 选择数据库

运行时连接只看 `DATABASE_URL`：

```env
DATABASE_URL=sqlite:storage/app.sqlite
# DATABASE_URL=postgresql://phoenix:secret@127.0.0.1:5432/phoenix
# DATABASE_URL=mysql://phoenix:secret@127.0.0.1:3306/phoenix
```

编译进二进制的驱动由应用 `Cargo.toml` 的 feature 决定，两处保持一致。脚手架默认 `default = []`（不链接任何驱动）；`px new --database sqlite|pgsql|mysql|all` 会同时写好 feature 与 `.env.example` 的 `DATABASE_URL`：

```toml
[features]
default = ["sqlite"]
database = ["phoenix/database"]
sqlite = ["database", "phoenix/sqlite"]
# 其它可选能力见 docs/FEATURES.md：tls / websocket / sse / auth / jwt / password / metrics …
```

切换到 PostgreSQL：启用 `pgsql` feature，并把 `.env` 的 `DATABASE_URL` 换成 `postgresql://...`。未编译对应驱动时连接会失败关闭。

| 驱动 | URL 形态 |
| --- | --- |
| SQLite | `sqlite:storage/app.sqlite` 或 `sqlite::memory:` |
| PostgreSQL | `postgresql://user:pass@host:5432/db` |
| MySQL | `mysql://user:pass@host:3306/db` |

## Feature 配置（`config/<feature>.toml`）

`px new --feature captcha,pay,notify` 会生成对应 TOML；应用代码用通用入口读取：

```rust
// 文件缺失时返回 Default；字符串值里的 ${VAR} 从进程环境 / .env 注入。
let config: PayFileConfig = phoenix::config::load_feature_config("pay")?;
```

约定：**密钥进 `.env`，结构进 TOML**。例如 `config/pay.toml`：

```toml
[wechat_native]
app_id = "wx1234567890"
api_v3_key = "${PAY_WECHAT_API_V3_KEY}"   # 值放 .env，不进仓库
private_key_path = "storage/certs/apiclient_key.pem"
notify_url = "https://shop.example.com/pay/notify/wechat"
```

`load_feature_config` 语义：

- 读 `config/<name>.toml`（相对应用根）；文件或目录缺失 → `T::default()`，Feature 保持零配置可跑；
- 反序列化前替换 `${VAR_NAME}` 占位符（进程环境优先，`.env` 兜底；未知变量替换为空串）；
- 名称必须是普通文件名（拒绝路径穿越）。

## 代码入口

```rust
// config/mod.rs
pub fn load() -> Result<AppConfig, ConfigError> {
    AppConfig::load()
}
```

生产密钥按用途声明：

```rust
AppConfig::builder()
    .required_secret("JWT_SECRET", 32)
    .load()?;
```

## 与 Laravel 对照

| Laravel | Phoenix |
| --- | --- |
| `.env` + `env()` | `.env` + `AppConfig` 强类型访问 |
| `config/services.php`（第三方参数） | `config/<feature>.toml` + `load_feature_config` |
| `DB_CONNECTION` / `DATABASE_URL` | `DATABASE_URL` |
| 无类型、运行时数组 | 启动时校验，非法 URL / 缺生产项失败关闭 |
