# 08 · 启用 SQLite

← [07 初级验收](../beginner/07-初级验收.md) · 下一章 → [09 模型、迁移与 CRUD](./09-模型迁移与CRUD.md)

## 目标

让应用具备数据库能力：`Cargo` feature、`database.toml`、`phoenix-manage`、本地能 `px migrate`。

> 若 `px new` 时已选 `--database sqlite` 且 migrate 已可用，本章做「核对清单」，缺什么补什么。

## 必做

### 1. 标记与依赖

检查 `.phoenix`：`database=sqlite`。

`Cargo.toml` 应对齐（示例）：

```toml
[features]
default = ["sqlite"]
database = ["phoenix/database"]
sqlite = ["database", "phoenix/sqlite"]

[dependencies]
toasty = { version = "0.8", default-features = false, features = ["migration", "serde", "sqlite"] }
```

### 2. 刷新脚手架核心

```bash
px update
# 若联调本仓库：px update --framework-path /path/to/Phoenix
```

确认出现或已存在：

- `src/bin/phoenix-manage.rs`
- `config/database.toml`（`default = "sqlite"`，`database = "storage/app.sqlite"`）
- `app/models/mod.rs`、`database/migrations/mod.rs`

### 3. 注入 Database

在 `src/lib.rs` 的 `application()` 中，在构建 Routes 后注入（参考框架示例 `examples/render-modes-smoke`）：

```rust
let db = database(&config).await?;
let built = routes(...).with_middleware(StateMiddleware::new(db));
Ok(Application::new(built)?)
```

确保已有 `database()` 辅助函数（`Database::builder(models::all()).connect(...)`）。

### 4. 迁移探活

```bash
px migrate
px status
```

即使还没有业务表，命令应能跑通（或显示无 pending）。

## 讲解

| 组件 | 作用 |
| --- | --- |
| Cargo feature | 编译期链接驱动；`default = []` 时必须显式启用 |
| `config/database.toml` | 连接信息；密钥走 `.env` |
| `phoenix-manage` | 迁移/回滚/seed；发版包也会带上 |
| `StateMiddleware<Database>` | 请求里取出 `State<Database>` / extensions |

**不要**在未启用 feature 时手写一半 ORM 代码。

## 验收

- [ ] `px migrate` / `px status` 无致命错误  
- [ ] `application()` 已注入 DB  
- [ ] `storage/` 下可出现 sqlite 文件（视驱动创建时机）

## 延伸阅读

- `docs/DATABASE.md`
- `docs/FEATURES.md` · 数据库 feature

## 下一章预告

`px make:model Note --all` 并改成真实读写。
