# 任务调度（phoenix-schedule）

Laravel 风格的定时任务首版：链式 DSL 声明任务、自研五段 cron 解析、纯函数 `next_run` 内核、`schedule:run`（一轮即退，配外部 crontab）与 `schedule:work`（常驻循环）两种运行模式、进程内防重叠，以及复用 `phoenix-queue` 的 `ShutdownSignal` 优雅退出。

## 公开 API

| 类型 / 函数 | 作用 |
| --- | --- |
| `Schedule` | 调度器：`new()` → `.job(name, spec, task)` 链式注册；`utc_offset_secs` / `utc_offset_hours` 设定墙钟时区 |
| `Spec` | 触发规则：`Every` / `DailyAt` / `Cron` |
| `every_seconds` / `every_minutes` / `every_hours` | 固定间隔，对齐 Unix epoch 整倍数（`every_minutes(30)` 即 `:00` 与 `:30`） |
| `daily_at("03:00")` / `try_daily_at` | 每天固定墙钟时刻（非法输入 panic / 返回 `Err`） |
| `cron("0 3 * * *")` / `try_cron` | 标准五段 cron（分 时 日 月 周） |
| `next_run(after, &spec)` / `next_run_with_offset` | **纯函数内核**：严格晚于 `after` 的下一次触发时间；仅不可达 cron 返回 `None` |
| `Schedule::run_due(now)` | 跑一轮当前分钟内到期任务，等待全部完成，返回 `RunSummary` |
| `Schedule::work(token)` | 常驻循环：睡到最近的到期时刻（至少每分钟检查一次），直到收到关闭信号 |
| `RunSummary` | `due` / `completed` / `failed` / `skipped`（防重叠跳过）计数 |
| `ScheduledTask` / `TaskOutcome` / `TaskResult` | 任务闭包契约；闭包可返回 `()` 或 `Result<(), E>` |
| `console_commands(Arc<Schedule>)` | 生成 `schedule:run` / `schedule:work` 两个 `phoenix-console` 命令 |
| `ShutdownSignal` / `ShutdownToken` | 自 `phoenix-queue` 重导出，队列 worker 与调度器共用一套优雅退出 |
| `ScheduleError` | `InvalidCron` / `InvalidDailyTime` |

## DSL

```rust
use std::sync::Arc;

use phoenix_schedule::{Schedule, cron, daily_at, every_minutes};

let schedule = Arc::new(
    Schedule::new()
        // .utc_offset_hours(8)  // 可选：cron / daily_at 按 UTC+8 解释，默认 UTC
        .job("sitemap", every_minutes(30), || async {
            // 重新生成 sitemap …
        })
        .job("nightly-report", cron("0 3 * * *"), || async {
            // 闭包也可以返回 Result；Err 会记入 failed 并打 tracing 日志
            Ok::<(), std::io::Error>(())
        })
        .job("digest", daily_at("08:30"), || async {}),
);
```

任务闭包为 `Fn() -> Future`，返回 `()` 或 `Result<(), E>` 均可。`Every` 间隔与时区无关；`daily_at` / `cron` 属于墙钟规则，默认按 UTC 解释，可用 `utc_offset_secs` / `utc_offset_hours` 设定固定偏移（暂不处理夏令时）。

## cron 语法与边界

五段：`分 时 日 月 周`，各段支持：

- `*`、数字、`,` 列表、`A-B` 区间、`*/S` / `A-B/S` / `A/S` 步进（`A/S` 即 `A..=最大值` 按 `S` 步进）
- 周日 `0` 与 `7` 等价；不支持月份 / 星期英文名
- **日 + 周并集**：当"日"和"周"都不是 `*` 时，任一匹配即触发（Vixie cron 标准语义），如 `0 0 13 * 5` = 每月 13 号**或**每周五
- `next_run` 严格晚于 `after`：恰好落在触发点上时返回下一个触发点
- 跨月 / 跨年 / 闰年正确：`0 0 31 * *` 自动跳过小月；`0 0 29 2 *` 会等到下一个闰年（含 2100 非闰年的世纪规则）
- 不可达表达式（如 `0 0 30 2 *`、`0 0 31 4 *`）`next_run` 返回 `None`，任务永不触发
- 非法表达式（段数不对、越界、`*/0`、倒置区间等）：`cron()` panic，`try_cron()` 返回 `ScheduleError::InvalidCron`

## 两种运行模式

以 `phoenix-console` 命令方式接入应用二进制（`src/main.rs`）：

```rust
Console::new(env!("CARGO_PKG_NAME"))
    .serve(|_ctx| async move { /* … */ Ok(()) })
    .commands(commands::registry())
    .commands(phoenix_schedule::console_commands(Arc::clone(&schedule)))
    .run()
    .await
```

之后 `cargo run -- schedule:run`、生产环境 `bin/<app> schedule:run`，或经 `px` 转发：`px schedule:run` / `px schedule:work`（在项目根执行，等价于 `cargo run -- schedule:…`）。

### `schedule:run` — 一轮即退（外部 crontab 驱动）

