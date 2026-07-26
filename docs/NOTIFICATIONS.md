# 通知系统（phoenix-notify）

## 目标

提供 Laravel Notification 的收敛版：一次 `send` 把同一条通知投递到多个通道。首版内置 **mail**（复用 `phoenix-mail`）与 **database**（`notifications` 表）双通道；广播 / 短信等留给后续适配。

邮件门面见 [MAIL.md](MAIL.md)；插件安装见 [FEATURES.md](FEATURES.md)。

## 公开 API

| 类型 | 职责 |
| --- | --- |
| `ChannelKind` | 通道枚举：`Mail` / `Database`（`as_str()` → `"mail"` / `"database"`） |
| `Notification` | `notification_type()`（写入 `type` 列）、`channels()`、`to_mail(&dyn Notifiable) -> Option<MailMessageBuilder>`、`to_database() -> Option<serde_json::Value>` |
| `Notifiable` | `notifiable_id() -> String`（字符串身份，兼容 `phoenix_auth::Principal::subject` / 整型主键 `to_string` / UUID）、`mail_address() -> Option<String>` |
| `MailMessage` / `MailMessageBuilder` | `phoenix_mail::Message` / `MessageBuilder` 的别名再导出 |
| `Notifier` | `new().with_mailer(Mailer).with_store(Arc<dyn NotificationStore>)`；`send` / `mark_read` / `unread_for` |
| `SendSummary` | 单次 send 的结果：`sent()` / `skipped()` / `stored_id()` / `sent_via(channel)` |
| `NotificationStore` | `insert` / `mark_read(id, read_at)` / `unread_for(notifiable_id)`，异步风格与全仓一致（`phoenix_http::BoxFuture`） |
| `MemoryNotificationStore` | 线程安全内存实现，供测试与本地开发；`all()` / `len()` |
| `DatabaseNotification` | 一行 `notifications` 记录：`id` / `notifiable_id` / `notification_type` / `data` / `read_at` / `created_at`，`is_read()` |
| `NotifyError` | 稳定错误：`ChannelUnconfigured`、`MissingMailAddress`、`Mail`、`DuplicateNotification`、`NotificationNotFound`、`Store` |
| `NotifyFeature` | `Plugin` 实现，仅注册 `notifications` 迁移（不注册路由） |

## 装配

```rust
use std::sync::Arc;
use phoenix_mail::Mailer;
use phoenix_notify::prelude::*;

let (mailer, transport) = Mailer::memory();          // 生产环境换成真实 MailTransport
let store = Arc::new(MemoryNotificationStore::new()); // Toasty 版 store 是下一里程碑

let notifier = Notifier::new()
    .with_mailer(mailer)
    .with_store(store.clone());
```

`Notifier` 是 `Clone`，启动时装配一次后放进应用状态共享。只装配用得到的通道即可：通知要求的通道**产出了内容**但通道未装配时，`send` 会 fail closed 返回 `ChannelUnconfigured`。

## 定义通知：支付成功示例

场景：支付回调处理完成后（如 `phoenix-pay` 的 `NotifyOutcome::Processed(event)` 分支，本 crate 不依赖它，仅约定形状），给下单用户发一条双通道通知。

```rust
use phoenix_notify::prelude::*;
use serde_json::json;

struct PaymentSucceeded {
    out_trade_no: String,
    amount: i64, // 分
}

impl Notification for PaymentSucceeded {
    fn notification_type(&self) -> &str {
        "payment.succeeded"
    }

    fn channels(&self) -> Vec<ChannelKind> {
        vec![ChannelKind::Mail, ChannelKind::Database]
    }

    // 不带收件人：Notifier 会从 Notifiable::mail_address 补上 `to` 再 build，
    // 构建错误以 NotifyError::Mail 显式抛出，不会被吞掉。
    fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessageBuilder> {
        Some(
            MailMessage::builder()
                .from("noreply@example.com")
                .subject("支付成功")
                .text_body(format!("订单 {} 已支付", self.out_trade_no)),
        )
    }

    fn to_database(&self) -> Option<serde_json::Value> {
        Some(json!({ "out_trade_no": self.out_trade_no, "amount": self.amount }))
    }
}
```

接收方实现 `Notifiable`（在应用自己的用户类型上，不改框架代码）：

```rust
impl Notifiable for User {
    fn notifiable_id(&self) -> String {
        self.id.to_string()
    }

    fn mail_address(&self) -> Option<String> {
        Some(self.email.clone())
    }
}
```

发送与断言：

```rust
let summary = notifier.send(&user, &PaymentSucceeded {
    out_trade_no: event.out_trade_no.clone(),
    amount: 990,
}).await?;

assert!(summary.sent_via(ChannelKind::Mail));
assert!(summary.sent_via(ChannelKind::Database));
let id = summary.stored_id().unwrap(); // database 通道写入的记录 id
```

