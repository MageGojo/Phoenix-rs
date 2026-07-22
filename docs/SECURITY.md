# 安全与数据传输

## 1. 安全目标

Phoenix 的默认 Web 栈应降低常见网站漏洞的发生概率，并且不对浏览器端数据保密能力作虚假承诺。安全设计覆盖传输、会话、请求伪造、序列化、上传、错误和依赖治理。

## 2. 信任边界

- 浏览器、请求头、Cookie、URL、表单、JSON 和上传文件都不可信。
- React props 到达浏览器后对当前用户可见，即使它们曾被应用层加密。
- 反向代理传来的协议、主机和客户端 IP 只有在代理被显式信任时才可信。
- 数据库内容不自动等于安全 HTML；React 默认转义不覆盖 `dangerouslySetInnerHTML` 等显式绕过。

## 3. P0 安全默认值

- 生产文档要求 HTTPS；可选中间件将可信 HTTP 请求重定向到 HTTPS。
- 会话 Cookie 默认 `HttpOnly`、`Secure`、合理的 `SameSite` 和受限 Path。
- 登录与权限变化后轮换会话 ID。
- 所有改变状态的浏览器会话请求执行 CSRF 校验。
- HTML、JSON 与脚本上下文使用对应的结构化编码器，不做字符串拼接转义。
- 页面 props 通过显式 Resource/ViewModel 暴露，模型不默认序列化全部字段。
- 输入契约中 `#[sensitive]` 字段的用户值默认禁止进入旧输入、日志、页面 Props、SSR HTML 和 hydration 数据；字段名仍可用于生成类型和空表单控件。
- 请求 body、表单字段与上传大小有默认上限和可配置硬上限。
- 上传文件名不直接作为磁盘路径，内容类型不只信任客户端声明。
- 默认发送 CSP 基线、`X-Content-Type-Options`、Referrer Policy 等安全头；CSP 与 Vite 开发模式分别配置。
- 错误响应不包含密钥、数据库 URL、SQL 参数、环境变量、绝对路径或堆栈。
- 密钥使用操作系统随机源生成，生产环境拒绝示例密钥和过短密钥。

## 3.1 当前已实现的基础防护

- Hyper 服务在读取完整 body 前应用可配置大小上限，超限返回 413。
- HTTP/1.1 请求头有可配置读取超时，降低慢速连接长期占用任务的风险。
- 请求 body 有独立读取超时，慢速 body 到期返回 408。
- 优雅关闭有硬超时，到期后中止仍未结束的连接任务。
- `Request::json()` 只接受 `application/json` 与 `application/*+json`；错误 MIME 返回 415，语法错误返回不含解析细节的 400。
- 路径参数执行严格百分号与 UTF-8 解码，非法编码返回 400，不使用有损替换字符。
- 业务 Handler 和路由中间件受到 panic boundary 保护，客户端只收到通用 500，后续请求仍可服务。
- release profile 固定使用 unwind，使 panic boundary 不会因 `panic=abort` 失效；业务代码仍不得把密钥或用户数据写入 panic 消息。
- `SecurityHeaders` 案例中间件设置 `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY` 和严格 Referrer Policy，并且不覆盖应用显式设置的值。
- 命名 URL 对动态路径段执行 RFC 3986 百分号编码，避免参数改变路径结构。
- `phoenix-security` 提供服务端 Session；浏览器只保存 256-bit 随机会话 ID，默认 Cookie 使用 `Secure`、`HttpOnly`、`SameSite=Lax`、受限 Path 与两小时 Max-Age。
- Session 支持保留数据的 ID 轮换和清空数据的注销轮换；内存存储按 TTL 清理过期记录。
- CSRF token 存在服务端 Session，GET/HEAD/OPTIONS 自动准备 token，其他方法必须提供匹配的 `X-CSRF-Token`。
- CORS 使用精确 Origin、Method 和 Header allowlist，预检与实际跨域方法均失败关闭，不反射未授权 Origin。
- 限流优先使用可信代理解析后的客户端 IP，缺失时回退 TCP peer；嵌入式请求没有地址时进入统一保守 bucket。
- 可信代理只在直连 peer 位于显式 allowlist 时解析 `X-Forwarded-For`，并从右到左移除可信 hop；非可信直连方不能伪造客户端 IP。
- Host allowlist 在进入业务 handler 前规范化并校验 HTTP authority；缺失、非法或未授权 Host 返回 400。
- `SecurityPolicy` 提供 CSP、HSTS、Permissions Policy、nosniff、frame deny 与 Referrer Policy，且不覆盖应用显式响应头。
- request ID 使用独立 128-bit 随机值并同时写入 Request extensions 与响应头；访问日志只记录 method、无 query 的 path、status、耗时、request ID 和客户端 IP。
- 日志诊断辅助函数默认脱敏 Authorization、Cookie、Set-Cookie、代理认证、API key 和 CSRF token；标准访问日志不记录任何请求头值或 query string。
- `phoenix-auth` 提供默认拒绝的 RBAC/ABAC、角色继承、deny-overrides、资源 policy、JWT principal 映射、401/403 中间件和不包含 token/资源正文的授权审计事件。
- `TokenService` 提供一次性 refresh rotation、并发 reuse detection、access/family 撤销、`jti`/`sid` 校验、过期清理，以及仅持久化 refresh hash 的内存/文件 store。

