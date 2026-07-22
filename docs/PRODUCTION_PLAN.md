# 生产化实施与验收矩阵

本文是 Phoenix 从技术预览走向可审查生产候选版本的执行清单。功能只有在代码、失败边界、自动测试、公开文档和全仓回归均存在时才标记完成。

| 能力 | 状态 | 验收证据 |
| --- | --- | --- |
| TLS/HTTPS 与 ALPN | 已完成首版 | rustls PEM 配置、握手 deadline、`h2`/`http/1.1` ALPN、真实 TLS+HTTP/2 测试、可信 scheme、canonical redirect、HTTPS-only HSTS |
| RBAC/ABAC | 已完成首版 | 精确角色/权限与继承图、资源属性 policy、deny precedence、typed principal/middleware、401/403、决策审计测试 |
| JWT refresh/revocation | 已完成首版 | 原子 rotation、并发/reuse detection、hashed refresh secret、access/family revoke、内存/文件 store contract、重启持久化与过期清理 |
| 分布式 Session | 待实现 | 原子 load/save/rotate/delete、TTL、冲突语义、共享后端适配器与双实例测试 |
| 分布式限流 | 待实现 | 原子窗口操作、key policy、Retry-After、共享后端故障策略与双实例测试 |
| 指标 exporter | 待实现 | 请求/连接/TLS/renderer/数据库/队列指标、Prometheus 文本端点、低基数规则 |
| CSP nonce | 待实现 | 每请求随机 nonce、Header/HTML/renderer 一致、Vite dev/production 策略、缓存边界 |
| WebSocket/SSE/流式请求 | 待实现 | 受控 upgrade、backpressure、取消、大小/deadline、优雅关闭、真实网络测试 |
| 队列 | 待实现 | job envelope、重试/backoff、幂等键、dead-letter、worker shutdown、持久化 backend contract |
| 邮件 | 待实现 | Message builder、文本/HTML、Header 注入防护、transport contract、内存测试 transport |
| 管理后台 | 待实现 | 认证/授权保护、资源列表/表单、审计日志、可替换 UI 与示例 |
| 插件机制 | 待实现 | manifest、兼容版本、显式注册、权限边界、冲突诊断、示例插件 |
| 正式安全评审 | 待实现 | threat model、依赖/许可证审计、模糊测试、公开报告、残余风险与修复验证 |
| 项目改名 | 待评估 | crates.io/npm/GitHub/搜索冲突、候选评分、迁移路径和最终 ADR |

## 跨阶段不变量

- 现有 `Application::new(routes)` 与多应用 API 保持兼容，破坏性变更必须有迁移说明。
- 真实 key、token、Cookie、客户数据、生产 Host/IP 不进入源码、fixture、日志或外部服务。
- 分布式能力通过 trait 定义语义；内存实现用于本地测试，生产适配器必须具备原子性测试。
- 默认拒绝未知算法、未知权限、未知插件能力和不可信代理信息。
- 指标标签、日志字段和审计事件不包含 token、密码、query、Header 值或高基数用户输入。
- 每个阶段完成后运行 workspace tests、严格 Clippy、Rustfmt、前端测试、类型检查和生产构建。

## 当前执行顺序

1. TLS/HTTPS 与有效 scheme。
2. RBAC/ABAC 和 token 生命周期。
3. 分布式状态与指标。
4. CSP nonce 与 React renderer 联动。
5. 实时/流式协议。
6. 队列、邮件、管理后台和插件。
7. 安全评审、改名决策与发布候选验收。
