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

## 2026-07-22：TypeScript 命名路由树

- `phoenix-vite` 自动扫描标准 Rust 路由目录，把字面量 `.name("...")` 生成到只读 `views/generated/routes.ts`。
- 点分路由名生成嵌套属性，静态 `RouteGroup` 名称前缀自动合并；动态名称、重复名称和 TypeScript 树冲突在生成阶段失败。
- 成员 Island 从裸字符串升级为 `callRust<Member>(members.store, { name })`，获得编辑器补全和 Rust 路由重命名检查。
- 生成常量只保存稳定名称，浏览器仍使用 Rust 注入的运行时路由表解析 URL；接口输入/输出自动推导明确留给强类型契约切片。
- 生成器 5 个测试和博客 React 6 个测试通过；真实浏览器使用生成属性创建成员成功，控制台无错误，SSR 表格继续位于唯一 Island 之外。

## 2026-07-22：Toasty 数据库与迁移系统

- 新增 `phoenix-database`，固定 Toasty `0.8.0`，支持 SQLite 与 PostgreSQL URL、连接池配置和顶层 Phoenix 重导出。
- 保留 Toasty 原生强类型模型 API，并以集成测试验证 SQLite CRUD、has-many/belongs-to、游标分页和事务 commit/rollback。
- 新增每测试独享的内存 SQLite `TestDatabase`，创建即初始化 schema，drop 即丢弃全部状态，不依赖测试顺序或共享清理。
- 新增 Phoenix 迁移执行器，支持有序 ID、up/down、状态查询、计划、SHA-256 校验和、batch 和不可逆迁移失败关闭。
- SQLite 使用 `BEGIN IMMEDIATE` 同时实现迁移锁和整批原子回滚；PostgreSQL 使用 advisory lock，并逐迁移事务提交。
- 空数据库会自动创建 `phoenix_migrations`；失败 SQL 测试验证同批已执行 DDL 与状态记录均被回滚。
- PostgreSQL 复用同一 CRUD/关系/分页契约测试，设置 `PHOENIX_TEST_POSTGRES_URL` 时连接真实实例执行。

## 2026-07-22：强类型请求与 Rust/TypeScript action 契约

- `phoenix-http` 新增 `Query<T>`、`Path<T>`、`Header<T>`、`Json<T>`、`Form<T>` 和 `Multipart<T>` extractor；Multipart 通过 `FromMultipart` 形成业务上传 DTO，并提供最多四参数的 `typed(...)` handler。
- `phoenix-validation` 新增 `Validate`、`Validated<E>` 和 `max_length`；提取错误自动映射为 400/415 JSON，字段验证失败自动映射为 422 JSON。
- 新增 `#[phoenix::contract(...)]`，覆盖 Input、Resource、Page Props 与 Shared Props；Vite 自动生成 TypeScript 类型、页面映射和稳定 contract hash。
- 生成器遵守方向性 Serde rename/default/flatten/alias/skip 规则，处理容器 default 与 unit enum alias/skip，检查 flatten、alias 和 enum wire-name 冲突；不安全大整数、tuple/generic struct 及无法准确表达的 wire transform 会失败关闭。
- Rust 路由通过 `.action::<Input, Output>()` 生成可调用 action；成员 Island 已从 `callRust<Member>(members.store, { name })` 收敛为 `members.store({ name })`，输入和返回值均由 Rust 推导。
- 成员页面删除手写 `Member` 与页面 Props 接口，改用 Rust `MemberResource`、`MembersPageProps` 和 `SharedProps` 生成结果。

## 2026-07-22：Web 安全基础栈

