# 三路并行交付（2026-07-23）

目标：在互不阻塞的前提下同时完成实时协议、Redis 生产适配器、测试门面与上传存储。

| 轨道 | 主要产物 | 隔离边界 | 验收 |
| --- | --- | --- | --- |
| A · 实时 | SSE 收口 + WebSocket 门面 | `phoenix-http`、`phoenix-core`、`docs/REALTIME.md` | 单元 + 真实 TCP；Origin/大小/关闭边界 |
| B · Redis | Session/限流/Token Redis 适配器 | 新 `phoenix-redis` crate；不改 Memory 语义 | 双实例 contract 测试（可用 Redis 或嵌入式 mock） |
| C · DX | `phoenix-testing` + 本地上传存储 | 新 `phoenix-testing`、`phoenix-storage` | TestApp 断言 + 文件名净化落盘测试 |

## 合并规则

1. 各轨道在独立 git worktree/分支开发，避免同时改同一文件。
2. 允许各自改根 `Cargo.toml` / `Cargo.lock`；合并时以 A→B→C 顺序解决冲突，保留全部新 member。
3. 公共 prelude 重导出放最后一轮由集成者统一写入 `crates/phoenix`。
4. 真实密钥、生产 Redis URL 不进仓库；测试用 `PHOENIX_TEST_REDIS_URL`，缺失时跳过或使用内存假后端。

## 工具与约定

- Rust `1.95` / edition 2024；`cargo test` / 严格 Clippy / `fmt --check`。
- Redis 客户端优先 `redis` crate（tokio + connection-manager）；原子语义用 Lua。
- WebSocket 首版：HTTP/1.1 `Upgrade` + TLS 下的 WSS；HTTP/2 extended CONNECT 可留明确 TODO，但不得假称已交付。
- 上传存储默认本地磁盘，路径禁止 `..` 穿越；Multipart 文件字段可流式落盘。
- 中文文档同步更新：`REALTIME.md`、`SECURITY.md`、`DX.md`、`PROGRESS.md`、`NEXT.md`、`PRODUCTION_PLAN.md`。


## 合并结果（2026-07-23）

三轨均已合入工作树：prelude 已重导出 SSE/WS；`phoenix` 可选 feature `redis` / `storage` / `testing`。相关包测试与严格 Clippy 通过。
