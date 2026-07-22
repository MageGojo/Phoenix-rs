# 项目进度

## 2026-07-22：项目规划检查点

- 初始化本地 Git 仓库并使用 `main` 分支。
- 完成产品定位、目标用户、核心旅程、P0/P1 范围和发布门槛。
- 完成模块化架构、请求生命周期、React 页面协议、Toasty 适配策略和迁移方向。
- 根据项目约束确定 Hyper 1.x 为 HTTP 核心，路由、处理器、提取器和中间件门面由 Phoenix 实现。
- 明确“数据传输加密”的真实边界：TLS、服务端会话、敏感字段白名单，以及可选安全信封。
- 写出 Laravel 风格 Rust API 草案，所有示例均标记为尚未实现。
- 创建框架 crates、React 包与博客示例应用的目录骨架。

## 2026-07-22：跨端契约与渲染模式规划

- 确定 Rust 是 Request、页面 Props、Shared Props 与 Resource 的唯一契约来源。
- 定义 TypeScript 类型、运行时表单描述、自动生成流程、兼容性 hash 和验证同步边界。
- 定义命名空间、input/output 方向、Serde wire name 与字段碰撞的构建失败规则。
- 将 SPA、SSR、Islands 纳入统一 React 页面协议，并明确分阶段交付顺序。
- 明确 SSR/Islands 默认使用持久 JS renderer，Islands 不等同于 React Server Components。

## 已验证事实

- 当前公开的 `toasty` crate 版本为 `0.8.0`。
- crate 元数据列出 SQLite、PostgreSQL、MySQL、Turso、DynamoDB 相关驱动与 migration feature。
- crate 元数据显示 Rust version 为 `1.95`。

以上仅证明发布元数据，不证明 API 已满足 Phoenix 的模型、关系、事务和迁移要求；这些能力必须通过下一阶段 spike 验证。

## 2026-07-22：Hyper 基础服务检查点

- 建立 Rust `1.95`、edition 2024 的 Cargo workspace 和锁文件。
- 实现 Phoenix Request、Response、JSON、Handler、IntoResponse 与中间件链。
- 实现 Hyper HTTP/1.1 监听、2 MiB 默认 body 上限、临时端口启动和优雅关闭。
- 实现 GET/POST/PUT/PATCH/DELETE、HEAD 回退、OPTIONS、路径参数、404 与 405。
- 实现 Laravel 风格 `.name()`、`RouteGroup` 路径/名称前缀、命名 URL 和冲突诊断。
- 实现 `required`、`string`、`min_length`、`Rule` trait 与闭包式 `custom_rule`。
- 在 `examples/blog` 实现健康、用户、注册和管理控制器，以及全局/组中间件。
- 11 个案例测试通过，其中 1 个通过真实 TCP socket 验证服务启动。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 实际启动案例并验证 `/health`、`/users/{user}`、`/admin/dashboard` 和 `/register` 响应。

## 2026-07-22：基础层安全与易用性强化

- 验证声明收敛为 `field("user", rules![...])`，trait 与闭包自定义规则继续可组合。
- JSON 请求强制检查标准或 vendor `+json` MIME，并区分 415 与 400。
- 增加业务 panic 双层隔离，通用 500 不暴露 panic 内容，安全头仍可应用到业务 panic 响应。
- 路径参数改为严格百分号和 UTF-8 解码，拒绝有损转换。
- 增加请求头/body 读取超时、优雅关闭硬超时和案例级 64 KiB body 上限。
- 官方案例默认启用 `SecurityHeaders`，同时保留全局、组和单路由中间件用法。
- 管理中间件通过 Request extensions 向控制器传递强类型认证上下文，避免业务层重复读取 Header。
- 删除验证器冗余 `.rule()` 写法，只保留组合式 `.field(..., rules![...])` 公共路径。
- 案例测试增加到 18 个，覆盖超限/慢速 body、慢请求头、MIME、非法路径、panic 与安全头。
- `cargo test --workspace` 与严格 Clippy 通过。

## 2026-07-22：React 页面垂直切片

- 新增 `phoenix-view`，实现统一 `PageEnvelope`、HTML 文档响应和 `X-Phoenix-Page` 局部导航协议。
- React 渲染模式支持 Islands、SPA 与 SSR，默认值固定为 Islands；模式只改变渲染方式，不改变页面名与业务 props。
- 新增 `@phoenix/react`，分别使用 `createRoot`、`hydrateRoot` 和逐岛 `hydrateRoot` 启动三种模式。
- 新增 `@phoenix/react-ssr`，SPA 返回空 shell，SSR/Islands 使用 React `renderToString` 生成首屏 HTML。
- 新增可插拔 `PayloadCodec` 和 AES-256-GCM 实现，信封包含版本、算法、`key_id`、用途、签发/过期时间、随机 nonce、密文和独立 tag。
- `examples/blog` 增加真实 TSX 页面、LikeButton island、三种 Rust 路由、页面协议测试和 React renderer 测试。
- Rust 案例测试增加到 21 个；React 包与博客案例共 10 个测试通过。

## 2026-07-22：100 条 Rust 数据 React 页面

- 博客案例新增 `/members` SPA 页面，Rust 控制器确定性生成 100 条成员数据并通过 `PageEnvelope` 传给 React。
- React 页面实现全文搜索、状态与角色筛选、三列排序、每页 10 条分页、无结果状态和移动端列表布局。
- `Page` 新增安全编码的 `script_src` 覆盖，用于从 Vite 开发服务加载真实 TSX 入口。