- 新增 `phoenix-security`，实现服务端 Session、安全 Cookie、会话 ID 轮换/注销、Session CSRF、精确 CORS、固定窗口限流、可信代理和 Host allowlist。
- Hyper 接入层把真实 TCP peer 写入 Request extensions；代理解析只有在直连 peer 明确信任时才消费 XFF，并按从右到左的 hop 链解析客户端地址。
- 新增可配置 CSP/HSTS 安全策略、随机 request ID、无 query/无 Header 值的结构化访问日志和敏感 Header 脱敏辅助函数。
- 5 个路由级测试覆盖 Cookie 属性与 CSRF 往返、会话轮换、代理欺骗边界、Host/CORS/限流拒绝、安全头、request ID 唯一性和日志脱敏。
- `cargo test -p phoenix-security`、严格 Clippy 和 `phoenix-core` 测试通过。

## 2026-07-22：生产资源与流式 SSR

- `phoenix-vite` 客户端构建输出 hash 文件名和 `phoenix-manifest.json`，包含 schema、构建版本、contract hash、公开路径、入口、CSS 与 import；SSR 构建输出 renderer manifest。
- `AssetManifest`/`RendererManifest` 在 Rust 启动侧校验 schema、相对路径、入口和 client/renderer contract hash；静态解析只接受清单明确拥有的文件。
- `Page::production_assets` 从 manifest 注入真实脚本、样式、asset version 和 contract hash；renderer worker 握手同时校验 client asset version 与 contract hash。
- `NodeRenderer` 支持可配置 worker 池、预热、健康快照、超时淘汰、故障替换和显式优雅关闭；两 worker 并发测试固定容量行为。
- React `renderToPipeableStream` 通过分帧协议连接 `ResponseBody::Stream` 与 Hyper，真实 TCP 测试验证无 Content-Length 的 chunked 响应，hydration 信封在完成帧后安全写入。
- Rust 15 个 view 测试、真实 Hyper 流测试、严格 Clippy、Vite 7 个测试、SSR 包测试和真实 client/SSR 生产构建通过。

## 2026-07-22：Laravel 风格开发体验

- 新增 `mount_routes!()`，按文件名确定性扫描并合并 `routes/*.rs`；博客案例入口不再手写单一路由文件调用。
- 新增 resource routes，覆盖七个标准 action、PUT/PATCH update、`only`、`except` 和自定义模型参数名。
- 新增中间件别名注册表；未知别名在路由构建前失败。`ModelBinding<T>` 异步加载路径模型并通过 `Bound<T>` 交给 handler，缺失/失败分别映射 404/500。
- 新增 `phoenix dev` 进程监督器，同时运行 Rust 与 strict-port Vite；Ctrl-C 或任一子进程退出时终止并回收两个 Unix 进程组。
- DX/CLI 单元测试、博客自动路由功能测试和真实双进程启动/退出验证通过；退出后 Rust 与 Vite 监听端口均已释放。

## 2026-07-22：五个功能域全量验收

- `cargo test --workspace --locked` 通过，覆盖数据库、迁移、安全、流式 HTTP、renderer 池、DX/CLI 与博客案例。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 和 `cargo fmt --all -- --check` 通过。
- React、React SSR、Vite 与博客共 30 个前端测试通过，示例 TypeScript 类型检查通过。
- 真实 client 构建生成 hash 资源与 `phoenix-manifest.json`；后续 SSR 构建校验相同 contract hash 并生成 renderer manifest。

## 2026-07-22：开发者使用文档归类

- 新增 `docs/DATABASE.md`，集中说明 Toasty 模型、SQLite/PostgreSQL、CRUD、关系、游标分页、事务、迁移与测试隔离。
- `docs/SECURITY.md` 增加完整中间件装配顺序、Session/CSRF、Cookie、CSP/HSTS 和日志使用示例。
- `docs/RENDERING.md` 增加 client/SSR 构建顺序、manifest、renderer 预热、流式页面、静态资源、健康指标与关闭流程。
- `docs/DX.md` 和 `docs/BUSINESS_GUIDE.md` 记录自动路由、resource routes、中间件别名、模型绑定与 `phoenix dev` 的当前公开用法。
