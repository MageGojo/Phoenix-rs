# 测试门面与上传存储

## `phoenix-testing`

`crates/phoenix-testing` 提供真实 TCP 启动的请求测试客户端。

### 公开 API

```rust
use phoenix_runtime::Application;
use phoenix_testing::TestApp;
use phoenix_http::StatusCode;

let app = TestApp::spawn(routes).await;
// 或: TestApp::spawn(Application::new(routes)).await;

let response = app.get("/posts/1").send().await;
response.assert_ok();
response.assert_status(StatusCode::OK);
response.assert_body_contains("...");
response.assert_json_path("id", 1);
response.assert_json(|value| assert_eq!(value["id"], 1));
response.assert_page("posts/show"); // X-Phoenix-Page JSON 或 HTML envelope

app.post_json("/login", &body).send().await;
app.post_form("/login", &form).send().await;
app.get("/members").page_protocol().send().await.assert_page("members");

app.shutdown().await?; // 也可依赖 Drop 停止服务
```

能力清单（已实现）：

- 临时端口启动真实 `Application`（`127.0.0.1:0`，复用 `ServerHandle`）。
- Cookie jar：自动保存 `Set-Cookie`，后续请求带上 `Cookie`。
- `get` / `post` / `post_json` / `post_form` / `put` / `patch` / `delete` / 自定义 Header。
- 页面协议：`.page_protocol()` 设置 `X-Phoenix-Page: 1`；`assert_page` 校验 page name。
- `Drop` / `shutdown` 时停止服务器。
- `try_spawn` 返回 `Result`；`spawn` 在启动失败时 panic（测试友好）。

不在首版：`#[phoenix::test]` 宏自动注入、Factory DSL、`TestDatabase` feature 注入。

验收：`cargo test -p phoenix-testing --locked`；覆盖 200 JSON、404、Cookie 往返、页面协议。

## `phoenix-storage`

本地文件存储抽象，供上传与下载路径使用。

```rust
use bytes::Bytes;
use phoenix_storage::{LocalDisk, Storage, StorageError, sanitize_key};

pub trait Storage: Send + Sync {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), StorageError>;
    async fn get(&self, key: &str) -> Result<Bytes, StorageError>;
    async fn delete(&self, key: &str) -> Result<(), StorageError>;
    async fn exists(&self, key: &str) -> Result<bool, StorageError>;
    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError>;
}

let storage = LocalDisk::new("./storage")?;
storage.put("avatars/user.txt", Bytes::from_static(b"hi")).await?;
storage.store_bytes("docs/a.bin", bytes).await?; // multipart helper 友好入口
```

安全规则（已实现）：

- `sanitize_key`：拒绝空、绝对路径、`..`、NUL、控制字符、Windows 盘符。
- `resolve` / `path_for`：canonicalize（或祖先检查）后路径必须仍在 `root` 下。
- 写文件用临时文件 + `rename`；目录按需创建。
- `store_bytes` 供 multipart 等已缓冲字段落盘；原始文件名只作元数据，存储 key 由应用给定。

`Download` 已存在于 `phoenix-http`；存储层只负责落盘与路径安全，不发明新的 Content-Disposition。

验收：`cargo test -p phoenix-storage --locked`；覆盖正常读写删、路径穿越拒绝、symlink escape（Unix）。

## 顶层集成说明

门面已提供可选 feature：

| Feature | 用途 |
| --- | --- |
| `testing` | `TestApp` 等（通常 `[dev-dependencies]` 打开） |
| `storage` | `LocalDisk` 上传落盘 |

```toml
phoenix = { package = "phoenixrs", version = "…", features = ["storage"] }
# 测试：
phoenix = { package = "phoenixrs", version = "…", features = ["testing"] }
```

也可直接 path 依赖 `phoenix-testing` / `phoenix-storage`。

## 文档同步

- `docs/DX.md` §10 已改为已实现 API 说明。
- `docs/SECURITY.md` §3.3 上传存储状态由安全轨/合并时同步。
