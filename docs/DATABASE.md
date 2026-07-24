# 数据库与迁移

本文面向应用开发者，说明如何使用 Phoenix-rs 的 Toasty 数据库门面、SQLite / PostgreSQL / MySQL、关系、游标分页、事务、迁移和测试隔离。模型与查询保持 Toasty 原生类型；Phoenix 负责连接配置、后端识别、迁移可靠性和测试数据库生命周期。

## 1. 定义模型与关系

从 `phoenix::prelude` 或 `phoenix::database` 导入数据库类型。关系字段使用 Toasty 的 `Deferred<T>`：

```rust
use phoenix::database::{Deferred, Model};

#[derive(Debug, Model)]
pub struct Author {
    #[key]
    #[auto]
    pub id: u64,
    pub name: String,
    #[has_many]
    pub posts: Deferred<Vec<Post>>,
}

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
```

`models!(Author)` 会形成供连接和 schema 初始化使用的 `ModelSet`；相关模型会通过关系被包含。数据库模型不要直接作为浏览器响应，公开字段应转换成 Resource。

## 2. 连接 SQLite、PostgreSQL 或 MySQL

开发和单元测试可以使用内存 SQLite：

```rust
use phoenix::database::{Database, models};

let mut database = Database::sqlite_memory(models!(Author)).await?;
database.initialize_schema().await?;
```

文件 SQLite、PostgreSQL 与 MySQL 使用同一 builder：

```rust
let database = Database::builder(models!(Author))
    .max_connections(10)
    .table_prefix("app_")
    .connect(&std::env::var("DATABASE_URL")?)
    .await?;
```

支持的 URL scheme 是：

```text
sqlite:...
postgres://...
postgresql://...
mysql://...
```

其他 scheme 会返回 `DatabaseError::UnsupportedBackend`；URL 指向未编译的驱动时返回 `DatabaseError::BackendNotCompiled` 并给出应启用的 Cargo feature。生产数据库 URL 必须来自环境或密钥服务，不要写进源码、日志或公开文档。应用侧需同步选择 `config/database.toml` 的 `default = "sqlite" | "pgsql" | "mysql"` 与 `Cargo.toml` 同名 feature（见 [CONFIG.md](./CONFIG.md)）。

## 按需编译数据库驱动

`phoenix-database` 的 `sqlite`、`pgsql` / `postgresql`、`mysql` features 分别转发到 Toasty 驱动。门面 `phoenixrs` 默认只启用 `sqlite`；应用脚手架关闭 Phoenix 与 Toasty 的隐式默认 features，再以应用 feature 精确选择一个驱动。因此 HTTP 主程序和 `phoenix-manage` 使用同一驱动集合，不再固定携带 SQLite、PostgreSQL、MySQL 全家桶。

```bash
# 默认 SQLite
cargo build --release

# 单次 PostgreSQL 构建
cargo build --release --no-default-features --features pgsql

# 单次 MySQL 构建
cargo build --release --no-default-features --features mysql
```

`initialize_schema()` 适用于空数据库、原型和隔离测试。已有生产数据库的版本演进应使用迁移，避免把 Toasty schema push 当作升级工具。

## 3. CRUD 与关系加载

Phoenix 重导出 Toasty 的 `create!`、`Model`、`query`、`update` 和相关执行接口：

```rust
use phoenix::database::create;

let mut author = create!(Author {
    name: "Ada",
    posts: [
        { title: "First" },
        { title: "Second" },
    ],
})
.exec(database.toasty_mut())
.await?;

let loaded = Author::filter_by_id(author.id)
    .get(database.toasty_mut())
    .await?;

let posts = loaded.posts().exec(database.toasty_mut()).await?;
let owner = posts[0].author().exec(database.toasty_mut()).await?;

author
    .update()
    .name("Grace")
    .exec(database.toasty_mut())
    .await?;

author.delete().exec(database.toasty_mut()).await?;
```

`Database` 可以直接解引用为 Toasty `Db`，但业务代码优先显式使用 `toasty()`/`toasty_mut()`，让数据库边界在函数签名和审查中更清楚。

## 4. 游标分页

当前分页使用 Toasty 游标页，不使用容易受并发插入影响的页码 offset：

```rust
let first = Author::all()
    .order_by(Author::fields().id().asc())
    .paginate(20)
    .exec(database.toasty_mut())
    .await?;

if first.has_next() {
    let second = first.next(database.toasty_mut()).await?;
    if let Some(second) = second {
        // 将 second 转成页面 Resource。
    }
}
```

分页前必须使用稳定且确定的排序字段。对外 API 应把 Toasty 的游标包装进应用自己的响应契约，不要暴露数据库内部结构。

## 5. 事务

事务必须显式 commit；错误路径应 rollback：

```rust
let mut transaction = database.toasty_mut().transaction().await?;

let result = Author::create()
    .name("Ada")
    .exec(&mut transaction)
    .await;

match result {
    Ok(_) => transaction.commit().await?,
    Err(error) => {
        transaction.rollback().await?;
        return Err(error.into());
    }
}
```

