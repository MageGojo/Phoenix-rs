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

把已经验证的 React 页面协议接入生产构建与长期运行的 renderer，同时继续推进类型化请求：

```text
views/**/*.tsx
  -> Vite 页面与 island 发现
  -> browser/server manifests
  -> 持久 renderer 池
  -> phoenix-view HTML
```

## 建议执行顺序

1. 用 `phoenix-vite` 自动发现页面和 islands，生成浏览器入口、服务端 bundle 与版本化 manifest。
2. 把 `@phoenix/react-ssr` 接入持久 Node renderer，验证超时、崩溃恢复、背压和版本握手。
3. 从 renderer 结果填充 `trusted_server_html`，移除博客控制器中的静态演示 HTML。
4. 为 Query、Path、JSON 和 Form 实现类型化 extractor，并复用现有错误语义。
5. 创建契约 spike，验证 Serde 映射、重名诊断、敏感字段和自动 TypeScript 生成。
6. 创建 Toasty spike，验证模型定义、CRUD、关系、事务、分页与迁移 API。

## 下一切片验收标准

- TSX 页面与 island 不需要手写注册表。
- SSR/Islands 的 HTML 来自持久 renderer，renderer 不可用时快速失败且不静默切换模式。
- 浏览器 bundle 只包含当前模式需要的代码，Islands 只加载页面实际出现的岛。
- 页面 manifest、资源版本和协议版本不一致时启动失败。
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
