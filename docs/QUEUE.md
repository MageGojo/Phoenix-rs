# 后台队列（phoenix-queue）

进程内后台任务队列首版：Job envelope、幂等键、重试 / 指数 backoff、dead-letter、Worker 优雅关闭，以及可选的 `phoenix-metrics` 挂钩。

设计边界见 [QUEUE_MAIL_CONSOLE.md](QUEUE_MAIL_CONSOLE.md)。

## 公开 API

| 类型 | 作用 |
| --- | --- |
| `JobId` / `JobEnvelope` | 任务身份与可序列化信封（payload 在 `Debug` 中脱敏） |
| `QueueBackend` | `push` / `reserve` / `ack` / `fail` / `dead_letter` / 可选 `reclaim_expired` / `purge_expired_idempotency` |
| `PushResult` | `Created(id)` 或 `Existing(id)`（幂等命中） |
| `MemoryQueue` | 单进程实现，供测试与本地开发；可选 `with_visibility_timeout` |
| `Queue` | 门面：`push_json`、`dispatch`、`dispatch_in`（延迟）、`dispatch_once` |
| `JobHandler` / `JobError` | Handler trait；支持 `Fn(JobEnvelope) -> Future` |
| `Worker` / `WorkerConfig` | 循环处理；可配置 `poll_interval`、`base_backoff` |
| `ShutdownSignal` / `ShutdownToken` | `watch` 通道优雅关闭 |
| `backoff_delay` | `base * 2^(attempts-1)`，上限 1 小时 |

## 幂等键语义

同一 `idempotency_key` 在任务仍处于 **queued 或 reserved** 时再次 `push`，返回 `PushResult::Existing(原 id)`，**不替换** payload。

`ack` 或 `dead_letter` 之后释放该键，可再次使用。`MemoryQueue::purge_expired_idempotency` 为 no-op（键在终态即释放）。

## 用法

```rust
use std::sync::Arc;
use std::time::Duration;

use phoenix_metrics::Metrics;
use phoenix_queue::{
    JobEnvelope, JobError, MemoryQueue, PushOptions, Queue, ShutdownSignal, Worker, WorkerConfig,
};

let backend = Arc::new(MemoryQueue::new());
let queue = Queue::new(Arc::clone(&backend));
let metrics = Metrics::new();

queue
    .push_json(
        "send-welcome",
        serde_json::json!({ "user_id": 42 }),
        PushOptions::new()
            .max_attempts(5)
            .idempotency_key("welcome:42"),
    )
    .await?;

// 或快捷 dispatch / dispatch_once
queue.dispatch("ping", serde_json::json!({})).await?;
queue
    .dispatch_once("ping", serde_json::json!({}), "ping:once")
    .await?;

let signal = ShutdownSignal::new();
let worker = Worker::new(
    Arc::clone(&backend),
    |job: JobEnvelope| async move {
        if job.name == "send-welcome" {
            Ok(())
        } else {
            Err(JobError::retryable("unknown job"))
        }
    },
    signal.token(),
)
.with_config(WorkerConfig::default().poll_interval(Duration::from_millis(50)))
.with_metrics(metrics);

tokio::spawn(async move { worker.run().await });
// …稍后
signal.shutdown();
```

## 延迟任务

入队时可指定延迟，任务到期后才会被 worker 领取执行：

```rust
use std::time::Duration;

// 快捷方式：10 分钟后才可执行
queue
    .dispatch_in("send-reminder", serde_json::json!({ "user_id": 42 }), Duration::from_mins(10))
    .await?;

// 或与其它选项组合
queue
    .push_json(
        "send-reminder",
        serde_json::json!({ "user_id": 42 }),
        PushOptions::new()
            .delay(Duration::from_mins(10))
            .idempotency_key("reminder:42"),
    )
    .await?;
```

语义：

- `PushOptions::delay` / `JobEnvelope::with_delay` 设置 `available_at = created_at + delay`；`QueueBackend::reserve` 只交出 `available_at <= now` 的任务（重试 backoff 复用同一机制）。
- 延迟任务不会阻塞已就绪任务：`MemoryQueue` 在 reserve 时跳过未到期任务，就绪任务照常 FIFO。
- 延迟对任意后端语义一致：`available_at` 随信封序列化（秒精度），后续 Redis 等持久后端只需在 reserve 时遵守同一约定。
- 幂等键语义不变：延迟期间任务处于 queued 状态，同键 `push` 返回 `Existing`。

### Handler 错误

- `JobError::Retryable`：若 `attempts < max_attempts` → `fail` + 指数 backoff，记 `JobOutcome::Retried`
- `JobError::Permanent` 或已用尽 attempts → `dead_letter`，记 `JobOutcome::Failed`
- `Ok(())` → `ack`，记 `JobOutcome::Completed`

