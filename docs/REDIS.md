# Redis 生产适配器

## 目标

为已存在的 contract 提供 Redis 实现，使多实例部署可以共享状态：

- `SessionBackend`（`phoenix-security`）
- `RateLimitBackend`（`phoenix-security`）
- `TokenStore`（`phoenix-crypto`，`jwt` feature）
- `QueueBackend`（`phoenix-queue`，`queue` feature）—— 持久化后台队列，见 `docs/QUEUE.md`「Redis 持久化后端」
- `ScheduleLock`（`phoenix-schedule`，`schedule` feature）—— 调度器跨进程锁，见 `docs/SCHEDULE.md`「跨进程锁」

内置 Memory/File 实现保持不变，仅用于本地与契约测试。`queue` / `schedule` 为**默认开启**的 feature；门面 `phoenix` 以 `default-features = false` 引用本 crate，故 `redis` feature 不会连带拉入队列 / 调度依赖。

## 新 crate：`phoenix-redis`

```text
crates/phoenix-redis/
  Cargo.toml
  src/lib.rs
  src/session.rs
  src/rate_limit.rs
  src/token.rs
  src/queue.rs           # RedisQueue（QueueBackend）
  src/schedule_lock.rs   # RedisScheduleLock（ScheduleLock）
  src/keys.rs
  tests/contracts.rs
  tests/queue_lock_contracts.rs
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
| Queue（每 name） | `phoenix:queue:{name}:{ready,reserved,jobs,attempts,idem,dead}` | ZSET/ZSET/HASH/HASH/HASH/LIST，详见 `docs/QUEUE.md` |
| Schedule lock | `phoenix:lock:{name}` | 锁 token 字符串，`SET NX PX`；Drop 时按 token 安全释放 |
| 实时广播 | `phoenix:ws:broadcast` | **pub/sub 频道，不是键**：不落盘、不占内存、`KEYS` 看不到 |
| 加密传输会话 | `phoenix:secure:{key_id}` | **存的是密钥material**：base64 的 AES-256-GCM 会话密钥，带 `PX` 过期 |

禁止把明文 refresh token、Cookie 值或用户密码写入 Redis。

> `phoenix:secure:*` 是唯一存放密钥material 的键空间。能读它的人能解密所有在线页面会话的流量：走 TLS + `AUTH`，优先关闭持久化，`session_ttl` 保持短。不需要多实例可互换时，用粘性路由 + 进程内存储更安全。详见 [SECURE_TRANSPORT.md](SECURE_TRANSPORT.md)。

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

### QueueBackend（`RedisQueue`）

- 每个状态迁移（`push` / `reserve` / `ack` / `fail` / `dead_letter` / `reclaim_expired`）都是单条 Lua 脚本，Redis 串行执行即天然互斥，多 worker 不会重复领取。
- `reserve` 先把 `reserved` 中超过可见性截止时刻的任务回收到 `ready`（惰性回收），再从 `ready` 弹出 `available_at <= now` 的任务、`HINCRBY` attempts、写入 `reserved`。
- 死信记录用**字符串拼接**包裹原始信封 JSON，不经 cjson 重新编码，避免破坏 payload（大整数精度、键序）。
- 语义与 `MemoryQueue` 对齐，详见 `docs/QUEUE.md`「Redis 持久化后端」。

### ScheduleLock（`RedisScheduleLock`）

- 获取：`SET phoenix:lock:{name} <uuid-token> NX PX <ttl>`。
- 释放：Lua `if GET == token then DEL`，**按 token 安全释放**，绝不误删他人的锁（TTL 过期被别的实例重获后尤为关键）。
- Redis 不可达时 fail-closed（`try_acquire` 返回 `None`，跳过本次运行）。

### SecureSessionStore（`RedisSecureSessionStore`）

- `insert` = `PSETEX phoenix:secure:{key_id} <剩余毫秒> <base64 密钥>`；`get` = `GET`。
- 过期完全交给 Redis 的 `PX`，不需要额外清理任务；已过期的 insert 直接跳过（负 TTL 会被 Redis 拒绝）。
- 读写失败**按会话不存在处理**（fail closed），绝不退回明文。
- `Debug` 不渲染连接对象——那会把带凭据的 Redis URL 打出来。

### Broadcaster（`RedisBroadcaster`）

- `publish` = `PUBLISH phoenix:ws:broadcast <json>`；`subscribe` = `SUBSCRIBE` 流。
- **无原子性可言，也不需要**：pub/sub 是 fire-and-forget，没有持久化、ack 或重放。断连期间的消息对该实例就是丢了。要「不丢」请用 `RedisQueue`。
- Hub 先本地送达再 publish，所以 Redis 故障 = 集群降级为互不相通的单实例，实时功能不整体失效。
- 断线自动重订阅（1s 退避）；载荷解析失败只跳过该条，不打断泵。
- 详见 [REALTIME.md](REALTIME.md)「跨实例广播」。

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

// 持久化队列 + 调度锁共享同一连接池：
let jobs = Arc::new(stores.queue("emails"));       // Arc<dyn QueueBackend>
let lock = Arc::new(stores.schedule_lock());       // Arc<dyn ScheduleLock>
```

连接失败在 `connect` / `from_client` 时返回 `RedisConnectError`；单次命令失败映射为各 store 的 backend error（Session 对外 503；限流按 middleware 的 fail-closed / fail-open）。

## 测试

```bash
# 无 Redis：单元测试（键编码、URL 脱敏、错误映射）必须通过
cargo test -p phoenix-redis --locked
cargo clippy -p phoenix-redis --all-targets --locked -- -D warnings

# 有 Redis：双客户端共享 contract（session conflict/rotate、限流累计、refresh reuse），
# 以及队列 / 锁 contract（持久入队→reserve→ack、延迟到点可见、nack 重试与死信、
# 可见性超时重投、幂等、双实例不重复领取、锁互斥 / TTL 过期 / Drop 释放 / 不误删他人锁）
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1/0 cargo test -p phoenix-redis --locked
```

```bash
# 跨实例广播 contract（两个 Hub + 两个 broadcaster 模拟两台实例）
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test -p phoenix-redis --test broadcast_contracts
# 跨进程加密会话 contract（在 A 握手、请求打到 B 仍拿到密文）
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test -p phoenix-redis --test secure_session_contracts
```

未设置 `PHOENIX_TEST_REDIS_URL` 时，集成测试（`tests/contracts.rs`、`tests/queue_lock_contracts.rs`、`tests/broadcast_contracts.rs`、`tests/secure_session_contracts.rs`）直接 return（不算失败）；Lua 逻辑之外的键编码、死信序列化、状态映射、以及广播的线上 JSON 格式有离线单测覆盖。

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