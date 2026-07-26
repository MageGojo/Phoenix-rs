# 实时协议与流式请求

Phoenix 的实时能力分为流式请求、SSE 和 WebSocket。流式请求始终可用；SSE 与 WebSocket 分别由 Cargo feature **`sse`** / **`websocket`** 启用（二者不合并，见 [ADR-042](./DECISIONS.md)）。WebSocket 首版仅承诺 HTTP/1.1 Upgrade 与 TLS 下的 WSS。

```toml
phoenix = { package = "phoenixrs", features = ["sse"] }         # 仅 SSE
phoenix = { package = "phoenixrs", features = ["websocket"] }   # 仅 WS
phoenix = { package = "phoenixrs", features = ["sse", "websocket"] }
```

## 流式上传

默认路由在 handler 运行前完整读取 body。大文件、增量解析或转发场景必须显式使用 `streaming`：

```rust
use phoenix::prelude::*;

async fn upload(mut body: RequestBodyStream) -> Result<Response, RequestBodyError> {
    let mut received = 0_usize;
    while let Some(chunk) = body.next_chunk().await {
        received += chunk?.len();
    }
    Ok(Response::text(format!("received {received} bytes")))
}

let routes = Routes::new().post("/upload", streaming(typed(upload)));
```

Raw handler 也可以通过 `request.take_body_stream()` 取得同一个 one-shot stream。第二次提取会失败；流式路由的 `request.body()` 固定为空，不能与 `Json<T>`、`Form<T>` 或 `Multipart<T>` 混用。

## 限制与错误

- `Content-Length` 大于 `Application::max_body_size` 时，在 middleware 和 handler 前返回 413。
- chunked 或 HTTP/2 未知长度 body 在拉取过程中累计限额，超限产生 `RequestBodyError::TooLarge`。
- `body_read_timeout` 是整个 body 的绝对 deadline，不会因每一块到达而重新计时。
- deadline 映射 408；传输失败映射 400。客户端已经断开时通常无法再发送该响应，但 handler 仍可观察错误并停止外部工作。
- handler 不消费完整 HTTP/1 body 时，Hyper 只在能够安全排空时复用连接，否则关闭连接；剩余字节不会被当成下一请求。HTTP/2 错误隔离在单个 stream。

应用必须继续对落盘空间、解压后大小、解析复杂度和上游转发施加独立限制。网络 body 上限不等于业务资源上限。

## SSE

公开类型：`SseEvent`、`KeepAlive`、`Sse`、`LastEventId`。

```rust
use futures_util::stream;
use phoenix::prelude::*;

async fn ticks() -> impl IntoResponse {
    let events = stream::iter([
        SseEvent::new().data("hello"),
        SseEvent::new().json_data(&serde_json::json!({"n": 1})).unwrap(),
    ]);
    Sse::from_events(events)
        .keep_alive(KeepAlive::new(Duration::from_secs(15)).unwrap())
}
```

约定：

- `Content-Type: text/event-stream; charset=utf-8`，`Cache-Control: no-cache`，`X-Accel-Buffering: no`。
- 单事件大小默认 64 KiB、上限 1 MiB；字段拒绝 NUL/CR/LF 注入。
- 源错误在线路上脱敏为通用 stream 错误，不泄露内部细节。
- 客户端取消或服务关闭结束 stream，不阻塞优雅关闭（由响应 body lifecycle + cancellation token 驱动）。

## WebSocket（首版 · 已实现）

受控门面（HTTP/1.1 only）：

- 仅 HTTP/1.1 `Connection: upgrade` + `Upgrade: websocket`；服务端通过 Hyper `serve_connection_with_upgrades` 完成升级。
- TLS 监听器上即为 WSS；明文仅用于本地测试。
- Origin：默认 require allowlist 匹配（空 allowlist 全部拒绝）；`.allowed_origin(...)` 追加；`.any_origin()` 放宽（测试用）。
- 默认可配置单帧/消息大小上限（默认消息 64 KiB、帧 16 KiB，硬顶 16 MiB）；超限关闭连接。
- 应用通过 `WebSocketUpgrade` extractor 取得升级句柄，`on_upgrade` 返回 101 并在升级完成后回调 `WebSocket`（`recv` / `send` / `close`，含关闭码）。
- **HTTP/2 extended CONNECT（RFC 8441）明确未交付**；不要依赖内部 Hyper upgrade 类型绕过门面。

