# Prometheus 指标

`phoenix-metrics` 提供进程内、无第三方 collector 依赖的低基数 registry，并输出 Prometheus 0.0.4 文本格式。应用使用同一个 `Metrics` 实例连接 HTTP middleware、内置 TCP/TLS server、React renderer、数据库适配层和后续 queue worker。

## Cargo feature

指标能力需要启用门面 feature **`metrics`**：

```toml
phoenix = { package = "phoenixrs", features = ["metrics"] }
```

未启用时 `Metrics` / `MetricsMiddleware` / `Application::metrics` 不会编进依赖图。

## 接入 HTTP 与服务端指标

```rust
use phoenix::prelude::*;

let metrics = Metrics::new();
let metrics_endpoint = metrics.clone();

let routes = Routes::new()
    .get("/internal/metrics", move |_request: Request| {
        let metrics = metrics_endpoint.clone();
        async move { metrics.response() }
    })
    .with_middleware(MetricsMiddleware::new(metrics.clone()));

let application = Application::new(routes)?
    .metrics(metrics.clone());
# Ok::<(), Box<dyn std::error::Error>>(())
```

`MetricsMiddleware` 记录 method、status class、活跃请求和固定 bucket 的请求耗时。`Application::metrics` 记录接受/活跃 TCP 连接与 TLS handshake success/failure。两者必须使用同一个 registry。

指标端点应放在独立内部监听器、管理应用或受网络策略保护的路由上；不要直接公开到互联网。不要在 `/metrics` 路由外层重复安装同一 registry 的 middleware，否则每次抓取也会增加应用请求统计。

## Renderer、数据库与队列

renderer health 是 point-in-time snapshot：

```rust
renderer.health().record_metrics(&metrics);
```

应用应在健康采集循环或抓取前刷新 snapshot。数据库适配器和 queue worker 使用固定枚举，不接受 SQL、表名、job payload 或用户输入作为 label：

```rust
metrics.record_database(DatabaseOutcome::Success);
metrics.record_job(JobOutcome::Retried);
```

分布式 Session 和限流 backend 分别调用 `record_session_conflict`、`record_session_store_error`、`record_rate_limit_rejection` 和 `record_rate_limit_store_error`。这些计数器不包含 session ID、客户端 IP 或 token。

## 已导出的指标

- `phoenix_http_requests_total{method,status_class}`
- `phoenix_http_active_requests`
- `phoenix_http_request_duration_seconds`
- `phoenix_connections_total`、`phoenix_connections_active`
- `phoenix_tls_handshakes_total{outcome}`
- `phoenix_renderer_*`
- `phoenix_database_operations_total{outcome}`
- `phoenix_queue_jobs_total{outcome}`
- Session/限流 conflict、store error 和 rejection counters

HTTP method 会归一化到固定集合，非标准 method 进入 `OTHER`；状态仅按 `1xx` 到 `5xx` 和 `other` 分类。框架不提供任意 label API，避免 token、query、Host、路由参数、用户 ID 或错误正文进入指标。

registry 是单进程原子计数器。多实例部署由 Prometheus 分别抓取每个实例并在查询层聚合，不能把进程内 gauge 写入共享数据库后当作全局瞬时值。
