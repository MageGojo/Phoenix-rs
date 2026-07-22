# 实时协议与流式请求

Phoenix 的实时能力分为流式请求、SSE 和 WebSocket。流式请求已经完成首版；SSE 与 WebSocket 仍在生产化实施中，不能仅凭底层 Hyper 支持视为已交付。

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

## SSE 与 WebSocket 状态

下一阶段会提供结构化 `Event`/`Sse`、keepalive、流完成观测，以及同时覆盖 HTTP/1.1、WSS 和 HTTP/2 extended CONNECT 的受控 WebSocket 门面。在这些 API 完成前，应用不应直接依赖 Phoenix 内部 Hyper upgrade 类型。
