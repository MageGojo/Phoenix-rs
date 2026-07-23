# 生产化实施与验收矩阵

本文是 Phoenix 从技术预览走向可审查生产候选版本的执行清单。功能只有在代码、失败边界、自动测试、公开文档和全仓回归均存在时才标记完成。

| 能力 | 状态 | 验收证据 |
| --- | --- | --- |
| TLS/HTTPS 与 ALPN | 已完成首版 | rustls PEM 配置、握手 deadline、`h2`/`http/1.1` ALPN、真实 TLS+HTTP/2 测试、可信 scheme、canonical redirect、HTTPS-only HSTS |
| RBAC/ABAC | 已完成首版 | 精确角色/权限与继承图、资源属性 policy、deny precedence、typed principal/middleware、401/403、决策审计测试 |
| JWT refresh/revocation | 已完成首版 | 原子 rotation、并发/reuse detection、hashed refresh secret、access/family revoke、内存/文件 store contract、重启持久化与过期清理 |
| 分布式 Session | 已完成首版 | 异步 load/create/CAS save/CAS rotate/CAS delete、滑动 TTL、409/503 失败关闭、成功后 Cookie 提交、指标与共享 backend 双 Router 测试 |
| 分布式限流 | 已完成首版 | 原子 `RateLimitBackend::hit`、可替换有界 key policy、Retry-After、默认失败关闭/显式失败开放、指标与共享 backend 双实例测试 |
| 指标 exporter | 已完成首版 | HTTP 延迟/状态 middleware、连接/TLS server 接入、renderer snapshot、数据库/队列 hooks、Prometheus 0.0.4 文本端点、固定低基数标签测试 |
| CSP nonce | 已完成首版 | 每请求随机 nonce、Header/HTML/Vite/renderer 一致、renderer v2、开发 origin 校验、HTML/XHTML no-store、失败关闭/断连取消及官方 React Suspense 跨语言 E2E |
| 工程质量门禁 | 已完成首版，托管 CI 待首跑 | Rust fmt/Clippy/test、Node test/typecheck/build、真实 PostgreSQL service、CSP E2E、Gitleaks、cargo-deny、npm audit、覆盖率下限、Criterion 基线与定时 fuzz |
| 流式请求 | 已完成首版 | 路由预分类、typed one-shot stream、backpressure、Content-Length/chunked 限额、绝对 deadline、断连、H1 pipeline、H2 并发与关闭测试 |
| WebSocket/SSE | 已完成首版 | SSE TCP keepalive/取消；H1 Upgrade WebSocket（Origin/大小/关闭）；H2 CONNECT 未交付；见 REALTIME.md |
| Redis 适配器 | 已完成首版 | `phoenix-redis` Session/限流/Token + Lua；无 Redis 单测绿，`PHOENIX_TEST_REDIS_URL` 双实例契约；见 REDIS.md |
| 测试门面 + 上传存储 | 已完成首版 | `TestApp` Cookie/页面协议断言；`LocalDisk` 路径净化与原子写；见 TESTING_AND_STORAGE.md |
| 队列 | 已完成首版 | Job envelope、重试/backoff、幂等键、dead-letter、Memory worker、metrics；见 QUEUE.md |
| 邮件 | 已完成首版 | Message builder、Header 注入防护、MemoryTransport；见 MAIL.md |
| 应用控制台 | 已完成首版 | `Console` + `commands!` + `px make:command`；`cargo run -- <cmd>`；见 QUEUE_MAIL_CONSOLE.md |
| 管理后台 | 待实现 | 认证/授权保护、资源列表/表单、审计日志、可替换 UI 与示例 |
| 插件机制 | 已完成首版 | `Plugin` + `FeatureSet`；能力 allowlist；冲突诊断；示例 `plugin-greeter`；见 FEATURES.md |
| 发版流水线 | 已完成首版 | `px release*` + `phoenix-release`；制品/校验/切换/回滚；见 RELEASE_PIPELINE.md |
| 正式安全评审 | 待实现 | threat model、依赖/许可证审计、模糊测试、公开报告、残余风险与修复验证 |
| 项目改名 | 已定案 | 对外 **Phoenix-rs**；CLI=`px`；门面=`phoenixrs`（ADR-009） |

## 跨阶段不变量

- 现有 `Application::new(routes)` 与多应用 API 保持兼容，破坏性变更必须有迁移说明。
- 真实 key、token、Cookie、客户数据、生产 Host/IP 不进入源码、fixture、日志或外部服务。
- 分布式能力通过 trait 定义语义；内存实现用于本地测试，生产适配器必须具备原子性测试。
- 默认拒绝未知算法、未知权限、未知插件能力和不可信代理信息。
- 指标标签、日志字段和审计事件不包含 token、密码、query、Header 值或高基数用户输入。
- 每个阶段完成后运行 workspace tests、严格 Clippy、Rustfmt、前端测试、类型检查和生产构建。
- 门禁命令、覆盖率下限、审计例外与本地复现方式统一记录在 [工程质量门禁](QUALITY_GATES.md)。

## 当前执行顺序

1. ~~TLS/HTTPS 与有效 scheme~~（已完成首版）
2. ~~RBAC/ABAC 和 token 生命周期~~（已完成首版）
3. ~~分布式状态与指标~~（已完成首版）
4. ~~CSP nonce 与 React renderer 联动~~（已完成首版）
5. ~~实时/流式协议、队列/邮件/console、Feature 插件、发版流水线~~（已完成首版）
6. **管理后台**（认证保护资源 CRUD、审计、可替换 UI）。
7. 正式安全评审与 crates.io 发布候选验收。
