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

完成全量回归、Vite 生成侧的独立提交和最终文档一致性检查：

```text
Cargo/TypeScript/React 全量测试
  -> staged snapshot 检查
  -> Vite manifest 生成提交
  -> 文档与验收项收口
```

## 建议执行顺序

1. （已完成）Toasty SQLite/PostgreSQL CRUD、关系、分页、事务、隔离测试与可靠迁移执行器。
2. （已完成）Session/CSRF/CORS/限流/可信代理/Host/安全头/request ID/日志脱敏。
3. （已完成）版本化 client/renderer manifest、生产静态解析、contract/resource 握手、多 worker、健康状态、关闭与流式 SSR。
4. （已完成）约定式 `routes/*.rs` 自动挂载、REST resource routes、中间件别名和异步模型绑定。
5. （已完成）统一启动 Rust/Vite、转发退出信号并回收子进程组的 `phoenix dev`。
6. 执行 workspace、前端包、博客构建和仅暂存快照回归，收口剩余文档。

## 下一切片验收标准

- 标准路由文件无需在 `main.rs` 手工逐个合并，缺失文件、重复名称和重复 method/path 有稳定诊断。
- resource routes 生成 index/create/store/show/edit/update/destroy 的标准集合，并允许 only/except。
- 中间件别名未知时构建失败；模型不存在映射为 404，加载失败映射为通用 500。
- 单个 dev 命令同时拉起 Rust 与 Vite，任一进程失败会终止另一进程，Ctrl-C 不遗留子进程。
- `cargo test`、严格 Clippy 和格式检查全部通过。

## 待验证决策

- Extractor 已在归一化 Request 上组合；后续需决定 State extractor 和多个 body extractor 的编译期互斥方式。
- 异步规则采用 boxed future、关联 future 还是宏生成，哪种编译错误对新手最清楚。
- 数据 enum、tuple/generic struct 的契约表达。
- P0 是否只承诺 SQLite + PostgreSQL，把 MySQL 标为实验性。
- 模型绑定是否在 P0 只提供显式 binder，还是为 Toasty 模型增加 derive 门面。
- 工作名称 Phoenix 是否在技术预览前更换。

## Definition of Done

下一切片完成时，博客案例应只通过标准路由文件、resource 声明、别名中间件和模型绑定表达常用 CRUD，并能由一个 dev 命令完整启动和停止 Rust/Vite。
