# 认证、授权与 Token 生命周期

Phoenix 把认证、授权和 token 生命周期拆成独立边界：认证证明请求主体是谁，授权决定主体是否能执行某项操作，`TokenService` 管理 access/refresh token 的签发、轮换和撤销。应用开发者通常通过顶层 `phoenix::prelude` 使用这些 API。

## RBAC 角色与权限

权限是精确匹配的 ASCII capability，不支持隐式通配符。角色图在启动时编译；重复角色、缺失父角色和继承环都会使启动失败。

```rust
use phoenix::prelude::*;

let rbac = Rbac::build([
    Role::new("writer")?
        .allow("posts.read")?
        .allow("posts.update")?,
    Role::new("admin")?
        .inherits("writer")?
        .allow("posts.delete")?,
])?;

let principal = Principal::new("USER_1").role("admin");
let engine = AuthorizationEngine::new(rbac);
engine.authorize(
    &principal,
    &Permission::new("posts.delete")?,
    &(),
)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

主体可以拥有直接 allow/deny。显式 deny 总是优先于角色、直接 allow 和 ABAC allow；没有规则允许时默认拒绝。

## ABAC 资源策略

ABAC policy 接收主体、精确权限和业务资源。策略返回 `Allow`、`Deny` 或 `Abstain`；任意 `Deny` 拒绝请求，否则至少一个 RBAC/ABAC `Allow` 才会通过。

```rust
use phoenix::prelude::*;

struct Document {
    owner: String,
    restricted: bool,
}

let policy = policy_fn(
    |request: &AuthorizationRequest<'_, Document>| {
        if request.resource.restricted
            && request.principal.attribute_value("clearance")
                != Some(&serde_json::json!("restricted"))
        {
            AuthorizationDecision::Deny
        } else if request.resource.owner == request.principal.subject() {
            AuthorizationDecision::Allow
        } else {
            AuthorizationDecision::Abstain
        }
    },
);
```

资源相关授权应在加载资源并确认租户边界后调用 `AuthorizationEngine<Resource>::authorize`。不要仅依赖客户端提交的 owner、tenant 或 role 字段。

## HTTP 中间件组合

`JwtAuth<T>` 适合只校验签名和标准 claims 的无状态 API。需要注销、refresh rotation 或主动撤销时使用 `StatefulJwtAuth<T, Store>`。认证中间件必须位于 principal 映射和权限检查外层：

```rust
use std::{sync::Arc, time::Duration};
use phoenix::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
struct AccessClaims {
    roles: Vec<String>,
}

let jwt = JwtManager::new(
    JwtKey::new("KEY_1", std::env::var("APP_JWT_KEY")?.as_bytes())?,
    JwtConfig::new(Duration::from_mins(15))
        .issuer("APP_A")
        .audience("APP_A_API"),
)?;
let tokens = Arc::new(TokenService::new(
    jwt,
    Arc::new(MemoryTokenStore::new()),
    Duration::from_hours(24 * 30),
)?);
let authorizer = Arc::new(AuthorizationEngine::new(Rbac::build([
    Role::new("admin")?.allow("admin.open")?,
])?));

let routes = Routes::new()
    .get("/admin", |_request: Request| async { "admin" })
    .with_middleware(StatefulJwtAuth::<AccessClaims, _>::new(tokens))
    .with_middleware(PrincipalFromJwt::new(
        |claims: &JwtClaims<AccessClaims>| {
            claims.custom.roles.iter().fold(
                Principal::new(&claims.sub),
                |principal, role| principal.role(role),
            )
        },
    ))
    .with_middleware(RequirePermission::new(
        authorizer,
        Permission::new("admin.open")?,
    ));
# Ok::<(), Box<dyn std::error::Error>>(())
```

缺失或无效认证返回 401 和 `WWW-Authenticate: Bearer`；身份有效但权限不足返回 403。`RequirePermission` 用于不依赖资源的权限；资源授权应在控制器中调用带资源类型的 engine。

## Refresh rotation 与撤销

`TokenService::issue` 返回短时 access token 和 256-bit opaque refresh token。持久化层只保存 refresh token 的 SHA-256 hash，不保存明文 token。每个 access token 都有唯一 `jti`；由 refresh family 签发的 access token还带 `sid`。

```rust
let pair = tokens.issue("USER_1", AccessClaims {
    roles: vec!["admin".to_owned()],
}).await?;

let rotated = tokens.refresh(&pair.refresh_token).await?;
tokens.revoke_access(&rotated.access_token).await?;
```

refresh token 只能成功轮换一次。已经轮换过的 token 再次出现时视为泄露，整个 family 会被撤销，family 中的 access token 也会被 `StatefulJwtAuth` 拒绝。refresh TTL 不能短于 access TTL，以保证 family 撤销记录覆盖仍可能有效的 access token。

应用应把 refresh token 放在受保护的 HttpOnly Secure Cookie 或等价的安全客户端存储中；不得放进 URL、日志、页面 props 或错误消息。修改密码、账号冻结、权限降级和主动“退出所有设备”时应撤销相关 family。

## TokenStore 选择

- `MemoryTokenStore`：单进程开发和测试；进程重启后状态丢失。
- `FileTokenStore`：单进程、低吞吐部署的持久 JSON 状态；同目录临时文件、同步落盘和原子替换，Unix 下新文件权限为 `0600`。
- 自定义 `TokenStore`：多实例生产环境必须使用提供原子 rotation/reuse detection 的共享数据库或缓存适配器。

`FileTokenStore` 不协调多个进程，也会执行同步文件 I/O，因此不能作为多实例或高吞吐 token backend。自定义实现必须把 `rotate_refresh` 作为单个原子操作；“先读后写”的分布式实现会让同一 refresh token 并发成功两次。

过期记录通过 `TokenService::purge_expired()` 清理。生产应用应定期运行清理任务，并监控 store 错误；store 不可用时认证失败关闭，不能退化为忽略撤销状态。

## 审计与隐私

`AuthorizationAudit` 接收主体别名、权限、最终 decision 和通用 reason。审计实现不应记录 JWT、refresh token、资源正文、Header、Cookie 或敏感主体属性。面向指标的标签不要直接使用 subject 或资源 ID，以免形成高基数数据。