## 在 `phoenix` prelude / feature 中暴露

门面已提供可选 feature **`queue`**（依赖 `phoenix-queue` 并 prelude 重导出）。应用侧：

```toml
phoenix = { package = "phoenixrs", version = "…", features = ["queue"] }
# 或 path 依赖：phoenix = { path = "…/crates/phoenix", features = ["queue"] }
```

```rust
use phoenix::prelude::*; // MemoryQueue / Worker / JobHandler 等
```

本地 / 测试用 `MemoryQueue`；多实例、重启不丢的生产部署用下方 Redis 持久化后端。

## Redis 持久化后端

`phoenix-redis` 的 `RedisQueue` 实现同一个 `QueueBackend` trait，语义与 `MemoryQueue` 对齐但**持久化**且**多实例安全**：所有状态迁移都是单条原子 Lua 脚本（Redis 串行执行脚本），两个 worker 绝不会领到同一任务。见 `docs/REDIS.md`。

```rust
use std::sync::Arc;
use std::time::Duration;

use phoenix_queue::{Queue, QueueBackend};
use phoenix_redis::RedisStores;

let stores = RedisStores::connect("redis://127.0.0.1/").await?;
// 同一 name 的多个实例共享同一条持久队列
let backend = Arc::new(
    stores.queue("emails").with_visibility_timeout(Duration::from_secs(60)),
);
let queue = Queue::new(Arc::clone(&backend));

queue.dispatch("send-welcome", serde_json::json!({ "user_id": 42 })).await?;
// worker 侧照常 Worker::new(Arc::clone(&backend), handler, token).run()
```

### 键设计（按队列 name 隔离）

| 键 | 类型 | 内容 |
| --- | --- | --- |
| `phoenix:queue:{name}:ready` | ZSET | score = `available_at`（unix 秒），member = job id —— 延迟 / backoff 时间线 |
| `phoenix:queue:{name}:reserved` | ZSET | score = 可见性截止时刻，member = job id —— 处理中任务 |
| `phoenix:queue:{name}:jobs` | HASH | job id → 信封 JSON（存在 == queued 或 reserved） |
| `phoenix:queue:{name}:attempts` | HASH | job id → 领取次数 |
| `phoenix:queue:{name}:idem` | HASH | 幂等键 → job id（仅在 in-flight 期间） |
| `phoenix:queue:{name}:dead` | LIST | 死信 `{"attempts":N,"envelope":…}` 记录 |

### 语义

- **延迟任务**：`reserve` 只交出 ready set 中 `available_at <= now` 的成员，`JobEnvelope::with_delay` / 重试 backoff 直接复用同一时间线，不阻塞已就绪任务。
- **可见性超时（至少一次投递）**：`reserve` 会让任务在 `reserved` set 中隐身到截止时刻；worker 崩溃未 ack/fail/dead_letter 时，任务会被重新放回 ready（下一次 `reserve` 惰性回收，或显式调用 `reclaim_expired`）。因此同一任务可能被投递多次——**handler 必须幂等**（`idempotency_key` 是入队去重提示，不是执行去重）。默认超时 `DEFAULT_VISIBILITY_TIMEOUT = 30s`，用 `with_visibility_timeout(Duration::ZERO)` 可关闭回收（reserved 直到终态）。
- **重试 / 死信**：`fail` 把 reserved 任务按新的 `available_at`（指数 backoff）放回 ready，`attempts` 不减；`attempts >= max_attempts` 或永久失败时 `dead_letter` 追加到死信 LIST。`RedisQueue::dead_letters()` 读回死信并还原 `attempts`。
- **幂等**：某幂等键的任务仍 in-flight 时，重复 `push` 该键返回原 id（`PushResult::Existing`），不替换 payload；`ack` / `dead_letter` 后释放该键。
- **原子性**：`push` / `reserve` / `ack` / `fail` / `dead_letter` / `reclaim_expired` 各为一条 Lua 脚本，参照 `rate_limit.rs` 的用法；死信记录用字符串拼接包裹原始信封 JSON，避免 cjson 重新编码破坏 payload（大整数精度）。

### trait 扩展（向后兼容）

为表达可见性超时，`QueueBackend` 新增一个**带默认实现**的方法：

```rust
fn reclaim_expired(&self) -> impl Future<Output = Result<usize, QueueError>> + Send {
    async { Ok(0) }
}
```

默认 no-op，既有后端与 `Worker` 无需改动即可编译（完全向后兼容）。`MemoryQueue` 也实现了它并新增 `with_visibility_timeout`，因此回收逻辑可离线单测；`reserve` 内部同样会惰性回收，任何活跃 worker 都能捞回卡住的任务，无需专门的清扫器。

## 测试

```bash
cargo test -p phoenix-queue --locked
cargo clippy -p phoenix-queue --all-targets --locked -- -D warnings
```
