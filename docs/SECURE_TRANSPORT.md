# 一键加密传输（Secure Transport）

> 前后端「一键」协商每会话密钥，把页面协议**响应体**封成二进制帧下发，并把
> 页面协议**请求体**同样封帧上行。本文覆盖：诚实的安全边界、线上协议（帧格式 /
> 方向绑定 / 握手时序）、一键启用、配置项、TTL、与 SSR 首屏的边界、Nonce 唯一性、
> 密钥轮换与过期、以及明确「不做」的部分。

## 0. 诚实的安全边界（先读这段）

**这不是端到端加密，也不能对终端用户隐藏内容。** 浏览器前端为了渲染页面，
**必须**能解密收到的数据；因此任何坐在浏览器里的人（或其扩展、DevTools）终究能
看到明文。本机制是 **TLS 之上的一层纵深防御（defense-in-depth）**，它买到的是：

- **密钥不进 bundle**：会话密钥经每次页面会话的 **ECDH 现场协商**得到，不写死在
  JS 里、不随构建产物分发。
- **客户端密钥不可导出**：派生出的 AES 密钥在浏览器侧以 `extractable: false` 的
  `CryptoKey` 形式存在，无法通过 `exportKey` 从活动页面里 dump 出来。
- **抬高被动抓取成本**：对被动抓包、反代/网关日志、误配置的缓存、以及只会
  «GET 一下 JSON 就走» 的爬虫来说，页面响应体不再是可直接读取的明文 JSON。

它**不替代 HTTPS**，也不防御能在客户端执行代码 / 读内存的主动攻击者。请始终在
TLS 上启用它。**SSR 首屏 HTML 不走此加密**（见 §6）。

## 1. 帧格式（`PHX1`）

加密体（**两个方向共用同一格式**）是一段紧凑的二进制帧（大端序），`Content-Type:
application/vnd.phoenix.secure`，头 `x-phoenix-encrypted: 1`：

```text
偏移  字段        长度  说明
[0]   magic       4     "PHX1" = 0x50 0x48 0x58 0x31
[4]   version     1     0x01
[5]   issued_at   8     u64 be，签发 Unix 秒
[13]  expires_at  8     u64 be，过期 Unix 秒
[21]  nonce       12    AES-GCM 随机 nonce（每帧唯一）
[33]  ciphertext || gcm_tag(16)   其余，AES-256-GCM 密文 + 16 字节认证标签
```

- **AAD**（附加认证数据）= 帧头 `frame[0..21]`（21 字节）**++** `UTF8(key_id)`
  **++** 方向标签。magic / version / issued_at / expires_at 因此都被 GCM 标签认证，
  篡改必败。
- **方向标签**：请求方向 `"req"`，响应方向 `"res"`（`FrameDirection`）。
  这一位不是装饰：**没有它，同一会话密钥下的请求帧与响应帧就是可互换的密文**，
  攻击者能把抓到的响应帧原样当成请求体回放并通过认证。绑定方向后，跨方向回放
  直接标签校验失败（Rust 与 TS 两侧都有针对这一条的用例）。
- **明文**：响应方向 = 页面信封 JSON 字节，与非加密时 `Content-Type:
  application/vnd.phoenix.page+json` 的响应体**逐字节相同**；请求方向 = 原本要发的
  请求体字节，其自身的 `Content-Type` 放在 `X-Phoenix-Content-Type` 头里随行，
  服务端解帧后原样还原（缺省 `application/json`）。

服务端实现见 `phoenix_crypto::seal_frame` / `open_frame`，以及
`phoenix_view::SecureCodec`（`encode_frame` / `decode_frame`）。客户端实现见
`packages/phoenix-react/src/secure.ts` 的 `decryptFrame` / `sealRequest`。

## 2. 握手时序