这些能力由 `examples/blog/tests/` 中的真实 TCP 与进程内测试覆盖。

推荐的外到内顺序是：`TrustedProxies` → `RequestId` → `AccessLog` → `HostAllowlist` → `Cors` → `RateLimit` → `SessionMiddleware` → `Csrf` → `SecurityPolicy` → 业务 Handler。`Routes::with_middleware` 的首次注册层位于最外侧；改变 Session/CSRF 或 request ID/日志的相对顺序会破坏相应上下文。

## 3.2 在应用中启用安全栈

`phoenix::prelude` 已重导出全部安全中间件。下面的顺序与请求生命周期一致：

```rust
use std::{
    net::IpAddr,
    time::Duration,
};

use phoenix::prelude::*;

let session_store = SessionStore::memory(Duration::from_hours(2));

let cors = Cors::new(CorsConfig {
    allowed_origins: ["https://app.example.test".to_owned()]
        .into_iter()
        .collect(),
    allowed_methods: [Method::GET, Method::HEAD, Method::POST]
        .into_iter()
        .collect(),
    allowed_headers: [phoenix::http::header::CONTENT_TYPE]
        .into_iter()
        .collect(),
    allow_credentials: true,
    max_age: Duration::from_mins(10),
});

let routes = phoenix::mount_routes!()
    .with_middleware(TrustedProxies::new([
        "127.0.0.1".parse::<IpAddr>().expect("valid proxy IP"),
    ]))
    .with_middleware(RequestId)
    .with_middleware(AccessLog)
    .with_middleware(HostAllowlist::new([
        "app.example.test",
        "localhost",
    ]))
    .with_middleware(cors)
    .with_middleware(RateLimit::new(RateLimitConfig {
        requests: 120,
        window: Duration::from_mins(1),
    }))
    .with_middleware(SessionMiddleware::new(
        session_store,
        SessionConfig::default(),
    ))
    .with_middleware(Csrf)
    .with_middleware(SecurityPolicy::default());
```

示例域名必须替换为实际部署域名。只把真正终止连接、且会重写转发头的代理 IP 放入 `TrustedProxies`；不要信任整个公网网段，也不要仅因为请求带有 `X-Forwarded-For` 就信任它。

开发环境通过明文 HTTP 访问时，默认 `Secure` Cookie 不会被浏览器回传。仅在本机开发配置中关闭：

```rust
let session_config = SessionConfig {
    secure: false,
    ..SessionConfig::default()
};
```

生产环境必须保留 `secure: true`。`SameSite=None` 会强制附加 `Secure`，跨站 Cookie 同时需要精确 CORS allowlist 和 `allow_credentials: true`，不能使用通配 Origin。

### 在控制器中使用 Session 和 request ID

```rust
pub async fn login(request: Request) -> Response {
    let Some(session) = request.extensions().get::<Session>().cloned() else {
        return Response::text("Session unavailable")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR);
    };

    // 验证凭证成功后再写入，并轮换公开 ID 防止 session fixation。
    session.put("user_id", 42);
    session.regenerate();

    let request_id = request
        .extensions()
        .get::<RequestIdValue>()
        .map(|value| value.0.as_str())
        .unwrap_or("missing");

    Response::text(format!("logged in; request={request_id}"))
}

pub async fn logout(request: Request) -> Response {
    if let Some(session) = request.extensions().get::<Session>() {
        session.invalidate();
    }
    Response::text("logged out")
}
```