```rust
use phoenix::prelude::*;

async fn chat(ws: WebSocketUpgrade) -> Response {
    ws.allowed_origin("https://app.example")
        .on_upgrade(|mut socket| async move {
            while let Some(msg) = socket.recv().await {
                let Ok(msg) = msg else { break };
                if msg.is_text() {
                    let _ = socket.send(msg).await;
                }
            }
        })
}
```

集成注意：`phoenix` prelude 需重导出 `WebSocketUpgrade`、`WebSocket`、`Message`、`CloseCode`、`CloseFrame`、`WebSocketError`、`WebSocketUpgradeRejection`、`Sse`、`SseEvent`、`KeepAlive`、`LastEventId`（由集成者统一写入，本轨不改 `crates/phoenix`）。

## 广播 / 频道 / 在线状态（单实例 · 已实现）

裸 `WebSocket` 只有单连接 `recv`/`send`。`Hub` 在其上提供进程内广播中心：命名频道、加入/离开、频道内在线成员（presence）、定向发送，以及一个可覆盖的鉴权钩子。跨实例（Redis pub/sub）见下节，本节为单实例。

### 出站模型与背压

每个连接持有一条有界 `tokio::sync::mpsc` 队列。定向发送、频道广播、presence 事件都经这一条队列出站——它是每个连接**唯一**的背压点。选 per-connection `mpsc` 而非共享 `tokio::sync::broadcast`，是因为只有它能做「定向送达单连接」、把慢消费者的影响隔离在当事连接、并让策略显式化。

队列满时按 `SlowConsumer` 策略处理（`HubConfig` 配置，默认 `Disconnect`）：

- `SlowConsumer::Disconnect`（默认）：驱逐该连接——关闭其 `Outbound` 接收端，socket 泵随之停止。保证消息连续性优先，避免慢消费者悄悄丢消息导致状态发散。
- `SlowConsumer::DropMessage`：丢弃溢出消息、保留连接。有损但不断线，适合尽力而为的遥测式广播。

presence 事件恒为尽力而为（队列满即丢弃），因此驱逐不会级联触发更多驱逐。

### 公开类型

`Hub`、`HubBuilder`、`HubConfig`、`SlowConsumer`、`Connection`、`ConnectionId`、`ConnectionMeta`、`Outbound`、`Outgoing`（`Message` / `Presence` 两个变体）、`PresenceMember`、`PresenceEvent`、`PresenceEventKind`、`Authorizer`、`AllowAll`、`ConnectionContext`、`Broadcaster`、`LocalBroadcaster`、`PeerFrame`、`PeerStream`、`HubId`、`JoinError`、`SendError`。

### 鉴权钩子

`Authorizer::authorize(channel, &ConnectionContext) -> bool` 在 `join` 时被调用。默认 `AllowAll` 全部放行；框架**不硬编码**任何鉴权，应用通过 `HubBuilder::authorizer(...)` 覆盖（闭包 `Fn(&str, &ConnectionContext) -> bool` 直接可用）。`ConnectionContext` 暴露 `id()` / `key()` / `state()`（来自 `connect_as` 传入的 `ConnectionMeta`），供按用户/频道决策。拒绝时返回 `Err(JoinError::Unauthorized)`，连接不会订阅该频道。

### 聊天室最小示例（handler 伪代码）

`Hub` 用 `Arc`/`State<Hub>` 共享；在 `on_upgrade` 里注册连接、起一个泵把 `Outbound` 写回 socket，收到文本就广播到房间。连接 handle 掉落即自动退出所有频道并发 `Leave`。

```rust
use phoenix::prelude::*;

async fn chat_room(ws: WebSocketUpgrade, State(hub): State<Hub>) -> Response {
    ws.allowed_origin("https://app.example")
        .on_upgrade(move |mut socket| async move {
            // 注册连接（identity 用于 presence 与鉴权）；掉落 conn 自动清理。
            let (conn, mut outbound) = hub.connect_as(ConnectionMeta::new().with_key("user:7"));
            if conn.join("room:42").is_err() {
                return; // 鉴权拒绝
            }

            loop {
                tokio::select! {
                    // 出站泵：Hub -> socket。None = 被驱逐/关闭。
                    out = outbound.recv() => match out {
                        Some(Outgoing::Message(msg)) => {
                            if socket.send(msg).await.is_err() { break; }
                        }
                        Some(Outgoing::Presence(ev)) => {
                            // 应用自选线格式，这里示意 JSON 文本
                            let line = format!("{:?} {}", ev.kind, ev.member.key);
                            if socket.send(Message::text(line)).await.is_err() { break; }
                        }
                        None => break,
                    },
                    // 入站：socket -> 广播到房间
                    incoming = socket.recv() => match incoming {
                        Some(Ok(msg)) if msg.is_text() => conn.broadcast("room:42", msg),
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    },
                }
            }
            // conn 在此掉落 -> 自动 leave("room:42") + 向其余成员发 Leave 事件
        })
}
```

