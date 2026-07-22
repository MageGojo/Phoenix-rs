# 下一阶段

## 当前目标

用最小垂直切片验证关键技术假设，在扩展功能前证明以下链路可工作：

```text
GET /posts
  -> 路由
  -> 控制器
  -> Toasty + SQLite 查询
  -> 页面协议
  -> Vite + React 页面
  -> 浏览器渲染
```

同时验证一次表单提交：请求 DTO -> 验证失败 -> React 字段错误，以及验证成功 -> Toasty 写入 -> 命名路由重定向。

## 建议执行顺序

1. 建立 Cargo workspace、最低 Rust 版本和基础 CI 检查。
2. 创建 Toasty spike，验证模型定义、CRUD、关系、事务、分页与迁移 API。
3. 创建 Hyper spike，验证 Tokio 连接适配、Phoenix `Handler`/提取器门面、路由分发、错误映射和中间件顺序。
4. 定义页面协议 v1，并验证安全 JSON 注入、完整刷新与协议请求。
5. 接入 Vite、React、TypeScript 和生产 manifest。
6. 完成博客列表与创建表单的端到端测试。
7. 根据 spike 结果修订 `docs/DX.md` 和 ADR，再决定公共 API。

## 第一阶段验收标准

- `cargo test --workspace` 可重复通过。
- 前端类型检查和生产构建可重复通过。
- SQLite 在空库上执行迁移并完成 CRUD。
- Rust 控制器能够把强类型 props 交给 `.tsx` 页面。
- 特殊字符 props 不产生脚本注入。
- 验证错误能够在完整页面和协议响应中保持一致。
- 文档中的目标 API 与实际实现差异已被记录，而不是静默偏离。

## 阻塞前必须回答的问题

- Toasty `0.8.x` 的迁移 API 是否足够稳定，是否需要 Phoenix 自有迁移描述层？
- Toasty 的 Rust `1.95` 要求是否符合项目预期的最低工具链？
- Hyper 核心如何归一化请求 body，同时为流式响应保留不泄漏复杂泛型的逃生口？
- P0 是否只承诺 SQLite + PostgreSQL，把 MySQL 标为实验性？
- 页面导航 P0 应直接支持局部导航，还是先以完整文档导航完成闭环？
- 工作名称 Phoenix 是否在技术预览前更换？

## Definition of Done

下一阶段完成的标准不是“搭好很多 crate”，而是一个新手能按 README 启动示例、查看数据库列表页、提交表单、看到验证错误，并能用测试复现整条链路。