`regenerate()` 保留现有数据并更换 ID；`invalidate()` 清空数据并更换 ID。登录、权限提升和账户切换后使用前者，注销时使用后者。

### 浏览器提交 CSRF token

安全方法会在服务端 Session 中准备 token。页面控制器通过 `Session::csrf_token()` 读取并交给 `Page::csrf_token(...)` 后，`@phoenix/react` 的 `callRust` 与生成命名 action 会自动用 `X-CSRF-Token` 原样回传：

```ts
await account.update({ display_name: "Ada" });
```

CSRF 依赖 `SessionMiddleware`，所以 Session 必须注册在 Csrf 外层。API token 客户端若不使用浏览器 Session，应放在独立路由组并采用明确的认证策略，而不是全局关闭 CSRF。

### CSP、HSTS 与日志

默认 `SecurityPolicy` 适合不加载第三方脚本的生产页面。Vite 开发服务或第三方资源需要按环境显式调整 CSP：

```rust
let policy = SecurityPolicy {
    content_security_policy:
        "default-src 'self'; script-src 'self'; style-src 'self'; \
         base-uri 'self'; frame-ancestors 'none'; object-src 'none'"
            .to_owned(),
    hsts: Some(Duration::from_hours(8760)),
    hsts_include_subdomains: true,
};
```

只有确认所有子域都支持 HTTPS 后才启用 `includeSubDomains`。HSTS 响应本身不能替代 TLS 终止。

`AccessLog` 不记录 query 或 Header 值。业务确需记录诊断 Header 时必须先调用 `phoenix::security::redact_headers()`；禁止直接用 `Debug` 输出完整 `Request`、Session 或环境变量。

内置 Session store 和限流 bucket 都是单进程内存实现。多实例部署必须接入共享后端或在可信网关层完成相应能力，否则不同实例之间不会共享状态。

分布式限流使用 `RateLimit::with_backend` 接入实现了原子 `hit` 的共享 backend。默认 key 只使用可信代理解析后的客户端 IP；应用可通过 `RateLimitKey` 提供租户/API key 等有界 key，但不得把原始 URL、query 或 token 作为 key。backend 故障默认返回 503；只有明确接受绕过风险的非关键路由才可选择 `RateLimitFailureMode::Open`。`MemoryRateLimitBackend` 可用于本地和双实例契约测试，不是跨进程存储。

### JWT、Bearer API 与密码

`phoenix-crypto` 的 JWT 首版固定 HS256，并把算法 allowlist、`kid`、过期时间、not-before、issuer 和 audience 当作同一个验证边界：

```rust
use std::{sync::Arc, time::Duration};
use phoenix::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
struct ApiClaims {
    role: String,
}

let manager = JwtManager::new(
    JwtKey::new("KEY_2026_07", std::env::var("APP_JWT_KEY")?.as_bytes())?,
    JwtConfig::new(Duration::from_mins(15))
        .issuer("APP_A")
        .audience("APP_A_API"),
)?;

let routes = Routes::new()
    .get("/api/me", typed(|claims: Jwt<ApiClaims>| async move {
        format!("{}:{}", claims.sub, claims.custom.role)
    }))
    .with_middleware(JwtAuth::<ApiClaims>::new(Arc::new(manager)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

新 key 用于签发，旧 key 只通过 `.with_verification_key(old_key)` 保留验证窗口；轮换窗口结束后删除旧 key。JWT payload 不是密文，禁止放入密码、密钥、支付数据或只应服务端可见的字段。Authorization Header 和完整 token 永远不得进入日志。

需要 refresh、主动注销或权限变更后撤销时，改用 `TokenService` 和 `StatefulJwtAuth`；RBAC/ABAC、principal 映射、refresh rotation、reuse detection 与 store 选择见[认证、授权与 Token 生命周期](AUTHORIZATION.md)。多实例部署必须实现共享且原子的 `TokenStore`，不能用本地文件状态假装分布式撤销。

密码使用 `Password::hash()` 生成 Argon2id PHC string，以 `Password::verify()` 验证。数据库只保存 PHC string；登录成功后如参数策略升级，可重新 hash 并替换旧值。

### 应用数据认证加密

`Encryptor` 使用 AES-256-GCM。调用方必须提供稳定且不可混用的关联数据，例如 `APP_A|users|7|email`；同一份密文用不同上下文解密会认证失败。密文 envelope 含版本、算法、`key_id`、随机 nonce 与含 tag 的 ciphertext，但不保存关联数据：

```rust
let key = EncryptionKey::new("KEY_2026_07", [7_u8; 32])?;
let encryptor = Encryptor::new(key);
let sealed = encryptor.seal(b"private value", b"APP_A|users|7|email")?;
let plaintext = encryptor.open(&sealed, b"APP_A|users|7|email")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

