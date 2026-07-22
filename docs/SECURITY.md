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

这些能力由 `examples/blog/tests/` 中的真实 TCP 与进程内测试覆盖。

## 3.2 尚未实现的安全能力

- TLS/HTTPS 终止与可信代理判断。
- 会话、Cookie 策略、认证与 CSRF。
- CORS、限流、Host allowlist 和代理头清洗。
- CSP nonce、HSTS 和生产/开发模式差异化安全头。
- 上传文件、Multipart、静态文件路径与下载响应。
- 结构化安全日志、request ID、敏感字段自动脱敏和依赖漏洞 CI。

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
