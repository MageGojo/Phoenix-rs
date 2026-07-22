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

## 2026-07-22：持久 React SSR renderer

- `phoenix-view` 新增长期运行的 Node renderer 客户端，使用版本化按行 JSON 协议和启动握手。
- 单 worker 并发槽位与 Node 响应共用 2 秒 deadline；超时快速失败，进程退出后重启并重试一次。
- renderer 子进程清空继承环境，只接收 `NODE_ENV=production`，不继承应用密钥或数据库配置。
- `/react/ssr` 与动态 `/members` 已接入真实 `renderToString` 输出；页面协议导航继续直接返回相同业务 props。
- `/members` 完整响应已验证包含 Rust 动态数据生成的业务 HTML，并可由浏览器 `hydrateRoot` 接管。
- Rust workspace 23 个案例测试、React 11 个测试、严格 Clippy 和格式检查通过。

## 2026-07-22：成员目录 Islands 验证

- `/members` 从整页 SSR hydration 切换为 Islands；Rust 仍提供 100 条初始数据，持久 renderer 仍生成完整首屏 HTML。
- 页面外壳不进入 hydration，`member-directory` 是唯一 hydration root，拥有独立浏览器入口。
- 成员目录 island 支持在浏览器会话中动态添加成员，并继续负责搜索、筛选、排序和分页。
- Rust 页面信封测试固定 island ID、组件名和 100 条 island props；jsdom 测试验证逐岛 hydration 与动态添加。
- 完整 Cargo 测试、严格 Clippy、TypeScript 类型检查、React 测试和 SSR 构建通过。

## 2026-07-22：简化 Islands 与命名 Rust action

- Rust Island 声明收敛为 `.island("member-directory", props)`，默认用组件名作为 island ID；多实例场景保留 `.island_with_id(...)`。
- React 使用 `island(MemberDirectory)` 与 `islands: [MemberDirectory]` 自动推导 `member-directory`，不再重复填写注册键。
- 路由器自动把 Rust 命名路由表注入页面协议，React 通过 `callRust("members.store", { name })` 调用后端，无需硬编码 `/api/members`。
- `/api/members` 由 Rust 完成输入校验、ID 分配和成员数据构造；成员 island 展示提交中、成功和错误状态。
- Cargo workspace 全量测试与 React 15 个测试通过，严格 Clippy 和 TypeScript 类型检查通过。

## 2026-07-22：Astro 风格 Islands 自动发现

- 新增 `@phoenix/vite`，自动发现 `views/pages` 与 `views/islands`，生成浏览器动态加载入口和服务端 renderer 入口。
- 页面可直接写 `<MemberCreator client:load />`；Vite 编译指令，组件内部不需要 Phoenix HOC 或专用 props。
- SSR renderer 自动收集实际 island 的组件名、稳定实例 ID 与 JSON props，Rust 通过 `Page::rendered` 合并结果；控制器不再手写 `.island(...)`。
- SSR 模式移除局部 wrapper 并整页 hydration；Islands 模式只加载页面信封中实际出现的动态组件。
- 成员案例拆成静态概览/表格 SSR 与 `member-creator` 表单 island，浏览器新增仍通过 Rust 命名 action 完成。