```
浏览器                                                     服务端
  │  1. 生成 ECDH-P256 临时密钥对（私钥不可导出）
  │
  │  POST /__phoenix/secure/handshake   (JSON over TLS)
  │  { v:1, kex:"ECDH-P256", hkdf:"HKDF-SHA256", aead:"A256GCM",
  │    client_public_key:"<b64url raw 65B, 0x04||X32||Y32>" }
  │ ─────────────────────────────────────────────────────►
  │                                    2. 生成服务端临时密钥对
  │                                    3. ikm = ECDH_P256(client_pub, server_priv)
  │                                            的共享 x 坐标(32B)
  │                                    4. session_key =
  │                                       HKDF-SHA256(ikm,
  │                                          salt = UTF8(key_id),
  │                                          info = "phoenix.secure.session.v1")
  │                                    5. 存储 key_id -> (session_key, expires_at)
  │  { v:1, key_id, server_public_key:"<b64url raw 65B>",
  │    expires_at, ttl }
  │ ◄─────────────────────────────────────────────────────
  │  6. 用自己的私钥 + server_public_key 复算同一 session_key
  │
  │  后续页面导航请求带上：
  │  X-Phoenix-Page: 1   (拿页面协议响应)
  │  X-Phoenix-Secure: 1
  │  X-Phoenix-Key: <key_id>
  │ ─────────────────────────────────────────────────────►
  │                                    7. 命中有效会话 → 响应体封成 PHX1 帧
  │  Content-Type: application/vnd.phoenix.secure
  │  x-phoenix-encrypted: 1   [二进制帧]
  │ ◄─────────────────────────────────────────────────────
  │  8. decryptFrame → PageEnvelope
```

**密钥派生两端必须一致**：`ikm` 取 ECDH-P256 的共享 **x 坐标 32 字节**（ring 的
`agreement::agree_ephemeral` 回调收到的即是该 32 字节）；HKDF 的 **salt 是
`key_id` 的 UTF-8 字节**（即握手响应里那串 base64url 字符串本身），**info** 固定为
`"phoenix.secure.session.v1"`，输出 32 字节作为 AES-256-GCM 密钥。

握手路由**不挂 CSRF**：它在建立加密之前、是幂等协商，且此时尚无会话。防滥用采用
请求体大小上限（默认 2048 字节，`max_handshake_body`）与会话存储容量上限
（默认 100k，`max_sessions`，超限按最早过期者淘汰）。

## 3. 一键启用

默认**关闭**。应用侧一行接入（`phoenix-runtime`）：

```rust
use phoenix_runtime::{secure_transport, Application, SecureTransportConfig};

// app_routes 是你已有的 Routes
let routes = secure_transport(app_routes, SecureTransportConfig::default());
let app = Application::new(routes)?;
```

`secure_transport(routes, config)` 做两件事：

1. 注册握手路由 `POST /__phoenix/secure/handshake`（命名 `phoenix.secure.handshake`）；
2. 以全局中间件 `SecureTransport::layer()` 包裹全部路由，在**响应阶段**对满足条件的
   页面协议响应加密。

> **CSRF 边界**：把 `secure_transport(...)` 作用在**不处于 CSRF 分组**里的
> `Routes` 上（Phoenix 的 CSRF 通常经 `RouteGroup` 局部挂载）。这样握手路由天然
> 免除 CSRF。

也可手动接线（等价、更细粒度）：

```rust
use phoenix_crypto::{SecureTransport, SecureTransportConfig};

let transport = SecureTransport::new(SecureTransportConfig::default());
let routes = app_routes
    .post(transport.handshake_path().to_owned(), transport.handshake_handler())
    .name("phoenix.secure.handshake")
    .with_middleware(transport.layer());
```

## 4. 加密注入点（架构说明）

加密在**中间件的响应阶段**完成，噪音最小、覆盖所有 handler、无需改动任何业务
controller：

