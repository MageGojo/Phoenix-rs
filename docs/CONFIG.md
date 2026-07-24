# 应用配置（Laravel 风格）

Phoenix 用 **TOML 配置文件**描述结构与默认值，用 **`.env`** 放密钥与机器差异。这对应 Laravel 的 `config/*.php` + `.env`。

## 文件

```text
config/
  app.toml          # 应用名、环境、监听地址、公开 URL
  database.toml     # 默认连接 + sqlite / pgsql / mysql
  schemas/          # JSON Schema（编辑器补全 / 校验）
  mod.rs            # AppConfig::load() 入口
taplo.toml          # Taplo / Even Better TOML 关联 Schema
.env.example        # 复制为 .env
.env                # 本地覆盖（勿提交）
```

## 编辑器代码提示（可实现）

TOML 本身没有类型系统，但可以通过 **JSON Schema + Taplo** 获得补全、悬停说明与校验：

1. 安装 Cursor / VS Code 扩展 **Even Better TOML**（或 CLI `taplo`）
2. 打开 `config/app.toml` / `config/database.toml`（文件顶部有 `#:schema ...`）
3. 输入 `driver`、`default` 等字段时会出现枚举与字段提示

框架仓库根目录提供：

- `schemas/phoenix-config-*.schema.json`
- `taplo.toml`
- `.vscode/settings.json`（`evenBetterToml.schema.associations`）

`px new` 会把 Schema 拷进应用的 `config/schemas/`，并生成应用级 `taplo.toml`。

## 优先级（低 → 高）

1. `config/*.toml` 默认值  
2. `.env`  
3. 进程环境变量  
4. `AppConfigBuilder::override_value`（测试 / 显式启动）

## 选择数据库

编辑 `config/database.toml`：

```toml
# 本地零依赖
default = "sqlite"

# 或 PostgreSQL
# default = "pgsql"

# 或 MySQL / MariaDB
# default = "mysql"
```

数据库配置决定运行时连接，应用 `Cargo.toml` 的 feature 决定编译进二进制的驱动。两处应保持一致。脚手架默认 `default = []`（不链接任何驱动）；启用数据库时只开一个驱动 feature：

```toml
[features]
default = []
database = ["phoenix/database", "dep:toasty"]
sqlite = ["database", "phoenix/sqlite", "toasty/sqlite"]
pgsql = ["database", "phoenix/pgsql", "toasty/postgresql"]
mysql = ["database", "phoenix/mysql", "toasty/mysql"]
# 其它可选能力见 docs/FEATURES.md：tls / websocket / sse / auth / jwt / password / metrics …
```

使用 SQLite 时：`cargo build --features sqlite`，并保持 `config/database.toml` 的 `default = "sqlite"`。切换到 PostgreSQL 时启用 `pgsql` feature，同时把配置改为 `default = "pgsql"`。这种编译期选择避免 `phoenix-manage` 静态链接未使用的另外两个数据库驱动。

如果对应驱动已经编译，也可以不改配置文件，只在 `.env` 覆盖运行时连接：

```env
DB_CONNECTION=mysql
DB_PASSWORD=secret
```

新脚手架默认不编译数据库驱动，所以使用上述 MySQL 覆盖前必须启用 `mysql` Cargo feature。

框架会按 `connections.mysql` 拼出：

```text
mysql://phoenix:secret@127.0.0.1:3306/phoenix
```

若直接提供完整 URL，**以 `DATABASE_URL` 为准**（覆盖 TOML）：

```env
DATABASE_URL=mysql://user:pass@db.example:3306/app
```

| 驱动 | `driver` | 说明 |
| --- | --- | --- |
| SQLite | `sqlite` | `database` 为相对应用根的路径，或 `:memory:` |
| PostgreSQL | `pgsql` / `postgres` / `postgresql` | `host` / `port`（默认 5432）/ `database` / `username` / `password` |
| MySQL | `mysql` | `host` / `port`（默认 3306）/ `database` / `username` / `password` |

## 应用配置

`config/app.toml`：

```toml
name = "my-app"
env = "development"
addr = "127.0.0.1:3000"
url = "http://127.0.0.1:3000"
```

映射到 `APP_NAME` / `APP_ENV` / `APP_ADDR` / `APP_URL`（均可被 `.env` 覆盖）。

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
| `config/database.php` + `DB_CONNECTION` | `config/database.toml` + `DB_CONNECTION` |
| `env('DB_PASSWORD')` | `.env` 的 `DB_PASSWORD` |
| `config/app.php` | `config/app.toml` |
| IDE 补全靠 PHP/插件 | JSON Schema + Taplo / Even Better TOML |
| 无类型、运行时数组 | 启动时校验，非法 URL / 缺生产项失败关闭 |
