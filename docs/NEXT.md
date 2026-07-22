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

## 当前目标

把当前原始 `Request` 控制器升级为类型化请求链路，同时保持已经验证的 API：

```text
Request body/query/path
  -> 内容类型和大小规则
  -> 类型化 extractor / Request DTO
  -> 同步与异步验证
  -> 稳定 JSON/Web 错误响应
  -> 控制器
```

## 建议执行顺序

1. 为 Query、Path、JSON 和 Form 实现类型化 extractor，并区分 400、413、415 和 422。
2. 让验证器支持异步自定义规则、嵌套数组路径、bail 和自定义消息。
3. 建立 `phoenix-testing` 测试客户端，替代案例中的手工 Request/TCP 辅助代码。
4. 为路由增加参数约束、模型绑定接口、fallback 和资源路由。
5. 增加 HTTP/2、流式 body、连接错误日志、关闭超时与并发压力测试。
6. 创建契约 spike，验证 Serde 映射、重名诊断、敏感字段和自动 TypeScript 生成。
7. 创建 Toasty spike，验证模型定义、CRUD、关系、事务、分页与迁移 API。
8. 定义包含渲染模式与契约 hash 的页面协议并接入 React SPA。
9. SPA 稳定后验证 SSR renderer，再实现 Islands。

## 下一切片验收标准

- 控制器可以直接接收已解析并验证的强类型 DTO。
- 错误响应能稳定区分格式错误、内容类型错误、超限和字段验证失败。
- 自定义异步规则可以访问应用状态，但不会迫使纯同步规则产生额外任务。
- 请求测试不需要绑定真实端口；启动测试仍保留真实 TCP 覆盖。
- 已有 11 个案例测试继续通过，新增 extractor 与错误路径测试。
- `cargo test`、严格 Clippy 和格式检查全部通过。

## 待验证决策

- Extractor trait 如何在不暴露 Hyper body 泛型的情况下组合 Path、State 和 body。
- 异步规则采用 boxed future、关联 future 还是宏生成，哪种编译错误对新手最清楚。
- Toasty 迁移 API 是否足够稳定，是否需要 Phoenix 自有迁移描述层。
- 契约层采用现有类型导出库还是自研 derive，哪种能完整遵守 Serde。
- P0 是否只承诺 SQLite + PostgreSQL，把 MySQL 标为实验性。
- 工作名称 Phoenix 是否在技术预览前更换。

## Definition of Done

下一切片完成时，案例控制器不再手工解析 JSON，也不手工把验证错误拼成响应；框架会从请求到强类型 DTO 再到稳定错误响应完成整条链路。
