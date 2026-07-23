# 队列、邮件与应用控制台

## 目标（2026-07-23）

| 能力 | Crate | 验收 |
| --- | --- | --- |
| 队列 | `phoenix-queue` | Job envelope、重试/backoff、幂等键、dead-letter、worker 优雅关闭、Memory backend + metrics |
| 邮件 | `phoenix-mail` | Message builder、text/HTML、Header 注入防护、`MailTransport`、`MemoryTransport` 测试 |
| 应用 CLI | `phoenix-console` | 项目二进制名即命令前缀；用户只定名 + 写 `async fn`；内置 `serve` / migrate 族 |

## 应用控制台 UX（用户诉求）

新建项目包名为 `xxx` 时：

```bash
xxx serve              # 启动 HTTP（原 cargo run 默认行为改到此子命令）
xxx update             # 用户自定义：只需实现 update
xxx migrate            # 内置，替代单独 phoenix-manage 调用路径（仍兼容）
px make:command Update # 生成 app/commands/update.rs 并自动注册
```

用户侧最小写法：

```rust
// app/commands/update.rs
use phoenix::prelude::*;

pub async fn update(_ctx: CommandContext<'_>) -> CommandResult {
    println!("Software updated.");
    Ok(())
}

// app/commands/mod.rs
phoenix::commands! {
    update,
}
```

框架负责：argv 解析、帮助、未知命令错误、把 `serve` 接到应用启动闭包。不必手写 clap 样板。

## 队列首版边界

- 信封：`JobId` / `JobEnvelope`（id、name、payload JSON、attempts、max_attempts、idempotency_key、available_at、created_at）；payload `Debug` 脱敏
- `QueueBackend`：`push`、`reserve`、`ack`、`fail(available_at)`、`dead_letter`、可选 `purge_expired_idempotency`
- `MemoryQueue`：单进程；相同 `idempotency_key` 且仍在 queued/reserved → `PushResult::Existing`（不替换 payload）；ack/dead-letter 后释放键
- `Queue` 门面：`push_json` / `dispatch` / `dispatch_once`
- Handler：`JobHandler` trait 或 `Fn(JobEnvelope) -> Future<Result<(), JobError>>`
- Worker：reserve → handle → ack/fail/dead；指数 backoff；`ShutdownSignal`/`ShutdownToken`；可配 `poll_interval`
- 可选 `Metrics::record_job`（Completed / Failed / Retried）
- 用法文档：[QUEUE.md](QUEUE.md)

## 邮件首版边界

详见 [MAIL.md](MAIL.md)。

- `Address`：非空、含 `@`、无控制字符
- `Message` / `MessageBuilder`：from/to/cc/bcc/subject/text/html；subject 与地址字段拒绝 CR/LF Header 注入
- `MailError`：`InvalidAddress` / `HeaderInjection` / `MissingFrom` / `NoRecipients` / `Transport`
- `MailTransport::send(&Message) -> BoxFuture<Result<(), MailError>>`
- `MemoryTransport`：线程安全记录；`sent()` / `clear()` 供断言
- `Mailer`：`Arc<dyn MailTransport>` 门面
- 不内置真实 SMTP（`// future SmtpTransport`）；应用可 `impl MailTransport`

## 脚手架变更

- `src/main.rs` 改为 `Console` 入口；`px dev` 的 Rust 命令变为 `cargo run -- serve`
- `src/bin/phoenix-manage.rs` **保留**现有 migrate 路径（本轨不强制合并到 console，降低风险）；`px migrate` 仍调用 `phoenix-manage`
- 新增 `app/commands/`、`px make:command <Name>`（生成 `async fn` 并写入 `commands!` 托管区块）
- README 模板补充：`cargo run -- serve` / `cargo run -- update`

## 用户最小示例

```rust
// app/commands/update.rs
use phoenix::prelude::{CommandContext, CommandResult};

pub async fn update(_ctx: CommandContext<'_>) -> CommandResult {
    println!("Software updated.");
    Ok(())
}

// app/commands/mod.rs
use phoenix::prelude::commands;

// <phoenix:modules>
pub mod update;
pub use update::update;
// </phoenix:modules>

commands! {
// <phoenix:commands>
update,
// </phoenix:commands>
}

// src/main.rs
Console::new(env!("CARGO_PKG_NAME"))
    .about("My application")
    .serve(|_ctx| async move { /* start HTTP */ Ok(()) })
    .commands(commands::registry())
    .run()
    .await?;
```

## 并行轨道

- A：`phoenix-queue`
- B：`phoenix-mail`
- C：`phoenix-console` + CLI/scaffold/`make:command`