规则：

- `channels()` 中重复的通道只投递一次（按首次出现顺序）。
- `to_mail` / `to_database` 返回 `None` → 该通道跳过（计入 `skipped()`），其余通道照常投递。
- mail 通道要求 `mail_address()` 有值，否则 `MissingMailAddress`（fail closed，不做静默跳过）。

## database 通道与已读

`notifications` 表结构（`NotifyFeature` 注册的迁移，id `202607260002`，SQLite 优先，与 `payments` 迁移同款注记）：

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | TEXT PRIMARY KEY | UUID v4，由 `Notifier` 生成 |
| `notifiable_id` | TEXT NOT NULL | `Notifiable::notifiable_id()` |
| `type` | TEXT NOT NULL | `Notification::notification_type()` |
| `data` | TEXT NOT NULL | `to_database()` 的 JSON |
| `read_at` | TEXT | 未读为 NULL |
| `created_at` | TEXT NOT NULL | 默认 `CURRENT_TIMESTAMP` |

附索引 `notifications_notifiable_read (notifiable_id, read_at)`。

查询辅助（都要求已装配 store）：

```rust
let unread = notifier.unread_for(&user.notifiable_id()).await?; // 未读，旧的在前
let record = notifier.mark_read(&id).await?;                     // 幂等；重复标记保留首次 read_at
assert!(record.is_read());
```

## 安装 Feature（仅迁移）

```rust
use phoenix_plugin::FeatureSet;
use phoenix_notify::NotifyFeature;

let parts = FeatureSet::new()
    .plugin(NotifyFeature::new())?   // 与 pay 等其它插件链式安装
    .into_parts();
// parts.migrations 交给应用的 MigrationRunner；parts.routes 为空
```

`NotifyFeature` 刻意**不注册路由**：通知列表 / 已读接口涉及鉴权、分页、序列化，属应用层。应用侧 handler 示例：

```rust
use std::sync::Arc;
use phoenix_http::{IntoResponse, Json, Request, Response};
use phoenix_routing::Routes;
use phoenix_notify::Notifier;
use serde_json::json;

fn notification_routes(notifier: Arc<Notifier>) -> Routes {
    let list = Arc::clone(&notifier);
    let read = notifier;
    Routes::new()
        .get("/notifications", move |request: Request| {
            let notifier = Arc::clone(&list);
            async move {
                let user_id = current_user_id(&request); // 应用自己的鉴权
                match notifier.unread_for(&user_id).await {
                    Ok(records) => Json(json!({
                        "unread": records.iter().map(|r| json!({
                            "id": r.id,
                            "type": r.notification_type,
                            "data": r.data,
                        })).collect::<Vec<_>>(),
                    })).into_response(),
                    Err(error) => Json(json!({ "message": error.to_string() }))
                        .into_response()
                        .with_status(phoenix_http::StatusCode::INTERNAL_SERVER_ERROR),
                }
            }
        })
        .name("notifications.index")
        .post("/notifications/{id}/read", move |request: Request| {
            let notifier = Arc::clone(&read);
            async move {
                let Some(id) = request.param("id") else {
                    return Json(json!({ "message": "missing id" })).into_response();
                };
                match notifier.mark_read(id).await {
                    Ok(record) => Json(json!({ "id": record.id, "read": true })).into_response(),
                    Err(error) => Json(json!({ "message": error.to_string() }))
                        .into_response()
                        .with_status(phoenix_http::StatusCode::NOT_FOUND),
                }
            }
        })
        .name("notifications.read")
}
```

## 与队列配合

`Notifier::send` 本身是普通 async 调用，天然适合作为 `phoenix-queue` 的 job handler 内容：应用把通知参数序列化进 job payload，worker 反序列化后重建通知并调用 `notifier.send`。crate 不内置 `queue()`（`Notification` 是非序列化 trait，硬塞注册表会破坏收敛），见下方未做清单。

## 非目标（首版）

- 广播（WebSocket/SSE）、短信、Slack 等更多通道（`ChannelKind` 预留扩展位）
- Toasty/数据库版 `NotificationStore`（迁移已就位，与 `phoenix-pay` 的 DB store 同一里程碑）
- 内置 `queue()` / `ShouldQueue`（需要应用侧通知注册表，见上节）
- `mark_all_read` / 分页 / 通知偏好 / 本地化
- 通知路由端点（应用层实现，示例见上）

## 验收

```bash
cargo test -p phoenix-notify --locked
cargo clippy -p phoenix-notify --all-targets --locked -- -D warnings
```
