# 实时协议与流式请求

Phoenix 的实时能力分为流式请求、SSE 和 WebSocket。流式请求与 SSE 首版已交付；WebSocket 首版仅承诺 HTTP/1.1 Upgrade 与 TLS 下的 WSS。

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