- `SecureTransport::layer()` 是一个 `Middleware`。请求进入时读取
  `X-Phoenix-Secure` / `X-Phoenix-Key`，查会话密钥；`next.run` 之后，若响应是
  **明文页面协议响应**（`content-type: application/vnd.phoenix.page+json` 且
  `x-phoenix-encrypted: 0`、非流式），就把响应体重新封成 `PHX1` 帧并改写
  content-type / 加密标记，其余头（`cache-control`、`vary`）保留。其它响应（文档
  HTML、已是 JSON 加密路径、流式、非页面）**原样透传**。

**为什么密码学 + 握手 + 中间件都落在 `phoenix-crypto`，而不是 `phoenix-http`？**
依赖方向是 `phoenix-crypto → phoenix-http`（crypto 依赖 http），因此 http **不能**
反过来依赖 crypto，否则成环。`phoenix-crypto` 是唯一同时能看到
`phoenix-http` 的 `Handler`/`Middleware`/`Request`/`Response` **与** `ring`/`aes-gcm`
的层，所以握手 `Handler`、会话 store、加密 `Middleware` 都放这里，避免把 ECDH 逻辑
在 http 里重复实现。`phoenix-http` 只承载共享的线上常量
（`SECURE_HANDSHAKE_PATH` / `SECURE_REQUEST_HEADER` / `SECURE_KEY_HEADER` /
`SECURE_ENCRYPTED_HEADER` / `SECURE_CONTENT_TYPE` / `PAGE_PROTOCOL_MEDIA_TYPE`），
让客户端与服务端两半逐字节对齐。`phoenix-view` 通过 `SecureCodec` 复用
`phoenix-crypto` 的帧实现，并提供构造期加密的 `Page::respond_secure`（单一实现，
无重复漂移）。`phoenix-runtime` 只提供 `secure_transport(...)` 一键接线。

> 除了中间件路径，`phoenix-view::Page::respond_secure(page_request, Some(&secure), codec)`
> 允许单个 handler 在**构造期**直接产出加密帧（三选一：二进制帧 / 旧 JSON 加密 /
> 明文）。上线默认走中间件，此方法留给需要精细控制的场景。

## 5. 配置项（`SecureTransportConfig`）

| 字段 | 默认 | 含义 |
| --- | --- | --- |
| `session_ttl` | 5 分钟 | 协商出的会话密钥有效期；过期后需重新握手 |
| `frame_ttl` | 1 分钟 | 单个响应帧的有效期（写入 `expires_at`） |
| `handshake_path` | `/__phoenix/secure/handshake` | 握手路由路径 |
| `max_handshake_body` | 2048 字节 | 握手请求体大小上限（防滥用） |
| `max_sessions` | 100_000 | 进程内会话上限，超限淘汰最早过期者 |
| `max_request_frame` | 1 MiB | 加密**请求**帧大小上限，超限在解密前就返回 413 |

会话过期在每次握手时惰性清理（`prune_expired`）；查表命中过期条目视为未命中。

## 5.1 请求体加密（上行）

握手完成后，页面协议的 mutation（`submitPage` / 生成的 action）会自动把 JSON
请求体封成 `PHX1` 帧上行：

```text
Content-Type: application/vnd.phoenix.secure
X-Phoenix-Encrypted: 1
X-Phoenix-Secure: 1
X-Phoenix-Key: <key_id>
X-Phoenix-Content-Type: application/json     # 明文本来的类型
<body> = PHX1 帧（方向 "req"）
```

服务端中间件在 **handler 之前**解帧，把明文写回请求体、还原 `Content-Type`、
清掉帧标记，因此 `Json<T>` / `Validated<T>` 等提取器**完全无感**。

**失败一律关闭（fail closed）**，绝不把密文或空体交给 handler：

| 情况 | 结果 |
| --- | --- |
| 未标记为加密帧 | 原样放行，**与未启用本特性逐字节相同** |
| 帧被篡改 / 换密钥 / 换方向 / 截断 / 根本不是帧 | `400` |
| 帧已过期 | `400` |
| 没有可用会话（未握手 / key_id 未知 / 已过期） | `400` |
| 帧超过 `max_request_frame` | `413`（**解密之前**就拒） |

