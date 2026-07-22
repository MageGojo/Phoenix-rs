# TLS、HTTPS 与 ALPN

Phoenix 可以直接使用 rustls 终止 TLS，也可以部署在可信反向代理之后。两种模式都会生成统一的有效请求 scheme，供 HTTPS 重定向、HSTS、Cookie 和业务策略使用。

## 内置 TLS listener

```rust
use phoenix::prelude::*;

# async fn run(application: Application) -> Result<(), Box<dyn std::error::Error>> {
let tls = TlsConfig::from_files(
    "config/tls/certificate.pem",
    "config/tls/private-key.pem",
)?
.handshake_timeout(std::time::Duration::from_secs(5))?;

application
    .bind_tls("0.0.0.0:443", tls)
    .await?
    .run()
    .await?;
# Ok(())
# }
```

`TlsConfig` 默认提供 `h2` 和 `http/1.1` ALPN。应用的 `HttpProtocol` 会同步收紧 ALPN：

| HTTP 策略 | ALPN |
| --- | --- |
| `Auto` | `h2`, `http/1.1` |
| `Http1Only` | `http/1.1` |
| `Http2Only` | `h2` |

证书链和私钥在绑定 listener 前解析，缺失证书、缺失 key、key/certificate 不匹配和零握手超时都会启动失败。TLS 握手受独立 deadline 限制，不占用 HTTP Header/Body timeout。

高级部署可以构造 `rustls::ServerConfig` 后使用 `TlsConfig::from_server_config(...)`。应用仍应让 Phoenix 管理 HTTP ALPN；mTLS、自定义证书 resolver、会话恢复和密码套件策略由传入的 rustls 配置负责。

## 直接 TLS 请求元数据

框架在每个请求 extensions 中写入 `ConnectionInfo`：

```rust
async fn inspect(request: Request) -> String {
    let connection = request.extensions().get::<ConnectionInfo>();
    connection.map_or_else(
        || "embedded".to_owned(),
        |connection| format!(
            "{}:{}",
            connection.scheme().as_str(),
            connection.alpn_protocol().unwrap_or("none"),
        ),
    )
}
```

直接 TLS 连接的 scheme 是 `https`；明文连接是 `http`。`ConnectionInfo` 还包含直连 peer 和协商后的 ALPN，但不会包含证书私钥或敏感请求数据。

## 可信反向代理

代理终止 TLS 时，推荐顺序为：

```rust
let routes = routes
    .with_middleware(TrustedProxies::new(trusted_proxy_ips))
    .with_middleware(HostAllowlist::new(["APP_HOST"]))
    .with_middleware(HttpsRedirect::new("APP_HOST")?)
    .with_middleware(SecurityPolicy::default());
```

`TrustedProxies` 只有在直连 TCP peer 位于显式 allowlist 时才读取 `X-Forwarded-Proto`。未受信客户端自行发送 `X-Forwarded-Proto: https` 仍被视为 HTTP。

`HttpsRedirect` 使用配置期固定的 canonical authority，不反射请求 Host，避免开放重定向。默认返回 308；需要临时保留方法语义时使用 `.temporary()` 返回 307。路径与 query 会保留。

## HSTS 与 Cookie

`SecurityPolicy` 只在有效 scheme 为 HTTPS 时发送 `Strict-Transport-Security`。明文开发请求不会错误缓存 HSTS。Session Cookie 的 `Secure` 属性仍由 `SessionConfig` 控制：生产必须保持开启，本机明文开发才可显式关闭。

启用 `includeSubDomains` 前必须确认所有子域都能通过 HTTPS 正常服务。HSTS 不是证书管理、TLS redirect 或 ALPN 的替代品。

## 部署边界

- 证书与私钥必须来自仓库外的 secrets/volume，不提交 Git。
- Phoenix 不实现 ACME 签发或自动续期；证书生命周期由部署平台负责。
- 反向代理必须覆盖而不是追加来自公网的 forwarding headers。
- TLS listener 与代理终止两种模式都需要真实部署环境的证书续期、负载均衡和故障切换演练。
- 当前自动测试使用本地临时自签证书，不接触公网服务或生产 key。
