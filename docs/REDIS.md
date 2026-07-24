# Redis 生产适配器

## 目标

为已存在的三个 contract 提供 Redis 实现，使多实例部署可以共享状态：

- `SessionBackend`（`phoenix-security`）
- `RateLimitBackend`（`phoenix-security`）
- `TokenStore`（`phoenix-crypto`）

内置 Memory/File 实现保持不变，仅用于本地与契约测试。

## 新 crate：`phoenix-redis`

```text
crates/phoenix-redis/
  Cargo.toml
  src/lib.rs
  src/session.rs
  src/rate_limit.rs
  src/token.rs
  src/keys.rs
  tests/contracts.rs
```

依赖：`redis`（tokio + connection-manager）、`serde_json`、`phoenix-security`、`phoenix-crypto`、`phoenix-http`（`BoxFuture`）。

## 键空间

统一前缀 `phoenix:`，用途隔离：

| 用途 | 键模式 | 备注 |
| --- | --- | --- |
| Session | `phoenix:session:{id}` | JSON + version；TTL = absolute expires（逻辑时钟测试用长 TTL + JSON `expires_at`） |
| Rate limit | `phoenix:rl:{key}` | 窗口起点 + 计数；TTL = 剩余 window |
| Refresh | `phoenix:token:refresh:{hash}` | 仅存 hash 后的 refresh 记录（明文 refresh 永不入库） |
| Family members | `phoenix:token:family_members:{id}` | SET：家族内 token hash，供 reuse 撤销扫最大过期 |
| Family revoke | `phoenix:token:family:{id}` | 过期时间戳 |
| Access revoke | `phoenix:token:access:{jti}` | 过期时间戳 |

禁止把明文 refresh token、Cookie 值或用户密码写入 Redis。

## 原子语义

### Session

- `load`：GET；未过期时可延长滑动 TTL（写回 `expires_at`），**不**提升 version。
- `create`：SET NX + EXPIRE；冲突 → `Collision`。
- `save`/`delete`：Lua 比较 `expected_version`，成功则写/删并刷新 TTL。
- `rotate`：单 Lua：校验旧 ID version → 写新 ID → 删旧 ID；任一步失败整体回滚语义。

### Rate limit

单 Lua `hit`：窗口过期重置计数，否则 INCR；返回 allowed/remaining/retry_after。

### TokenStore

- `rotate_refresh` 必须原子检测 reuse：旧 hash 已标记用过 → `Reused` 并 revoke family。
- 与 `MemoryTokenStore` / `FileTokenStore` 行为对齐，以现有 crypto 测试为 oracle。

## 用法

```rust
use std::sync::Arc;

use phoenix_crypto::TokenService;
use phoenix_redis::RedisStores;
use phoenix_security::{RateLimit, RateLimitConfig, SessionMiddleware};

let stores = RedisStores::connect("redis://127.0.0.1/").await?;
// Debug 输出会脱敏 URL 密码：redis://user:***@host/db

let sessions = Arc::new(stores.session());
let limiter = RateLimit::with_backend(
    RateLimitConfig::default(),
    Arc::new(stores.rate_limit()),
);
let tokens = TokenService::new(jwt, Arc::new(stores.token()), refresh_ttl)?;

// 也可用设计文档别名：
// use phoenix_redis::RedisBackends;
// let stores = RedisBackends::connect("redis://127.0.0.1/").await?;
```

连接失败在 `connect` / `from_client` 时返回 `RedisConnectError`；单次命令失败映射为各 store 的 backend error（Session 对外 503；限流按 middleware 的 fail-closed / fail-open）。

## 测试

```bash
# 无 Redis：单元测试（键编码、URL 脱敏、错误映射）必须通过
cargo test -p phoenix-redis --locked
cargo clippy -p phoenix-redis --all-targets --locked -- -D warnings

# 有 Redis：双客户端共享 contract（session conflict/rotate、限流累计、refresh reuse）
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1/0 cargo test -p phoenix-redis --locked
```

未设置 `PHOENIX_TEST_REDIS_URL` 时，集成测试直接 return（不算失败）。

## 集成建议（`phoenix` crate）

门面可选 feature：

```toml
phoenix = { package = "phoenixrs", features = ["redis"] }
# RedisTokenStore 另需 jwt：
phoenix = { package = "phoenixrs", features = ["redis", "jwt"] }
```

启用后从 `phoenix::prelude::*` / `phoenix::redis` 使用 `RedisStores`、`RedisSessionBackend`、`RedisRateLimitBackend`；`RedisTokenStore` 在同时启用 `jwt` 时可用。

契约测：

```bash
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1/0 cargo test -p phoenix-redis --locked --features jwt
```