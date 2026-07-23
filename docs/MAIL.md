# 邮件门面（phoenix-mail）

## 目标

提供 Laravel 风格的出站邮件门面：校验地址、构建 multipart 消息、拒绝 Header 注入，并通过可插拔 `MailTransport` 投递。首版只内置 `MemoryTransport` 供测试；真实 SMTP 留给后续适配。

设计总览见 [QUEUE_MAIL_CONSOLE.md](QUEUE_MAIL_CONSOLE.md)。

## 公开 API

| 类型 | 职责 |
| --- | --- |
| `Address` | 基础邮箱校验（非空、含 `@`、无控制字符） |
| `Message` / `MessageBuilder` | `from` / `to` / `cc` / `bcc` / `subject` / `text_body` / `html_body` |
| `MailError` | 稳定错误：`InvalidAddress`、`HeaderInjection`、`MissingFrom`、`NoRecipients`、`Transport` |
| `MailTransport` | `send(&Message) -> BoxFuture<Result<(), MailError>>` |
| `MemoryTransport` | 线程安全记录已发送邮件；`sent()` / `clear()` / `len()` |
| `Mailer` | 持有 `Arc<dyn MailTransport>`，`send(message)` |

```rust
use std::sync::Arc;
use phoenix_mail::prelude::*;

let transport = MemoryTransport::new();
let mailer = Mailer::new(Arc::new(transport.clone()));

let message = Message::builder()
    .from("noreply@example.com")
    .to("user@example.com")
    .subject("Welcome")
    .text_body("Hello")
    .html_body("<p>Hello</p>")
    .build()?;

mailer.send(message).await?;
assert_eq!(transport.sent().len(), 1);
```

或测试快捷方式：`let (mailer, transport) = Mailer::memory();`

## Header 注入防护

- `subject` 与地址字段（`from`/`to`/`cc`/`bcc`）禁止 CR、LF 及其它 Unicode 控制字符 → `MailError::HeaderInjection`
- `text_body` / `html_body` **允许**换行（正文不是 header）

## 收件人规则

- 必须有 `from`
- `to` + `cc` + `bcc` 至少一个非空，否则 `NoRecipients`
- 允许仅 `bcc`

## 非目标（首版）

- 真实 SMTP / Sendmail / SES 驱动（可留 `// future SmtpTransport`，应用可自行 `impl MailTransport`）
- MIME multipart 序列化、附件、DKIM、模板引擎

## Prelude / feature

门面可选 feature **`mail`** 已接线：`phoenix = { features = ["mail"] }` 后从 `phoenix::prelude::*` 使用 `Mailer` / `EmailMessage`（避免与 WebSocket `Message` 冲突）等。crate 内亦可 `phoenix_mail::prelude::*`。

## 验收

```bash
cargo test -p phoenix-mail --locked
cargo clippy -p phoenix-mail --all-targets --locked -- -D warnings
```
