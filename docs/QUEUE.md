# 后台队列（phoenix-queue）

进程内后台任务队列首版：Job envelope、幂等键、重试 / 指数 backoff、dead-letter、Worker 优雅关闭，以及可选的 `phoenix-metrics` 挂钩。

设计边界见 [QUEUE_MAIL_CONSOLE.md](QUEUE_MAIL_CONSOLE.md)。

## 公开 API

| 类型 | 作用 |
| --- | --- |
| `JobId` / `JobEnvelope` | 任务身份与可序列化信封（payload 在 `Debug` 中脱敏） |
| `QueueBackend` | `push` / `reserve` / `ack` / `fail` / `dead_letter` / 可选 `purge_expired_idempotency` |
| `PushResult` | `Created(id)` 或 `Existing(id)`（幂等命中） |
| `MemoryQueue` | 单进程实现，供测试与本地开发 |
| `Queue` | 门面：`push_json`、`dispatch`、`dispatch_once` |
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

生产驱动（Redis 等）仍待后续；首版内置 `MemoryQueue`。

## 测试

```bash
cargo test -p phoenix-queue --locked
cargo clippy -p phoenix-queue --all-targets --locked -- -D warnings
```
