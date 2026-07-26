# 09 · 模型、迁移与 CRUD

← [08 启用 SQLite](./08-启用SQLite.md) · 下一章 → [10 契约与生成物](./10-契约与生成物.md)

## 目标

用官方生成器拉齐 Note（或 Post）切片，并改成**真实**列表 + 创建。

## 必做

### 1. 生成切片

```bash
px make:model Note --all
```

会生成模型、迁移、Request、Resource、控制器、routes、页面 Props、React 页，并刷新 contracts。

### 2. 修正迁移 SQL（SQLite）

生成器骨架可能是通用 SQL。SQLite 建议：

```sql
CREATE TABLE notes (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL
)
```

`down`：`DROP TABLE notes`。

### 3. 跑迁移

```bash
px migrate
```

### 4. 改成真实读写

对照 `examples/render-modes-smoke` 的 Note：

- `index`：`Note::all().exec(db.toasty_mut()).await`，填入页面 Props  
- `store`：`create!(Note { name: … })`，返回 `NoteResource`  
- 页面模式与 Home 一致（如 `.islands()` + `respond_with_renderer`）

生成器默认的假 id（`"generated"`）必须删掉。

### 5. 手工验收

```bash
px dev
```

- 浏览器打开 `/notes`：空列表文案  
- 带 CSRF 的 POST `/notes` `{"name":"first"}` → 201  
- 刷新列表可见新行  

## 讲解：关系只写「关联哪个模型」

模型用 `#[phoenix::model]`，重复的部分由约定补齐，你只写这个模型独有的东西：

```rust
#[model]
pub struct Post {
    pub title: String,
    #[belongs_to]                 // ← 只说「属于 User」
    pub user: Deferred<User>,
}
```

`posts` 表名、`id` 主键、`user_id` 外键、`key = user_id, references = id` 的映射，四样都不用写。命令行更直接——**选类型 + 选模型**：

```bash
px make:model Post --belongs-to=User --migration --factory
```

三种关系：`--belongs-to`（外键在自己身上）、`--has-many`、`--has-one`（外键在对方身上）。**单向关联完全可以**：`Post` 写 `#[belongs_to]`，`User` 什么都不写。

**默认全自动，每一条都能单独接管**：写了 `#[table = "..."]`、自己的 `#[key]` 字段、`#[belongs_to(key = author_id)]`、或外键字段本身，宏就不碰那一部分。展开结果就是普通 Toasty 模型，没有暗门。

一条边界：自动生成的主键与外键是 `i64`。用别的键类型时自己声明外键字段——宏看不到对方模型的键类型。

## 讲解

```text
make:model --all  → 可编译骨架
你填 SQL + 查询/写入 → 可演示业务
契约字段变更 → 重新生成 types（勿手改 generated）
```

一次只做一种资源；不要同时引入用户系统。

## 验收

- [ ] 表已迁移  
- [ ] GET 列表读库  
- [ ] POST 写库且再次 GET 可见  

## 延伸阅读

- `docs/DATABASE.md` · 迁移与部署时序  
- 示例 `examples/render-modes-smoke/app/controllers/note_controller.rs`

## 下一章预告

看清契约如何流到 TypeScript。