错误体只有一句通用文案，不区分「验签失败」和「过期」之外的细节，避免变成预言机。

客户端侧：`session.sealRequest(body, contentType)` 在会话过期时返回 `null`，
调用方**回退明文**而不是发一个服务端注定打不开的帧。

## 6. 与 SSR 首屏的边界

**首屏文档 HTML 永远不加密。** 浏览器必须能直接解析首个 HTML 文档（含水合数据），
所以文档响应始终是可读 HTML，仅受 TLS 保护。加密只作用于**软导航**的页面协议响应
（`X-Phoenix-Page: 1`）。因此：

- SPA 首屏、老客户端、未完成握手的请求 → 自动回退明文页面协议，**逐字节等同**未启用
  本特性时的行为。
- 只有「已握手 + 带 secure 头 + 走页面协议」的请求才会拿到二进制帧。

## 7. Nonce 唯一性

每一帧的 AES-GCM nonce 都是 `OsRng` 现取的 12 字节随机值，**绝不复用**，且 nonce
明文写入帧的 `[21..33]`、同时作为 AEAD 的 IV。GCM 在同一密钥下重复 nonce 会灾难性
破坏机密性，故此处严格「一帧一随机 nonce」。帧头（含 issued_at/expires_at）纳入 AAD，
任何对 nonce 或帧头的篡改都会导致标签校验失败。

## 8. 密钥轮换与过期

- **每会话一密钥**：每次握手都生成全新的服务端临时密钥对与新 `key_id`，天然轮换。
- **双层过期**：会话层（`session_ttl`）+ 帧层（`frame_ttl`）。客户端 `decryptFrame`
  会先检查帧 `expires_at`，服务端 `open_frame` 同样校验；会话到期后握手需重来。
- **无长期密钥**：不存在需要人工轮换的静态传输密钥；进程重启即清空所有会话。

## 9. 已知不做

- **文件上传不加密**：带 `File` 的表单走 multipart + XHR（为了进度条），不经过本
  机制；这类请求仍只由 TLS 保护。
- **不覆盖非页面协议响应**：静态资源、SSE/WS 不在本机制内。
- **不做主动攻击者防护**：见 §0，无法对能在客户端执行代码者隐藏内容。
- **无跨进程会话共享**：会话表在进程内存里，多实例部署下客户端必须与握手时同一个
  实例通话（或按 `key_id` 做粘性路由）。

## 10. 测试与实现索引

- `crates/phoenix-crypto/src/secure.rs`：ECDH-P256 + HKDF 派生、`seal_frame` /
  `open_frame`、`SecureTransport` store、握手 `Handler`、加密 `Middleware`。
  单测覆盖：两端 ECDH 自洽、帧字节布局逐偏移断言、seal→open 回环、篡改
  密文/帧头/nonce/错误密钥/错误 key_id/过期/截断**必败**、独立「客户端」互操作、
  经 Router 的握手→加密页面端到端、错误参数握手回 400；请求方向另有：解密后
  handler 看到明文与还原的 `Content-Type`、未加密请求逐字节原样通过、
  篡改/错密钥/**跨方向回放**/过期/截断/无会话一律 400、超限 413。
- `crates/phoenix-view/src/lib.rs`：`SecureCodec`、`respond_secure` 三选一分支，
  与旧 `Aes256GcmCodec`/`EncryptedPayload` JSON 路径并存。
- `crates/phoenix-runtime/src/lib.rs`：`secure_transport(...)` 一键接线；端到端
  测试用独立 ring 客户端复算密钥、解出正确信封，并断言无 secure 头时逐字节回退明文。
- `crates/phoenix-http/src/lib.rs`：共享线上常量。
- 客户端契约：`packages/phoenix-react/src/secure.ts`（`decryptFrame` /
  `sealRequest`），接线在 `page-client.ts` 的 `submitPage`；用例覆盖封帧字节布局、
  服务端可解、content-type 随行、跨方向回放失败、会话过期返回 `null`、
  每帧 nonce 不重复。
