# 下一阶段

## 已完成基础

第一实现切片已经验证以下链路：

```text
TCP -> Hyper HTTP/1.1 -> Phoenix Request
    -> 全局/路由组中间件
    -> Laravel 风格路由与命名
    -> async 控制器
    -> 自定义验证规则
    -> Phoenix Response
```

权威验证命令：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## 本轮状态

数据库、迁移、安全、生产构建/SSR 与 Laravel 风格开发体验已经完成并通过全量回归：

```text
Toasty + migrations
  -> security middleware stack
  -> versioned assets + renderer pool + streaming SSR
  -> conventional routes + resource/binding + phoenix dev
```

强类型请求提取与跨端契约也已完成当前稳定范围：`Query`、`Path`、`Header`、`Json`、`Form`、`Multipart<T>`、`Validated<DTO>`，以及 Input、Resource、Page Props、Shared Props 和可直接调用的命名 action。未支持的 Rust/Serde wire 形态在构建期失败关闭。

Laravel 风格 CLI 也已完成：`phoenix new`、`phoenix dev` 和 controller/model/migration/request/resource/middleware/page/island 生成器；`make:model --all` 会形成可编译、可构建、可运行的完整业务骨架。

## 建议执行顺序

1. （已完成）Toasty SQLite/PostgreSQL CRUD、关系、分页、事务、隔离测试与可靠迁移执行器。
2. （已完成）Session/CSRF/CORS/限流/可信代理/Host/安全头/request ID/日志脱敏。
3. （已完成）版本化 client/renderer manifest、生产静态解析、contract/resource 握手、多 worker、健康状态、关闭与流式 SSR。
4. （已完成）约定式 `routes/*.rs` 自动挂载、REST resource routes、中间件别名和异步模型绑定。
5. （已完成）统一启动 Rust/Vite、转发退出信号并回收子进程组的 `phoenix dev`。
6. （已完成）workspace、前端包、博客生产构建和仅暂存快照回归。

## 本轮验收结果

- 标准路由文件由 `mount_routes!()` 确定性挂载；重复名称和 method/path 在构建时诊断。
- resource routes 覆盖七个标准 action、PUT/PATCH update、`only` 和 `except`。
- 未知中间件别名失败关闭；模型不存在映射为 404，加载失败映射为通用 500。
- `phoenix dev` 同时拉起 Rust/Vite；任一进程失败或 Ctrl-C 都会回收两个进程组。
- Cargo workspace、严格 Clippy、Rustfmt、React/Vite 测试、示例类型检查及 client/SSR 生产构建全部通过。

## 下一阶段优先级

1. TLS/认证授权接入文档与部署模板。
2. 分布式 Session store、限流后端和指标 exporter。
3. CSP nonce、流中错误语义与 hydration 诊断。
4. PostgreSQL CI 服务与迁移并发压力测试。
5. 技术预览前完成项目名称、公共 API 和安全评审。

## 待验证决策

- Extractor 已在归一化 Request 上组合；后续需决定 State extractor 和多个 body extractor 的编译期互斥方式。
- 异步规则采用 boxed future、关联 future 还是宏生成，哪种编译错误对新手最清楚。
- 数据 enum、tuple/generic struct 的契约表达。
- P0 是否只承诺 SQLite + PostgreSQL，把 MySQL 标为实验性。
- 模型绑定是否在 P0 只提供显式 binder，还是为 Toasty 模型增加 derive 门面。
- 工作名称 Phoenix 是否在技术预览前更换。

## 当前增强路线

1. （已完成）同端口 HTTP/1.1 + HTTP/2 自动识别、协议限制和结构化日志初始化。
2. （已完成）JWT 签发/校验、Bearer 提取、密钥轮换与成熟密码学门面。
3. （已完成）兼容现有 `Application::new(routes)` 的多应用挂载、Host/路径分派与应用级状态。
4. （已完成）在稳定运行时 API 上提供路由与多应用声明宏，保证展开结果可读；控制器和中间件继续使用普通类型/函数。
5. （已完成）把官网、前台、后台写入同一示例项目，并补齐端到端测试与迁移文档。

## Definition of Done

本轮增强 Definition of Done 已满足：HTTP/2、日志、JWT/密码学、多应用、宏、三应用示例与全仓回归均有直接证据。下一阶段以 TLS/ALPN 部署、多实例安全状态、完整授权流程和公开 API 稳定性为完成标准。