跑完**当前分钟内**到期的任务后退出。判定：任务自本分钟起点（含）的下一次触发时间落在本分钟内即为到期；亚分钟间隔任务在此模式下每轮只跑一次。生产 crontab：

```cron
* * * * * cd /srv/myapp/current && bin/myapp schedule:run >> storage/schedule.log 2>&1
```

### `schedule:work` — 常驻循环

常驻进程：睡到最近的到期时刻（最长一分钟醒一次），到期即触发；亚分钟间隔（`every_seconds`）按精确节奏执行。Ctrl-C（SIGINT）触发 `ShutdownSignal` 优雅退出：不再派发新任务，等待在途任务完成后返回。systemd 示例：

```ini
[Unit]
Description=myapp scheduler
After=network.target

[Service]
WorkingDirectory=/srv/myapp/current
ExecStart=/srv/myapp/current/bin/myapp schedule:work
Restart=always
KillSignal=SIGINT
TimeoutStopSec=90

[Install]
WantedBy=multi-user.target
```

两种模式二选一即可；`schedule:run` + crontab 更契合 `px release` 的目录切换发布（每分钟拉起的总是 `current` 指向的新版本），`schedule:work` 则需在发布后重启服务。

## 防重叠与失败语义

- **防重叠（默认进程内）**：同名任务上一次执行未结束时，本轮直接跳过并记 `skipped`（`tracing::warn`）。默认用进程内锁（`InProcessLock`），仅保证单进程不重叠。多实例部署注入分布式锁见下方「跨进程锁」。
- **失败隔离**：任务返回 `Err` 或 panic 只记入 `failed` 并打 `tracing::error` 日志，不影响同轮其它任务，也不会中止 `schedule:work` 循环。
- 长任务跨过下一个触发点时，错过的触发不补跑（跳过语义，非排队）。

## 跨进程锁（多实例部署）

`run_due` 由 crontab 每分钟拉起新进程、或多机同时跑 `schedule:work` 时，进程内锁挡不住跨进程重叠。为此引入 `ScheduleLock` 抽象，把防重叠从进程内 `AtomicBool` 改造成「默认 `InProcessLock`，可注入分布式锁」：

| 类型 / 方法 | 作用 |
| --- | --- |
| `ScheduleLock` | `try_acquire(name, ttl) -> Option<LockGuard>`（异步，返回 `BoxLockFuture`）；`LockGuard` Drop 释放 |
| `InProcessLock` | 默认实现，按 job name 键控的进程内互斥（等价于旧的 per-job `AtomicBool`） |
| `Schedule::with_lock(Arc<dyn ScheduleLock>)` | 注入自定义锁；**未注入时行为与之前等价** |
| `Schedule::lock_ttl(Duration)` | 锁 TTL（默认 `DEFAULT_LOCK_TTL = 1h`）；仅对分布式锁有意义，兜底崩溃未释放的持有者 |

`phoenix-redis` 提供 `RedisScheduleLock`：

```rust
use std::sync::Arc;
use std::time::Duration;

use phoenix_redis::RedisStores;
use phoenix_schedule::{Schedule, every_minutes};

let stores = RedisStores::connect("redis://127.0.0.1/").await?;
let schedule = Schedule::new()
    .with_lock(Arc::new(stores.schedule_lock()))
    .lock_ttl(Duration::from_mins(10)) // 需大于任务最长运行时长
    .job("sitemap", every_minutes(30), || async {
        // 整个集群同一时刻只有一个实例在跑 sitemap
    });
```

原子性与安全释放：

- **获取**：`SET phoenix:lock:{name} <token> NX PX <ttl>`——原子「不存在才写 + 带过期」。
- **释放（Drop）**：Lua `if GET == token then DEL`——**仅当仍持有自己的 token 才删**，绝不误删 TTL 过期后被别的实例重新获取的锁。释放是尽力而为（Drop 时在当前 tokio runtime 上派发）；若无 runtime，锁靠 PX TTL 自然过期。
- **失败关闭**：Redis 不可达时 `try_acquire` 返回 `None`（跳过本次运行）并打 `tracing::warn`，宁可少跑也不重复跑。
- **TTL 取舍**：TTL 必须大于任务最长运行时长，否则锁中途过期、第二实例可能并发启动；TTL 过大则崩溃后该 job 全集群阻塞到过期。

调度器仍复用同一套 `ShutdownSignal` 优雅退出，`schedule:run` / `schedule:work` 两种模式接入方式不变。

## 在 `phoenix` prelude / feature 中暴露

门面提供可选 feature **`schedule`**（隐含 `queue`，依赖 `phoenix-schedule` 并 prelude 重导出；`console_commands` 别名为 `schedule_commands`）：

```toml
phoenix = { package = "phoenixrs", version = "…", features = ["schedule"] }
```

```rust
use phoenix::prelude::*; // Schedule / cron / every_minutes / schedule_commands 等
```

## 测试

```bash
cargo test -p phoenix-schedule --locked
cargo clippy -p phoenix-schedule --all-targets --locked -- -D warnings
```