定向发送用 `hub.send_to(connection_id, Message::text("..."))`（**仅本实例**）或 `hub.send_to_key("user:42", …)`（按身份，**跨实例**，见下节）；在线快照用 `hub.presence("room:42")`。

## 跨实例广播（Redis pub/sub）

`Broadcaster` trait 是跨实例复制的接缝：`publish(&PeerFrame)` 把本地消息转发给其它实例；`subscribe() -> Option<PeerStream>` 拉回其它实例发布的帧。`Hub` **总是**先本地 fan-out，再 `publish` 给 peers；若 `subscribe()` 返回 `Some`，`Hub` 会起一个入站泵把 peer 帧本地 fan-out（不再回投，避免环路）。`PeerFrame` 携带 `origin: HubId`，各实例据此跳过自己的回声。

两种目标跨越这道接缝（`PeerTarget`）：

| 目标 | 触发 | 语义 |
| --- | --- | --- |
| `Channel(name)` | `hub.broadcast(channel, msg)` | 发给该频道的所有成员 |
| `Key(key)` | `hub.send_to_key(key, msg)` | 发给 `ConnectionMeta::key` 等于 `key` 的所有连接 |

**为什么定向发送按身份而不是连接 id**：`ConnectionId` 是**每个 Hub 自己的句柄**，在另一个节点上没有任何含义——「连接 7」在两台机器上是两个不相干的人。而一个用户经常在多台实例上有多条连接。因此 `send_to(id, …)` 保持纯本地，跨实例定向用 `send_to_key`（返回值是**本地**送达数，返回 0 不代表没送到，peer 可能仍会送达）。

`HubId` 不是简单自增：它混入了进程启动时间与 pid。两个进程都从 `1` 开始编号会让彼此的帧被当成自己的回声丢掉——这个坑必须在 id 生成处堵死。

### `RedisBroadcaster`（在 `phoenix-redis`）

```rust
use phoenix_http::Hub;
use phoenix_redis::RedisBroadcaster;

let bus = RedisBroadcaster::connect(&redis_url).await?;   // 可 .channel("my:bus") 改频道
// 必须在 Tokio runtime 内构造：会 spawn 入站泵
let hub = Hub::builder().broadcaster(bus).build();
```

- `publish` = Redis `PUBLISH`，`subscribe` = Redis `SUBSCRIBE` 流；默认频道 `phoenix:ws:broadcast`（`BROADCAST_CHANNEL`），不同频道 = 同一个 Redis 上互相隔离的集群。
- `publish` 是**同步签名**，实际 I/O 被 detach 到一个 task：Hub 的广播路径不会因为 Redis 卡住而阻塞。
- 断线自动重订阅（1s 退避）；Hub 掉落后接收端关闭，泵随之结束。
- **投递语义：Redis pub/sub 是 fire-and-forget**——无持久化、无 ack、无重放。实例断连期间发布的消息对它就是丢了。这对实时 fan-out 是对的取舍（迟到的聊天消息没有价值），对「绝不能丢」的东西是错的——那类用 `phoenix-queue`。
- **本地送达从不依赖 Redis**：Hub 先发本地连接再交给 broadcaster，所以 Redis 故障只会把集群降级成一组互不相通的单实例，而不是让实时功能整体失效。
- 线上格式是显式写死的 JSON（`{origin, target:{kind,…}, message:{type,…}}`，二进制用 base64），**不跟随内存类型重构**，滚动发布期间新旧实例才不会互相看不懂；解析失败的载荷跳过，不会打断泵。Ping/Pong/Close 是每条 socket 的存活信号，跨实例无意义，不按原样转发。

验收（需要真实 Redis）：

```bash
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test -p phoenix-redis --test broadcast_contracts
```

覆盖：频道广播跨实例送达且原实例不重复投递、按身份定向只到该身份、不同 Redis 频道互不可见、二进制载荷逐字节还原。