示例数组只用于说明类型，真实 key 必须由 secrets manager 或受保护的环境注入，不能硬编码或提交到 Git。

## 3.3 尚未实现的安全能力

- 分布式 `TokenStore`/`SessionStore` 生产适配器，以及多密钥 Cookie session；限流已有原子共享 backend contract，但内置 backend 仍只面向单进程/契约测试。
- CSP nonce 与 Vite 开发模式的自动策略切换；当前 CSP/HSTS 由应用按环境显式配置。
- Multipart 已在全局 body 上限内解析为内存字段；上传存储、文件名净化、静态文件路径与下载响应尚未实现。
- 依赖漏洞/许可证 CI 与正式发布前的独立安全评审。

因此当前基础层适合继续开发和评审，不应未经上述能力与独立安全审查直接暴露为生产网站。

## 4. 加密方案边界

### 传输加密

TLS 是浏览器与服务之间数据保密和完整性的主要机制。框架负责正确识别可信代理后的 HTTPS 状态并提供部署检查，证书签发和代理配置由部署环境负责。

### 会话

P0 推荐服务端会话：浏览器只保存高熵随机会话 ID，业务数据保存在服务端存储。若提供 Cookie 会话，必须使用带认证的加密（AEAD）、版本化密文格式、用途绑定、过期时间和密钥轮换。

### 可选安全信封

应用层信封只适合防止中间层读取、篡改或跨用途重放，不能阻止最终浏览器读取。格式至少包含：

```text
version | key_id | purpose | issued_at | expires_at | nonce | ciphertext | tag
```

实现前必须决定威胁模型并采用成熟密码学库。禁止自行设计算法、复用 nonce、使用无认证加密或把长期解密密钥嵌入前端。

当前页面协议已提供显式可选的 `Aes256GcmCodec`：使用操作系统随机 nonce、60 秒默认有效期、`page-navigation` 用途绑定，并认证版本、`key_id`、用途和时间元数据。浏览器端通过调用方提供的 `CryptoKey` 解密，Phoenix 不负责把密钥送到浏览器。普通配置保持明文 JSON 并依赖 TLS；初始 HTML 始终是可读 hydration 数据。

## 5. 数据分类

| 类型 | 是否可进入 React props | 处理要求 |
| --- | --- | --- |
| 页面公开数据 | 可以 | 正常序列化和转义 |
| 当前用户可见业务数据 | 可以 | Resource 白名单与权限检查 |
| 密码、密钥、访问令牌 | 不可以 | 仅服务端处理，日志也必须脱敏 |
| 内部模型字段 | 默认不可以 | 显式选择后才能暴露 |
| 一次性表单状态 | 可以但最小化 | 短时效，排除敏感字段 |

## 6. 验证计划

- CSRF：缺失、错误、跨会话、过期 token 均失败。
- 会话：固定攻击、轮换、注销、过期和 Cookie 属性测试。
- 页面协议：`</script>`、Unicode 分隔符和恶意页面名不会突破上下文。
- HTTP 基础层：超限/慢速 body、慢请求头、非法路径编码、错误 JSON MIME、panic 隔离和安全头测试。
- 契约：输入/输出方向隔离、Serde rename/flatten 冲突和敏感字段泄漏测试。
- SSR/Islands：恶意 props、hydration payload、CSP nonce、renderer 超时与任意模块加载测试。
- 路由与代理：Host Header、开放重定向、伪造转发头测试。
- 上传：路径穿越、双扩展名、超限、空文件和内容类型欺骗测试。
- 依赖：CI 中执行许可证和漏洞扫描，锁文件进入版本控制。

正式对外发布前需要独立安全评审。任何“默认安全”的声明都必须由自动测试支撑。