不要把外部 HTTP 调用或长时间 CPU 工作放在数据库事务中。事务闭包应只覆盖必须保持原子的数据库操作。

## 6. 定义迁移

迁移 ID 必须非空、唯一并严格递增。每条 SQL 使用目标数据库支持的语法：

```rust
use phoenix::database::Migration;

fn migrations() -> Vec<Migration> {
    vec![
        Migration::new("202607220001", "create posts")
            .up(
                "CREATE TABLE posts (\
                 id INTEGER PRIMARY KEY, \
                 title TEXT NOT NULL)",
            )
            .down("DROP TABLE posts"),
        Migration::new("202607220002", "create post title index")
            .up("CREATE INDEX posts_title_idx ON posts (title)")
            .down("DROP INDEX posts_title_idx"),
    ]
}
```

确实无法安全回滚的迁移要显式 `.irreversible()`。这会让 `down()` 失败关闭，而不是假装回滚成功。

迁移 checksum 覆盖 ID、名称和全部 up/down SQL。已经应用的迁移不可原地修改；需要修正时增加一个新迁移。否则 `plan()`/`up()` 会返回 `ChecksumMismatch`。

## 7. 执行、查看与回滚迁移

标准项目直接使用应用自己的模型和迁移注册表执行命令：

```bash
px status
px migrate
px rollback --step 1
px fresh
px fresh --seed
px seed
```

`px` 会定位项目根并调用生成项目内的 `phoenix-manage` 二进制，因此执行的是应用编译后的 `models::all()`、`migrations::all()` 和 `database/seeders/mod.rs`，不解析或猜测 Rust 源码。`fresh` 会按迁移状态回滚全部已登记迁移后重新升级；不可逆迁移、未知状态行或 checksum 漂移都会中止，不会静默删除未知业务表。`--seed` 在迁移完成后执行应用 seeder。

底层库 API 同样可以直接调用：

```rust
use phoenix::database::MigrationRunner;

let mut runner = MigrationRunner::new(&mut database, migrations())?;

let plan = runner.plan().await?;
for id in &plan.pending {
    println!("pending: {id}");
}

let applied = runner.up().await?;
println!("applied {applied} migration(s)");

for status in runner.status().await? {
    println!("{} batch={} checksum={}", status.id, status.batch, status.checksum);
}

let rolled_back = runner.down(1).await?;
println!("rolled back {rolled_back} migration(s)");
```

第一次调用 `status()`、`plan()`、`up()` 或 `down()` 会在空数据库中创建 `phoenix_migrations` 状态表。

并发与原子性规则：

- SQLite 使用 `BEGIN IMMEDIATE` 锁住并原子执行整批迁移；同批任一 SQL 失败会回滚该批全部 DDL 和状态记录。
- PostgreSQL 使用 session advisory lock 防止多个实例同时迁移；每个迁移在独立事务中提交。
- MySQL 使用 `GET_LOCK('phoenix_migrations', 0)` / `RELEASE_LOCK`；每个迁移在独立事务中提交。
- `up()` 只执行 pending 项，重复执行返回 `0`。
- `down(n)` 从最近 batch/ID 开始回滚最多 `n` 个迁移。

应用部署应在新版本接流量前执行迁移（`px release:install` 默认会在切换 `current` 前跑 `phoenix-manage migrate`），并确保同一数据库只有迁移 runner 负责 schema 变更。

CI 契约测分别通过 `PHOENIX_TEST_POSTGRES_URL` 与 `PHOENIX_TEST_MYSQL_URL` 连接一次性服务，并为每个 job 只启用对应数据库 feature。

## 8. 测试隔离

每个测试创建自己的 `TestDatabase`：

```rust
use phoenix::database::{TestDatabase, models};

#[tokio::test]
async fn creates_an_author() {
    let mut database = TestDatabase::new(models!(Author)).await?;

    Author::create()
        .name("Test Author")
        .exec(database.toasty_mut())
        .await?;

    assert_eq!(
        Author::all()
            .count()
            .exec(database.toasty_mut())
            .await?,
        1,
    );

    Ok::<_, Box<dyn std::error::Error>>(())
}
```

`TestDatabase` 使用独占的内存 SQLite，创建时初始化 schema，drop 后全部数据消失。不要在并行测试之间共享全局数据库或依赖测试执行顺序。

PostgreSQL 契约测试需要隔离测试库：

```bash
PHOENIX_TEST_POSTGRES_URL='postgresql://…/phoenix_test' \
  cargo test -p phoenix-database --no-default-features --features postgresql \
  postgresql_crud_relations_and_pagination_when_configured
```

该环境变量未设置时 PostgreSQL 测试会跳过；CI 要验证 PostgreSQL 时必须显式提供一次性测试数据库。

## 9. 常用验证命令

```bash
cargo test -p phoenix-database
cargo clippy -p phoenix-database --all-targets -- -D warnings
```

实现依据和可运行案例位于 `crates/phoenix-database/tests/`。
