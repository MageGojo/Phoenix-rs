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
- 新增 `@apizero/react`，分别使用 `createRoot`、`hydrateRoot` 和逐岛 `hydrateRoot` 启动三种模式。
- 新增 `@apizero/react-ssr`，SPA 返回空 shell，SSR/Islands 使用 React `renderToString` 生成首屏 HTML。
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

- 新增 `@apizero/vite`，自动发现 `views/pages` 与 `views/islands`，生成浏览器动态加载入口和服务端 renderer 入口。
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
- Rust 15 个 view 测试、真实 Hyper 流测试、严格 Clippy、Vite 9 个测试、SSR 包测试和真实 client/SSR 生产构建通过。

## 2026-07-22：Laravel 风格开发体验

- 新增 `mount_routes!()`，按文件名确定性扫描并合并 `routes/*.rs`；博客案例入口不再手写单一路由文件调用。
- 新增 resource routes，覆盖七个标准 action、PUT/PATCH update、`only`、`except` 和自定义模型参数名。
- 新增中间件别名注册表；未知别名在路由构建前失败。`ModelBinding<T>` 异步加载路径模型并通过 `Bound<T>` 交给 handler，缺失/失败分别映射 404/500。
- 新增 `px dev` 进程监督器，同时运行 Rust 与 strict-port Vite；Ctrl-C 或任一子进程退出时终止并回收两个 Unix 进程组。
- DX/CLI 单元测试、博客自动路由功能测试和真实双进程启动/退出验证通过；退出后 Rust 与 Vite 监听端口均已释放。

## 2026-07-22：Laravel 风格项目与业务生成 CLI

- 对外 CLI 二进制统一缩短为 `px`；帮助、错误提示、生成项目 README、测试和业务文档均使用同一命令，不保留旧命令别名。
- `px new` 生成独立 Cargo/npm/Vite/TypeScript 项目、标准业务目录、SPA 首页、Page Props 契约和本地 Git；默认安装依赖并刷新生成类型。
- 新增 controller、model、migration、request、resource、middleware、page、island 生成命令，支持嵌套命名、冲突拒绝和显式 `--force`。
- 生成器只维护 `<phoenix:...>` 区块，自动注册 Rust modules、多个 Toasty 模型、迁移集合、命名路由和 TypeScript contracts/routes。
- `make:model Post --all` 生成模型、迁移、验证 Request、Resource、控制器、七条 resource 路由、类型化 store action、Page Props 与 React 页面。
- 独立临时项目通过 Cargo check、TypeScript、client/SSR 生产构建；实际 HTTP 验证 index 页面、201 JSON action、422 验证错误和运行时命名路由表。

## 2026-07-22：五个功能域全量验收

- `cargo test --workspace --locked` 通过，覆盖数据库、迁移、安全、流式 HTTP、renderer 池、DX/CLI 与博客案例。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 和 `cargo fmt --all -- --check` 通过。
- React、React SSR、Vite 与博客共 30 个前端测试通过，示例 TypeScript 类型检查通过。
- 真实 client 构建生成 hash 资源与 `phoenix-manifest.json`；后续 SSR 构建校验相同 contract hash 并生成 renderer manifest。

## 2026-07-22：开发者使用文档归类

- 新增 `docs/DATABASE.md`，集中说明 Toasty 模型、SQLite/PostgreSQL、CRUD、关系、游标分页、事务、迁移与测试隔离。
- `docs/SECURITY.md` 增加完整中间件装配顺序、Session/CSRF、Cookie、CSP/HSTS 和日志使用示例。
- `docs/RENDERING.md` 增加 client/SSR 构建顺序、manifest、renderer 预热、流式页面、静态资源、健康指标与关闭流程。
- `docs/DX.md` 和 `docs/BUSINESS_GUIDE.md` 记录自动路由、resource routes、中间件别名、模型绑定与 `px dev` 的当前公开用法。

## 2026-07-22：HTTP/2 与结构化日志基础

- Hyper 与 hyper-util 启用 HTTP/2 server-auto；默认监听器按连接 preface 自动服务 HTTP/1.1 或 HTTP/2。
- `HttpProtocol` 提供 `Auto`、`Http1Only` 和 `Http2Only` 三种策略，保留原有 `Application::new(routes)` 调用兼容性。
- 真实 TCP 测试使用 Hyper HTTP/2 客户端完成握手和请求，并验证 HTTP/1-only 模式拒绝 HTTP/2；原 HTTP/1.1 chunked 流测试继续通过。
- 新增 `phoenix-logging`，支持 compact 文本、逐行 JSON、`PHOENIX_LOG` 环境过滤、ANSI/target 配置和重复初始化错误。
- TLS/ALPN 仍属于部署/TLS 适配层；当前 HTTP/2 验证是明文 prior-knowledge 连接，不虚假宣称已交付 HTTPS。

## 2026-07-22：JWT 与通用密码学门面

- 新增 `phoenix-crypto`，明确区分 JWT 签名、AES-256-GCM 可逆加密与 Argon2id 不可逆密码哈希。
- `JwtManager` 固定 HS256 算法、拒绝短于 256 bit 的 secret、要求 `kid`、支持验证旧 key，并校验 `exp`、`nbf`、`sub`、可选 issuer/audience 和 clock leeway。
- 自定义 JWT claims 必须序列化为对象且不能覆盖 `sub/exp/iat/nbf/iss/aud`；Bearer 中间件失败统一返回 401 与 `WWW-Authenticate: Bearer`，成功后提供强类型 `Jwt<T>` extractor。
- `Encryptor` 使用操作系统随机 nonce、版本化 A256GCM envelope、关联数据和解密 key ring；错误关联数据和被篡改密文统一认证失败。
- `Password` 生成带随机 salt 的 Argon2id PHC string，验证时沿用 hash 参数，并限制异常超长输入。
- 7 个密码学与中间件测试、严格 Clippy 和 Rustfmt 通过。

## 2026-07-22：单项目多应用架构

- `Application::multi()` 与 `ApplicationModule` 把官网、用户前台、管理后台编译到同一个 `Application`，原 `Application::new(routes)` 保持兼容。
- 模块默认挂载在 `/{name}` 且命名路由自动加 `{name}.` 前缀；`.root()`、`.prefix()`、`.host()` 和 `.name_prefix()` 可显式覆盖约定。
- 分派器优先匹配 Host-bound 模块，再比较显式端口和最长 path prefix；`/admin` 不会误匹配 `/administrator`。
- 每个模块可以挂载独立 middleware 与同类型不同值的强类型 State；handler 通过 `ApplicationContext` 获得当前模块名、prefix 与 Host。
- 组合 Router 汇总全部命名路由，因此后端 URL 生成和 React route manifest 都能看到 `website.*`、`frontend.*` 与 `admin.*`。
- 新增 `examples/multi-app`，真实验证 `/` 官网、`/app` 前台、`/admin` 后台、隔离 State、404 边界和跨应用 URL 生成。

## 2026-07-22：快速声明宏

- 新增 `routes!`，批量声明 GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS、可选命名路由与逐路由中间件。
- 新增 `applications!`，用 ident 生成稳定应用名，并支持 root、prefix、host、name prefix、State 和 middleware 选项。
- 两个宏只展开为已经验证的 builder API；动态组装继续使用普通 Rust，不另建隐式注册系统。
- `examples/multi-app` 已改为真实使用两个宏；macro doctest、路由中间件测试、三应用集成测试和严格 Clippy 均通过。

## 2026-07-22：增强目标全仓验收

- `cargo test --workspace --locked` 全部通过，覆盖原单应用博客、HTTP/1.1/HTTP/2、JWT/AES-GCM/Argon2id、多应用、声明宏、CLI、数据库、安全与 renderer。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 与 `cargo fmt --all -- --check` 通过。
- React、React SSR、Vite 与博客共 33 个前端测试通过；示例 TypeScript 类型检查通过。
- `@apizero/react`、`@apizero/react-ssr`、`@apizero/vite`、博客 client 和 SSR production build 全部通过。
- 工作树中既有 CLI 脚手架、IDE 配置、临时配置和示例生成数据保持未提交；本轮四个功能提交没有纳入这些并发改动。

## 2026-07-22：应用状态、页面外围协议与安全响应

- `StateMiddleware<T>` 与 `State<T>` 让数据库、配置和外部客户端以可克隆强类型依赖进入控制器；缺失状态返回不泄露内部类型的 500。
- `PageHead` 覆盖 title、description、canonical、robots 与 Open Graph，完整 HTML 和页面信封共享同一受控结构并执行上下文转义。
- `PageEnvelope` 新增可选 CSRF token；React `callRust` 与生成命名 action 自动发送 `X-CSRF-Token`，`Session::csrf_token()` 提供受控读取。
- `Redirect` 验证 Location；`Download` 默认生成 `private, no-store`、`nosniff`、MIME 与双文件名 Content-Disposition，并阻断 CRLF 文件名注入。
- Rust 26 个相关 crate 测试与 React/SSR/Vite/博客 33 个测试通过；外部真实项目集成由 iOS 证书与应用分发案例持续验证。

## 2026-07-22：RBAC/ABAC 与持久化 Token 生命周期

- 新增 `phoenix-auth`，实现精确权限、角色继承图、主体 direct allow/deny、deny-overrides ABAC、默认拒绝和可替换授权审计。
- 重复角色、缺失父角色和继承环在启动编译阶段失败；HTTP 适配提供 `CurrentPrincipal`、JWT principal 映射和资源无关权限中间件，稳定区分 401 与 403。
- JWT 增加随机 `jti` 与 refresh family `sid`；`TokenService` 实现 refresh rotation、并发 reuse detection、单 access token 撤销、family 撤销和过期清理。
- `MemoryTokenStore` 支持测试/开发；`FileTokenStore` 仅保存 refresh hash 和撤销状态，使用同目录临时文件、同步落盘和原子替换，重启后保持状态，持久化失败不会污染内存状态。
- 测试覆盖角色图、资源属性策略、审计、JWT→principal→permission 链路、并发 refresh、reuse family revoke、access revoke、文件重开与持久化回滚；workspace 全量测试、严格 Clippy 和 Rustfmt 通过。

## 2026-07-22：Prometheus 指标 exporter

- 新增 `phoenix-metrics`，以原子 counter/gauge 和固定 latency bucket 输出 Prometheus 0.0.4 文本，不接受任意用户 label。
- `MetricsMiddleware` 采集 HTTP method/status class、活跃请求与耗时；`Application::metrics` 在真实网络边界采集 TCP 连接和 TLS handshake 成败。
- renderer health 可写入同一 registry；数据库和后续 queue worker 使用固定 success/failure/retry outcome hook，Session/限流预留无 ID 的安全状态计数器。
- 测试验证 request query 不进入 exporter、连接 guard 正确归零、TLS 成功计数及 content type；目标 crate 测试和严格 Clippy 通过。

## 2026-07-22：分布式限流 contract

- `RateLimitBackend` 把窗口过期、计数递增和 allow/retry 决策收敛为单个原子 `hit`，生产共享存储可替换内置 memory backend。
- 默认 key 使用可信客户端 IP，`RateLimitKey` 支持应用提供有界租户/API key；响应继续提供准确 `Retry-After`。
- backend 故障默认失败关闭为 503，可显式选择失败开放；rejection/store error 写入无客户端标识的指标。
- 双 limiter/双 Router 测试证明共享 backend 跨实例累计，同步覆盖失败关闭和显式失败开放；目标 crate 测试与严格 Clippy 通过。

## 2026-07-22：分布式 Session backend contract

- 新增版本化 `SessionBackend`，原子定义 load/create/CAS save/CAS rotate/CAS delete、ID collision、missing、conflict 与绝对 TTL 语义。
- `load` 可在不提升版本的前提下延长滑动 TTL，避免并行只读请求相互制造写冲突；业务修改必须携带读取版本。
- `MemorySessionBackend` 作为共享参考实现；双 handle 测试固定 stale write 冲突、旧 ID 原子失效、删除和过期清理。
- `SessionMiddleware::distributed` 已把 Cookie 生命周期接入异步 load/create/CAS save/CAS rotate/CAS delete；旧 `SessionMiddleware::new(SessionStore, ...)` 保持兼容。
- handler 只修改请求级快照，冲突返回 409、backend 故障返回 503；持久化失败或冲突不会发送 `Set-Cookie`，成功 load/commit 后才刷新 Cookie。
- 双 Router 测试覆盖跨实例 create/load/save、原子 ID 轮换、终结式 destroy、并行写冲突、滑动 TTL 以及无 ID 的 conflict/store-error 指标。

## 2026-07-23：CSP nonce 端到端集成

- `phoenix-http` 新增验证并脱敏 Debug 的 `CspNonce`，以及在 Handler 消耗 Request 后保留安全响应元数据的 `ResponseContext`；直接返回 `Page`、`Result<Page, _>` 和状态 tuple 均可自动继承 nonce。
- `NonceSecurityPolicy` 每请求生成 128-bit nonce，拒绝重复 directive、`unsafe-inline`、硬编码 nonce、控制字符和非法 Vite origin；嵌套策略复用同一值，下游 CSP 不一致返回 500。
- 同一 nonce 已贯穿 CSP Header、Vite `csp-nonce` meta、stylesheet、hydration JSON、module script、Rust renderer context 与 React Suspense 流式恢复脚本；nonce 保持在 `PageEnvelope` 和 contract hash 之外。
- Renderer protocol 从 v1 升到 v2，旧 worker 明确失败；同一常驻 worker 的连续请求测试证明 nonce 不串线。SPA/SSR 页面协议请求直接返回 JSON，Islands 仍调用 renderer 收集实际 island 描述。
- 带 nonce 的 HTML/流式响应固定为 `Cache-Control: private, no-store` 并移除验证器，JSON/API 缓存策略保持不变。
- `examples/blog` 与 `px new` 脚手架默认按 debug/release 装配开发/生产 nonce policy；Vite 插件明确不生成构建期静态 nonce。
- 独立审计后补齐大小写 CSP 关键字校验、`default-src`/element directive nonce、短路与 panic 错误安全头、XHTML no-store、ResponseContext query/Header 脱敏、流式失败关闭和 chunk 帧校验。取消/超时会在同一 worker 锁内原子作废进程；排队请求回归证明不会复用残留帧或被前序 reset 误杀。
- `cargo test --workspace --locked`、全目标严格 Clippy、Rustfmt、React/SSR/Vite/博客测试、示例类型检查、三包与 client/SSR 生产构建全部通过。`npm run test:e2e:ssr-csp` 另行真实覆盖 Rust renderer v2 → 官方 `startRenderer` → React Suspense recovery script → CSP/meta/hydration/module 同 nonce。

## 2026-07-23：用途隔离的 HMAC 盲索引

- `phoenix-crypto` 新增专用 `BlindIndexKey` 与 `BlindIndexer`，强制至少 32 byte key、严格脱敏 Debug，并禁止空、控制字符或超长 key ID/purpose。
- HMAC-SHA256 输入使用显式格式版本和 key ID/purpose/value 长度 framing；稳定 envelope 携带算法、版本和 Base64URL key ID，解析拒绝非规范编码与未知 key。
- key ring 固定 active-first 顺序并限制为最多 8 个 active/legacy key；查询候选有界，旧 envelope 可验证，tag 使用常量时间比较。
- 测试覆盖稳定向量、用途隔离、不同 key、轮换候选、短 key、重复/超量 key、Debug 脱敏及畸形/未知/认证失败 envelope。
- 安全文档明确盲索引不是加密，会泄漏等值关系；低熵数据在 key 泄露后仍可离线枚举，且 key 必须独立于 Encryptor、JWT、Session 和其他用途。

## 2026-07-23：生产工程门禁

- 新增 GitHub Actions 门禁，覆盖 Rustfmt、严格 Clippy、locked workspace tests、Node 测试/类型检查/生产构建、官方 React CSP E2E 和真实 PostgreSQL 17 contract test。
- 新增 Gitleaks 全历史扫描、`cargo-deny` advisories/licenses/bans/sources、`npm audit`、Rust/JavaScript LCOV 与最低行覆盖率；本地基线分别为 Rust 89.7%、React 91.1%、React SSR 54.2%、Vite 80.5% 和博客 28.3%。
- JWT 密码学后端从 `rust_crypto` 切换到 `aws_lc_rs`，从依赖图移除存在 RUSTSEC-2023-0071 的 `rsa`；crypto 22 项与 auth 5 项测试通过。
- `RUSTSEC-2025-0134` 仅因 Phoenix 当前 PEM 读取和 Toasty 0.8 PostgreSQL 链路保留临时精确例外；迁移到 `rustls-pki-types` 且 Toasty 上游移除后必须删除。
- 新增独立 locked Criterion 基准与两个 nightly libFuzzer 目标；AWS-LC 构建下盲索引本机基线为 360.89-364.79 ns/次，两个 fuzz 目标的 sanitizer smoke 均通过。
- `actionlint`、四套 JavaScript coverage、Rust workspace coverage、CSP E2E、Gitleaks 45 个提交扫描和 `cargo-deny` 全策略均在本地通过；GitHub PostgreSQL service、Ubuntu sanitizer 与定时任务仍需托管 CI 首跑确认。

## 2026-07-23：可编程 HTML 文档模板

- 从 `phoenix-view` 主模块拆出文档渲染边界，新增 cloneable `DocumentTemplate` 与按页面执行的 Rust 函数入口。
- `DocumentSlots` 支持 html/body/root attributes、可信 head、root 前后 chrome；属性值统一转义并保留框架拥有的 root ID 与渲染模式。
- `DocumentContext` 提供页面信封与当前请求 CSP nonce，供应用自有 script/style 标签使用；Phoenix 继续托管 hydration JSON、module 入口和上下文安全编码。
- 普通、SSR/Islands 完整响应与流式响应共用同一文档模板；模板失败返回不泄露内部详情的 500，页面协议导航不执行 HTML chrome。
- `cargo test -p phoenix-view --locked` 通过 30 个测试，覆盖自定义 chrome、属性注入防护、nonce 函数上下文和错误失败关闭。

## 2026-07-23：真实流式请求 body

- 路由通过 `streaming(handler)` 显式选择 pull-based body；raw handler 可调用 `Request::take_body_stream()`，typed handler 使用 one-shot `RequestBodyStream` extractor。
- Request 保留 Hyper 的 HTTP version 与 extensions；普通构造器保持 HTTP/1.1 兼容默认，为 H1 `OnUpgrade` 和 H2 RFC 8441 protocol extension 留出后续边界。
- 声明的超限 `Content-Length` 在进入中间件/handler 前返回 413；chunked/H2 body 继续由运行时总字节限额约束，读取使用从请求进入 handler 时开始的绝对 deadline。
- `RequestBodyError` 稳定映射 413/408/400；`Json`、`Form`、`Multipart` 在流式路由明确返回配置错误，不再把空缓冲区误解析为客户端输入。
- 真实网络测试证明首块在上传完成前可见、客户端断连可观察、未读取 EOF 不污染 H1 pipeline、H2 同连接并发 stream 保持健康，stalled upload 会在 graceful shutdown 硬期限内终止。

## 2026-07-23：三路并行开工（已由下文「交付完成」收口）

- 设计文档：`docs/PARALLEL_TRACKS.md`、`docs/REDIS.md`、`docs/TESTING_AND_STORAGE.md`、`docs/工具与约定.md`。
- 轨道 A / B / C 目标见下节完成条目（勿再按「进行中」开工）。
  状态：已完成@工作树（见下一节）

## 2026-07-23：三路并行交付完成

- 轨 A：SSE 拆分收口 + H1 WebSocket 门面；`phoenix-core` 真实 TCP 验收（SSE keepalive/断开、WS echo/Origin/超大消息）。
- 轨 B：`phoenix-redis` 实现 Session/RateLimit/Token（Lua）；无 Redis 单测通过，契约测试门控 `PHOENIX_TEST_REDIS_URL`。
- 轨 C：`phoenix-testing::TestApp` 与 `phoenix-storage::LocalDisk` 落地。
- 顶层 `phoenix`：prelude 重导出 SSE/WS；可选 feature `redis` / `storage` / `testing`。
- 产物：`crates/phoenix-{http,core,redis,testing,storage}/`，`docs/{REALTIME,REDIS,TESTING_AND_STORAGE,PARALLEL_TRACKS}.md`。
  状态：已完成@工作树（prelude + feature 已合并；相关包测试/Clippy 通过）

## 2026-07-23：队列 / 邮件 / 应用控制台（开工记录，已收口）

- 设计：`docs/QUEUE_MAIL_CONSOLE.md`
- 轨道：`phoenix-queue`、`phoenix-mail`、`phoenix-console` + `px make:command`
  状态：已完成@工作树（见下一节）

## 2026-07-23：队列 / 邮件 / 应用控制台完成

- `phoenix-queue`：MemoryQueue + Worker（幂等/重试/dead-letter/shutdown/metrics）。
- `phoenix-mail`：Message builder + MemoryTransport（无内置 SMTP）。
- `phoenix-console`：`commands!` + `Console`；脚手架 `cargo run -- serve`；`px make:command`。
- 顶层 feature：`queue` / `mail`；console 默认导出。邮件 `Message` 在 prelude 中为 `EmailMessage`（避免与 WebSocket `Message` 冲突）。
  状态：已完成@工作树

## 2026-07-23：React DX hooks（开工记录，已收口）

- 设计：`docs/REACT_DX_HOOKS.md`
  状态：已完成@工作树（见下一节）

## 2026-07-23：React DX hooks 完成

- `page-state.tsx`：`PhoenixPageProvider` + hooks；flash 本地 consume；`pathMatches`
- `progress.tsx`：顶栏 `ProgressBar`（事件驱动）
- `redirect.ts` + `Form.redirectTo`；Active `Link`（exact/prefix）
- `BrowserNavigator` 整页与 Islands 均注入 `PhoenixPageProvider`
- 文档：`docs/REACT_DX_HOOKS.md`、`docs/RENDERING.md`
- 验收：`packages/phoenix-react` 39 tests 全绿
  状态：已完成@工作树

## 2026-07-23：React DX 表单 P2（开工记录，已收口）

- 设计：`docs/REACT_DX_FORMS.md`
  状态：已完成@工作树（见下一节）

## 2026-07-23：React DX 表单 P2 完成

- `PageForm` / `usePageForm` + `submitPage`；VisitOptions `method`/`data`；422 回填
- `confirm`（Link/Form/PageForm）；可注入 `setConfirmImplementation`
- Vite 为 input 契约生成 `*Fields`；`form.field()` 绑定
- `useOptimisticAction`（乐观更新 / 回滚 / onSuccess）
- 非目标维持：Method spoofing Link、Multipart 上传进度
- 验收：`phoenix-react` 50 测、`phoenix-vite` 13 测全绿
  状态：已完成@工作树

## 2026-07-23：React DX P3/P4（开工记录，已收口）

- 设计：`docs/REACT_DX_PERF.md`
  状态：已完成@工作树（见下一节）

## 2026-07-23：React DX P3/P4 完成

- Prefetch：`prefetchPage` + Link `hover|mount|viewport`；不写当前页 CSRF
- Partial：`only`/`except` + `X-Phoenix-Only/Except` + 客户端 props 合并；`WhenVisible`
- 滚动：`[data-phoenix-scroll-region]` 写入 HistorySnapshot.regions
- ErrorBoundary / lazy 超时可重试 / `NavigationStatusBanner`
- Remember 草稿 + `Form remember`；`PhoenixDevOverlay`
- Island Link：有 Provider 拦截；无 Provider 原生跳转（文档+测试）
- 验收：`packages/phoenix-react` 86 tests 全绿
  状态：已完成@工作树

## 2026-07-23：Phoenix Agent Skill

- 产物：`.cursor/skills/phoenix/{SKILL,api-rust,api-react}.md`；同步 `~/.cursor/skills/phoenix/`
- 内容：新项目清单、`px` 工作流、反模式、Rust/React API 速查
- 文档入口：`docs/工具与约定.md`
  状态：已完成@工作树

## 2026-07-23：GitHub 发布前整理（未 push）

- 说明：`fuzz/` 为框架 cargo-fuzz 质量门禁，保留
- `.gitignore`：排除 `.cursor/*`（**保留** `skills/phoenix/`）、私有示例/密钥/抓包数据、fuzz 产物
- 新增 `LICENSE`（MIT · 极数本源）、重写 `README.md`（署名 [ApiZero](https://apizero.cn/)）
- 清单：`docs/RELEASE.md`
  状态：已整理，等待用户确认后再发布

## 2026-07-23：Laravel 风格 config/*.toml

- `config/app.toml` + `config/database.toml`（选 `sqlite` / `pgsql` / `mysql`）
- 优先级：TOML < `.env` < 进程环境；`DB_CONNECTION` / `DB_PASSWORD` / `DATABASE_URL`
- 脚手架默认生成；文档：`docs/CONFIG.md`
  状态：已完成@工作树

## 2026-07-23：TOML Schema 补全 + MySQL

- JSON Schema：`schemas/phoenix-config-*.schema.json` + `taplo.toml` / `.vscode/settings.json`
- `px new` 生成 `config/schemas/`、`#:schema`、应用级 `taplo.toml`
- Toasty `mysql` feature；`Backend::MySQL` + 迁移锁 `GET_LOCK`；`config/database.toml` 增加 `connections.mysql`
  状态：已完成@工作树

## 2026-07-23：品牌 Phoenix-rs + `cargo install px-cli`

- 对外 / GitHub：**Phoenix-rs**；ADR-009 已接受
- CLI crates.io 包名 **`px`**（目录仍 `crates/phoenix-cli`）→ `cargo install px-cli` 后 `px new`
- 门面 crates.io 包名 **`phoenixrs`**，`[lib] name = "phoenix"`；脚手架 Registry 依赖已对齐
  状态：已完成@工作树

## 2026-07-23：Feature / 插件首版

- `crates/phoenix-plugin`：`Plugin` / `FeatureSet` / `Capability`；冲突与能力 allowlist 失败关闭
- 门面：`phoenix::plugin`；文档：`docs/FEATURES.md`；ADR-039
- 示例：`examples/plugin-greeter`
  状态：已完成@工作树

## 2026-07-23：发版流水线 MVP

- `crates/phoenix-release`：manifest / pack / install / rollback / status
- `px release` / `release:install` / `release:rollback` / `release:status`
- 文档：`docs/RELEASE_PIPELINE.md`；ADR-040；脚手架 `deploy/restart.sh.example`、`/dist` ignore
  状态：已完成@工作树

## 2026-07-23：文档对账刷新

- 同步 BUSINESS_GUIDE / DX / PROJECT / PRODUCT / DATABASE / NEXT / Skill / AGENTS / README 与代码现状
- 关闭「改名 / 仅 sqlite+pgsql / SSE 未完成 / 插件待做」等过时表述
  状态：已完成@工作树

## 2026-07-23：公开托管 README（待确认后 push）

- README：双镜像徽章 + GitHub / GitCode 源码表与 clone / `cargo install --git` 说明
- `docs/RELEASE.md`：双端空仓元数据与 `origin` + `gitcode` push 流程（**未 push**）
- 目标仓：`ApiZero/Phoenix-rs` @ GitHub 与 GitCode
  状态：已于 2026-07-24 实际上传至 `MageGojo/Phoenix-rs`（GitHub）与 `Roufsi/Phoenix-rs`（GitCode）


## 2026-07-24：管理后台 / Auth 示例链路首版

- `examples/blog` 新增 Auth 示例域：固定演示账号、登录、登出、密码重置请求、用户清单与审计事件 fixture。
- 新增 Rust 契约：`LoginInput`、`PasswordResetInput`、`AuthTokenResource`、`AuthMessageResource`、`AdminUserResource`、`AuditEventResource`、`AdminDashboardProps`。
- 新增页面：`views/pages/admin/dashboard.tsx`，展示管理后台指标、用户/角色表与审计日志。
- 路由新增 `/login`、`/logout`、`/password-reset` 与受 `RequireExampleToken` 保护的 `/admin/dashboard` 页面协议链路。
- 回归覆盖登录成功/失败、登出、密码重置 accepted、admin page envelope 与生成 named action 类型树。
- 验收：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked`、`npm run ci:node` 全部通过。
  状态：已完成首版@示例；下一步是把示例链路上升为 `px make:auth` / `px make:admin` 生成器与持久化用户模型。

## 2026-07-24：发布候选工程底盘收口

- 完整本地基线全绿：严格 Clippy、`cargo test --workspace --locked`、`npm run ci:node`、24 个 crate 逐个 `cargo package --locked --no-verify --list`。
- 真实服务契约本地复跑（Docker 一次性容器，端口 15432/13306/16379 避开宿主占用）：`toasty_integration` 4 passed（PG/MySQL 真实链路）、`phoenix-redis` contracts 4 passed；CI service job 门控变量与测试一致，容器已清理。
- crates.io packaging 对账：24 个 crate 元数据齐全、内部 path 依赖已全部带 `version`；verify 失败属「上层 crate 未发布」的顺序问题而非清单问题，拓扑发布顺序写入 `docs/RELEASE.md`。
- 设计文档入库：`docs/RC_CLOSURE_PLAN.md`、`docs/AUTH_ADMIN.md`（拆分提交）。
  状态：工程底盘已收口；剩余红线项为实际 `cargo publish` / `git push`（等用户确认），下一步进入 `px make:auth` 持久化链路（见 `docs/AUTH_ADMIN.md`）。

## 2026-07-24：npm 前端包准备发布

- 去掉 `private: true`，补齐元数据；tsconfig 排除测试；`LICENSE` 入包
  状态：已完成准备

## 2026-07-24：公开同步 + px-cli 0.1.1

- 推送 GitHub `origin` + GitCode `gitcode`：`@apizero/*` 前端包名、`px dev` Rust 热重载、脚手架 tarball 依赖（`258e87a`）
- crates.io 已发布 `px-cli` **0.1.1**
  状态：已完成@258e87a


- `DevSupervisor` 默认 `watch_rust`：监听 `app/`、`src/`、`routes/`、`config/`、`database/`、`Cargo.toml`，变更后重启 `cargo run -- serve`
- 编译失败不拖垮 Vite，等待下次改动再重建；Vite 仍走 HMR
- 依赖：`notify`；实现拆到 `crates/phoenix-cli/src/dev.rs`；ADR-025 / DX / BUSINESS_GUIDE 已同步
- 验收：`cargo test -p px-cli` 全绿；本机 `cargo install --path crates/phoenix-cli --force`
  状态：已完成@工作树


- `@phoenix/*` 无权限；`@phoenixrs` Scope not found → 改发 `@apizero/react|vite|react-ssr@0.1.2`
- packument `npm view` 仍 404，tarball 可装；脚手架 Registry 依赖改为 tarball URL；本地 `cargo install --path crates/phoenix-cli` 已替换 `px`
- 验收：tarball `npm install` 可解析 `@apizero/vite` / `@apizero/react`；`px-cli` scaffold_commands 2 passed；本机 `px new`（Local 探测）成功
  状态：已完成@工作树（crates.io 上的旧 `px` 仍写 `@phoenix/react`，需另发 `px-cli` 才惠及 `cargo install px-cli` 用户）

## 2026-07-24：release staging 空白页

- 根因：① HTML 硬编码 `/assets/phoenix.js`（未 `production_assets`）；② pack 把 assets 摊平进 `public/`；③ 无静态资源中间件 → JS 404
- 修复：`ServeProductionAssets` + 脚手架/manifest 接线；`write_staging` 落到 `public/assets` / `public/ssr`；`px_text` 已本地验证 hashed JS/CSS 200
  验收：`cargo test -p phoenix-view -p phoenix-release --lib` 全绿；staging HTML 含 `/assets/phoenix-*.js` 且资源 200
  产物：`crates/phoenix-view/src/static_files.rs`、`crates/phoenix-release/src/pack.rs`、`crates/phoenix-cli/src/scaffold.rs`
  状态：已完成@工作树

## 2026-07-24：生产 SPA `useState` null

- 根因：`file:` 链接 `@apizero/react` 时 Vite 生产把 monorepo 与 app 各打一份 React；island hooks 读到 null dispatcher
- 修复：`@apizero/vite` `resolve.dedupe` + `optimizeDeps.include`；重建 `px_text` assets；staging 已换新 `phoenix-BtQnio5N.js`，like-button 773B 且从入口 import React
  验收：island chunk `import{r as R}from"../phoenix-*.js"`；`/` + entry + island 200
  产物：`packages/phoenix-vite/src/index.ts`
  状态：已完成@工作树

## 2026-07-24：公开发布补丁（release 空白页 + React 双份）

- 版本：`@apizero/vite@0.1.3`、`phoenix-view/phoenix-release/phoenixrs@0.1.1`、`px-cli@0.1.2`
- 内容：生产静态资源中间件与 pack 布局、脚手架 `production_assets`、Vite `dedupe` React
- 已推送：GitHub `origin/main` + GitCode `gitcode/main` @`f170999`
- 已发布：npm `@apizero/vite@0.1.3`；crates.io `phoenix-view`/`phoenix-release`/`phoenixrs` 0.1.1、`px-cli` 0.1.2
  状态：已完成@f170999

## 2026-07-24：release 二进制体积与数据库驱动按需编译

- 根因确认：脚手架固定给 Toasty 启用 SQLite、PostgreSQL、MySQL，导致实际执行迁移的 `phoenix-manage` 把三套驱动全部静态链接；应用主程序未触发 DB 路径时则可能被链接器偶然裁掉。
- 修复：`phoenix-database` / `phoenixrs` 暴露数据库 features；新应用以 `sqlite` / `pgsql` / `mysql` 只选择一个驱动，默认 SQLite；未编译驱动返回稳定错误。
- 体积 profile：脚手架与框架 release 使用 `opt-level = "z"`、LTO、`codegen-units = 1`、strip，保留 `panic = "unwind"`。
- 验收：`px_text` 的依赖图只含 `toasty-driver-sqlite`；`cargo check --bins`、`cargo test`、release 管理命令均通过；SQLite-only + size profile 实测 `px-text` **2,071,616 bytes**、`phoenix-manage` **2,658,624 bytes**（优化前约 6.8 MiB / 14 MiB）。配置切换到未编译 MySQL 时稳定返回 `BackendNotCompiled { feature: "mysql" }`。
  状态：已完成@工作树

## 2026-07-24：可选能力 Cargo feature（ADR-042）

- 已完成：`tls` / `websocket` / `sse` / `auth` / `jwt` / `password` / `metrics` 做成门面 feature（WS 与 SSE 不合并）；数据库 optional WIP 一并收口。
- 验收：`cargo check -p phoenixrs --no-default-features`；分别 `--features tls|websocket|sse|auth|jwt|password|metrics` 及组合；无 feature 时 dep tree 不含 rustls/tungstenite/jsonwebtoken/argon2；`cargo test --workspace --locked` 全绿；blog 启用 `sqlite,password`。
- 产物：各底层 crate features + `crates/phoenix/{Cargo.toml,src/lib.rs}` + scaffold + ADR-042 + 文档同步
- 发布：`@c9ffe1b` 已 push GitHub + GitCode；GitHub Release [v0.1.3](https://github.com/MageGojo/Phoenix-rs/releases/tag/v0.1.3)；crates.io：`phoenixrs 0.1.3`、`px-cli 0.1.4`、相关 crate `0.1.1`/`phoenix-view 0.1.2`
- 状态：已完成@c9ffe1b

## 2026-07-24：无数据库应用 `px release` + `px update` 核心刷新

- 根因：无 DB 的 `px new` 不生成 `src/bin/phoenix-manage.rs`，但 `px release` 仍 `--bin phoenix-manage`
- 修复：`phoenix-release` 将 manage 视为可选；CLI 仅在源文件存在时编译/打包
- 新增：`px update` 只刷新框架核心（lib/main、Vite/TS、schemas、依赖钉扎、可选 manage），不改业务代码
- 验收：`cargo test -p phoenix-release -p px-cli --lib`；无 DB 项目可 `px release --tarball`；`update_core` 单测通过
- 版本：`phoenix-release 0.1.2`、`px-cli 0.1.6`
- 状态：已完成@a8a8cc5（GitHub/GitCode + Release v0.1.6 + crates.io `phoenix-release 0.1.2` / `px-cli 0.1.6`）

## 2026-07-24：脚手架渲染模式可切换 + px new 默认可配置

- 内容：`Page::respond_with_renderer` SPA 短路；`px new` / scaffold 支持渲染模式切换与默认可配置项
- 仅发布变更 crate：`phoenix-view 0.1.3`、`px-cli 0.1.5`
- 发布：`@b6adb6a` push GitHub + GitCode；Release [v0.1.5](https://github.com/MageGojo/Phoenix-rs/releases/tag/v0.1.5)；crates.io 已上架
- 状态：已完成@b6adb6a

## 2026-07-24：release 资源模式与开发实时刷新修复

- 根因：编译后的应用仍以 `APP_ENV=development` 判断资源模式，生成 `/assets/phoenix.js` 而非 manifest 的 hashed JS/CSS；发布目录未启动 Vite 时入口 404。
- 修复：`px dev` 明确注入 `PHOENIX_VITE_DEV=1`；脚手架仅在该生命周期信号存在时使用 Vite dev client，其他启动方式都加载 production asset manifest。生成的 controller 同时按运行时 `AppConfig` 设置 Vite dev entry，避免 release-profile 的 debug-assertion 分支影响 URL。
- 实时刷新：监听范围加入 `views/`、`package.json`、lockfile、TS/Vite 配置；变化后重建 client + renderer 并重启 backend。
- CLI：`px new` 交互菜单改为开发者导向英文文案和一致的默认项提示。
- 验收：`cargo test -p px-cli`；重建 `px_text` release staging，HTML 引用 `/assets/client-HaMzEeQc.css` 与 `/assets/phoenix-N3wLaEhm.js`，两者 HTTP 200。
- 版本：`px-cli 0.1.9`
- 状态：已完成@4833fdb（GitHub/GitCode + Release [v0.1.9](https://github.com/MageGojo/Phoenix-rs/releases/tag/v0.1.9) + crates.io）

## 2026-07-25：release `bin/` 直接 serve + 三模式 smoke

- 根因：在 `releases/<ver>/staging/bin` 下执行 `./<app> serve` 时 cwd 仍是 `bin/`，相对路径读不到 `public/assets/phoenix-manifest.json`，表现为 `Error: Read(Os { kind: NotFound })`。
- 修复：
  1. `phoenix-console` 在常规 `<root>/bin/<app>` 布局下自动 `chdir` 到 release 根（探测 `manifest.toml` + `config/` + `public/`）
  2. `AssetManifestError::Read` 携带文件路径，便于诊断
  3. `px release` 元数据改为正确的 `public/ssr/phoenix-renderer.json`
- 新增示例：`examples/render-modes-smoke` 同项目演示 SPA(=CPA) / Islands / SSR（`/spa` `/islands` `/ssr`）
- 验收：
  - `cargo test -p phoenix-console --lib`（含 packaged root 探测）
  - `cargo test -p phoenix-view --lib` assets；`cargo test -p px-cli --lib release`
  - 重建 `px_12345` 后从 `bin/` 启动 `./px-12345 serve` → 200
  - 本地 `px release --version 0.1.0 --tarball` smoke；从 staging `bin/` 启动后三模式 header 正确，SPA 空壳 / Islands 含 counter / SSR 含完整 HTML，`/assets/phoenix-*.js` 200
  - `cargo run -- serve`（无 `PHOENIX_VITE_DEV`）三模式同样通过
- 产物：`crates/phoenix-console/src/lib.rs`、`crates/phoenix-view/src/assets.rs`、`crates/phoenix-cli/src/release.rs`、`examples/render-modes-smoke/**`
- 状态：已完成@b918aed

## 2026-07-25：render-modes-smoke 启用 SQLite 并双路径验收

- 内容：`.phoenix database=sqlite`；Cargo `sqlite` feature + `toasty`；`config/database.toml`；`phoenix-manage`；`Note` 模型/迁移；`GET/POST /notes` 真实读写；`application()` 注入 `StateMiddleware<Database>`
- 开发态验收：`px migrate` → `px dev` → CSRF + `POST /notes {"name":"dev-note-1"}` → `GET /notes` HTML 含该笔记
- 编译产物验收：`px release --version 0.1.1 --tarball`；staging 含 `bin/phoenix-manage` + `bin/render-modes-smoke`；`./bin/phoenix-manage migrate`；从 `bin/` 启动后 `POST/GET` 写入 `release-note-1`，assets 200
- 产物：`examples/render-modes-smoke/**`（含 `src/bin/phoenix-manage.rs`、`app/models/note.rs`、`routes/notes.rs`）
- 状态：已完成@d173269

## 2026-07-25：框架能力开发/产物双路径验收

- 靶场：`examples/render-modes-smoke` 默认 features 扩至 `sqlite,password,jwt,auth,websocket,sse,metrics,storage,queue,mail`；挂载 `/features/*`、`/internal/metrics`、`/hello` + `greet`（FeatureSet greeter）
- 脚本：`scripts/verify-features.sh` + `scripts/ws_ping.mjs`（同一套 curl/WS 检查开发态与 release 产物）
- 开发态：`px migrate` + `px dev` → verify → **pass=20 fail=0**（日志见 `examples/render-modes-smoke/docs/FEATURE_VERIFY.md`）
- 产物：`px release --version 0.2.0 --tarball` → staging `mkdir storage` + `phoenix-manage migrate` → `bin/render-modes-smoke serve` → 同一脚本 → **pass=20 fail=0**；`greet` → `smoke-hello`
- 旁路：`phoenix-blog-example` / `phoenix-multi-app-example` release 二进制抽测 **PASS**；`phoenix-redis` / PG / MySQL 无 env·无服务 → **SKIP**（`docs/SIDE_EXAMPLES.md`）
- 明确不做：redis/pgsql/mysql 不进 smoke 默认依赖；`testing` 不进 release；blog/multi-app 不改写成 `px release` 应用
- 产物：`examples/render-modes-smoke/{Cargo.toml,app/features/**,app/plugins/**,app/controllers/features_controller.rs,scripts/**,docs/**,README.md}`；`docs/PROGRESS.md`；`docs/工具与约定.md`
- 状态：已完成@2a67da2

## 2026-07-25：Docker 补环境自测外部依赖与缺口能力

- 新增 `docker-compose.test-services.yml`（redis:16379 / postgres:15432 / mysql:13306，对齐 CI 镜像与账号）
- 新增 `scripts/verify-external-features.sh`：起服务 → redis/pg/mysql contract → JWT refresh → auth lib → testing → runtime tls → smoke TLS debug/release
- smoke：`tls` feature + `APP_TLS_*` 走 `bind_tls`；`curl -sk https://127.0.0.1:3443/hello` 双二进制 PASS
- 实测：redis/postgres/mysql/jwt.refresh/auth/testing/runtime.tls/tls.dev/tls.release 全 PASS（见 `docs/EXTERNAL_FEATURE_VERIFY.md`）
- 清理：`docker compose -f docker-compose.test-services.yml down -v`（可选 `colima stop`）
- 状态：已完成@63b377c

## 2026-07-25：同步 GitHub / GitCode + crates.io v0.1.10

- Push：`origin` + `gitcode` → `main@b585575`
- crates.io：`phoenix-console 0.1.2`、`phoenix-view 0.1.4`、`phoenixrs 0.1.4`、`px-cli 0.1.10`
- GitHub Release：[v0.1.10](https://github.com/MageGojo/Phoenix-rs/releases/tag/v0.1.10)
- 未上传：`.env` / 证书与 sqlite / `dist/` / Docker 卷；未重发无变更 crate / npm
- 状态：已完成@57637b0

## 2026-07-27：px-cli 0.1.11 — release 打包静态资源

- 修复：`px release` 只打 `public/assets` + `public/ssr`，漏掉 `fonts/`、`images/` 等 → 生产 `/fonts/fonts.css` 404
- 改动：`StagingSources.public_static_dirs`；CLI 扫描 `public/*`（跳过 assets/ssr）一并打进制品
- 版本：`phoenix-release 0.1.3`、`px-cli 0.1.11`
- 验收：`cargo test -p phoenix-release`；`px release` 制品含 fonts/images；HEAD `/fonts/fonts.css` 200
- Push：`origin` + `gitcode` → `main@b28a4ea`；tag `v0.1.11`
- crates.io：`phoenix-release 0.1.3`、`px-cli 0.1.11`
- GitHub Release：[v0.1.11](https://github.com/MageGojo/Phoenix-rs/releases/tag/v0.1.11)
- 状态：已完成@b28a4ea

## 2026-07-26：系统教程 docs/tutorial

- 新增按序教材：`docs/tutorial/README.md`（学习路径）+ `00` + 初级 01–07 + 中级 08–14 + 高级 15–20
- 约定：先路径后章节、一章一事、验收不过不进下一级；深挖仍指向专章 docs / Skill
- 入口：根 `README.md`「系统教程」；`docs/工具与约定.md` 互链
- 验收：目录齐全；章节含上一章/下一章链接与必做/验收块
- 产物：`docs/tutorial/**`
- 状态：进行中（文档已写，待用户 commit / push）

## 2026-07-26：React 体验文档收敛 docs/REACT.md

- 新增用户向教学文档 `docs/REACT.md`：速查表 + 启动/页面/导航/hooks/表单两条路/局部更新/全局点缀/乐观更新，一页覆盖日常 90% API
- 三份 `REACT_DX_*.md` 顶部标注为实现期设计文档并指向 REACT.md
- 教程同步：06 前端调用改为 `Form` + 生成 action 示例（curl 降为对照）；04/README 增补 REACT.md 链接；05 讲解补「路由连写非强制」（分段赋值 / merge / group）
- `render_mode` 硬导航语义定稿：跨渲染模式跳转执行完整浏览器导航（与 protocol / asset_version / contract_hash 同列），`navigation.test.tsx` 硬导航用例表新增 render_mode 项，islands 托管根用例改为同模式导航；REACT.md / RENDERING.md 已同步
- 状态：完成（待用户 commit / push）

## 2026-07-26：命名路由 URL helper + 示例迁移 + captcha / pay Feature

- **前端命名路由闭环**：`@apizero/react` 新增 `urlFor` / `createRouteUrl` / `registerRouteManifest`（`{param}` 插值 + percent-encode，语义镜像 Rust `Router::url`；缺参抛错；`query` 选项拼查询串）。导航器与 SSR 渲染器自动注册当前信封的路由表，SSR/Islands/SPA 全场景可用
- `@apizero/vite` 生成器：非 action 路由改生成可调用 URL 构造器，路径参数带类型（`routes.users.show({ id })`）；`.get(变量,…)` / `format!` 等非字面量路径降级为宽松参数类型，杜绝错绑前一条路由
- 示例迁移 DX：blog `member-creator` 改用 `Form`/`FieldError`/契约字段表；members 页改 `Link` + `members.index()`；blog 依赖从 registry 0.1.0 改为 `file:` 工作区链接（消除 node_modules 嵌套旧包遮蔽）
- `render_mode` 硬导航语义定稿：跨渲染模式跳转执行完整浏览器导航（与 protocol / asset_version / contract_hash 同列），测试用例表新增 render_mode 项；REACT.md / RENDERING.md 同步
- **phoenix-captcha**（新 crate）：纯 Rust SVG 验证码，session 存 SHA-256 哈希、一次一用、大小写不敏感、常数时间比较；`CaptchaFeature`（路由 `captcha.image`）+ `CaptchaProtected` 提取器 + `captcha_format` 规则；React 侧 `CaptchaImage` / `useCaptcha`；docs/CAPTCHA.md
- **phoenix-pay**（新 crate，MVP）：`Amount` 整数分、`PaymentProvider` trait、订单状态机 + `(provider, out_trade_no)` 幂等、`MemoryPaymentStore`、`PayFeature`（notify/查询路由 + payments 迁移）、Mock 全流程；微信 Native / 支付宝当面付配置结构（密钥 Debug 脱敏），网关签名明确 `NotImplemented` 留接缝；docs/PAYMENTS.md；FEATURES.md 增两行
- 测试：`@apizero/react` 101、`react-ssr` 7、`vite` 14、blog 示例 6 全绿；`cargo test -p phoenix-captcha -p phoenix-pay` 全绿；`cargo check --workspace` 干净
- 状态：完成（待用户 commit / push）

## 2026-07-26：地基七件套（上传 / auth 脚手架 / 调度 / 分页 / i18n / 通知 / 支付网关）

- **文件上传全链路**：`@apizero/react` 新增 `uploadRust` / `toFormData`；`Form`/`useForm` 检测到 `File` 自动切 multipart（XHR，`form.progress` 0–1），CSRF/422 回填/防重复提交与 JSON action 一致；教程新增 `tutorial/intermediate/番外-文件上传.md`（Multipart<T> + LocalDisk 落盘 + 进度条）
- **`px make:auth`**：Breeze 等价脚手架——routes/auth.rs（命名路由 + `.action` 契约 + POST 组 RateLimit）、控制器（session.regenerate、防枚举重置）、三张 React 页面（Form/field/FieldError，中文文案）、CaptchaImage 注释块 + 四步启用说明；端到端验证含真实 routes 生成与 tsc
- **phoenix-schedule**（新 crate）：自研五段 cron（Vixie 语义、闰年/2100 规则）+ `every_*`/`daily_at` DSL、`schedule:run`/`schedule:work` 命令（px 转发）、进程内防重叠；phoenix-queue 增 `dispatch_in` 延迟任务；docs/SCHEDULE.md
- **分页**：phoenix-database `page_paginate`/`cursor_paginate`（归一化、越界空页、base64 游标；`paginate` 名与 Toasty 固有方法冲突故命名 `page_paginate`）；React `<Pagination>` 窗口页码组件 + `PaginatedData`/`CursorPageMeta` 类型；DATABASE.md/REACT.md 更新。注：契约生成器不支持泛型，`Paginated<T>` 需应用侧落具体 Resource（文档已示例）
- **校验消息本地化**：内置 en/zh-CN 目录 + `set_locale`/`register_locale`/单条覆盖/字段显示名；422 形状与 rule 零变化，独立进程测试保证默认输出逐字节兼容；docs/VALIDATION.md
- **phoenix-notify**（新 crate）：`Notification`/`Notifiable`/`Notifier`，mail + database 双通道（fail closed）、notifications 迁移（202607260002）+ `NotifyFeature`；docs/NOTIFICATIONS.md
- **支付真网关**：微信 APIv3（RSA-SHA256 签名/验签、平台证书自举+缓存、AES-256-GCM 回调解密、native 下单/查询/关单、±300s 重放窗口）与支付宝当面付（RSA2 拼串签名/验签、precreate/query/close、app_id 校验）；`PayHttp` 可插拔传输（hyper+rustls 默认）；假网关集成测试含「坏签名必须拒绝」负例；零新第三方依赖（ring/rustls-native-certs/x509-parser 均为 lock 既有）；docs/PAYMENTS.md 更新
- 测试：JS 四套 111/7/14/6 全绿；Rust 新增/相关 crate 180 项全绿（pay 47、schedule 42、cli 20、database 27、captcha 11、queue 11、validation 10、notify 10 等）；`cargo check --workspace` 干净；各 crate clippy(-D warnings, pedantic) 干净
- 状态：完成（待用户 commit / push）；后续清单：退款/对账、DB 版通知与订单 store、captcha 进 phoenixrs 门面、Paginated 泛型契约

## 2026-07-26：一键加密传输 + px new 体验翻新

### 加密传输（TLS 之上纵深防御，非端到端）
- 客户端 `@apizero/react`：`startPhoenix({ secure: true })` 一键；启动做 ECDH-P256 握手协商每会话密钥，页面响应走 AES-256-GCM 二进制帧（`application/vnd.phoenix.secure`），按响应头自动识别解密；会话密钥经 Web Crypto 派生为**不可导出**、不入 bundle；握手失败默认回退明文（`secureRequired` 可强制）。新增 `secure.ts` + 5 测
- 服务端 phoenix-crypto/view/runtime：`server_handshake`/`seal_frame`/`open_frame`、`SecureTransport`/`SecureCodec`、`secure_transport(routes, cfg)` 一键中间件（默认关）；帧 `PHX1|ver|issued|expires|nonce|ct+tag`，AAD=帧头++key_id，HKDF salt=key_id/info="phoenix.secure.session.v1"；密码学负例（篡改密文/AAD/nonce/错 key/过期）全测、独立客户端互操作、握手→加密页面端到端、无 secure 头逐字节回退明文
- 门面：`phoenix::secure_transport` / `phoenix::crypto::*` / `phoenix::view::SecureCodec` 可达
- 硬化附带：页面协议 JSON 响应加 `Cache-Control: private, no-store` + `Vary: x-phoenix-page, accept`，防 bfcache/共享缓存把软导航 JSON 当文档还原（blog 测试同步更新）
- 诚实边界（写在 SECURE_TRANSPORT.md 最前 + REACT.md）：浏览器为渲染必须能解密，无法对终端用户隐藏内容；本轮只加密响应体，请求体方向列 phase 2
- 文档：docs/SECURE_TRANSPORT.md（新）+ REACT.md §6.5

### px new 体验翻新
- **镜像修复**：npm 依赖从 registry.npmjs.org tarball 直链改语义化版本（走用户镜像），Cargo 同理；清查确认产物无其它外链
- 中文分步向导（渲染模式/数据库/Tailwind/Feature 多选/git → 汇总 → 分组进度 → 下一步），NO_COLOR/非 TTY 降级；npm install 失败给镜像提示不 panic
- 三渲染模式各一极小 demo：home（Link 导航）+ demo/spa（计数器）+ demo/ssr（首屏含数据）+ demo/islands（counter 岛），每页 ≤40 行
- 配置收敛：删 config/app.toml、database.toml、schema、taplo.toml；启动/前端/数据库进 .env（分组注释）；config/ 只放 Feature TOML；数据库探测改读 .phoenix / .env DATABASE_URL；新增 `phoenix_config::load_feature_config`（读 config/<name>.toml + ${VAR} 环境占位）
- Feature 可选装配：phoenixrs 门面加 captcha/pay/notify feature；`px new --feature captcha,pay,notify` 或交互多选 → 依赖 features + FeatureSet 装配代码 + config/*.toml
- 瘦身：默认产物 41→37 文件
- 端到端验收：全 feature / --no-features 两组合 cargo check + tsc + 路由生成通过

### 全量门禁
- `cargo test --workspace` 无失败；`cargo clippy --workspace --all-targets -D warnings` 干净
- JS：@apizero/react 116、react-ssr 7、vite 14、blog 示例 6 全绿
- 状态：完成（待用户 commit / push）；后续：请求体方向加密（phase 2）、退款/对账、captcha DB store、registry 发布含新 feature 的 phoenixrs

## 2026-07-26：后续清单收口（S3 修复 + captcha DB store + Paginated 泛型 + 退款对账 + 请求体加密 + Redis 广播）

把此前几条记录末尾的「后续清单」全部做完。**动手前先跑基线，发现工作区其实编译不过**：`phoenix-storage/src/s3.rs` 调用了不存在的 `self.sanitized(key)`（5 个错误），先补上 `sanitized_key()`（`PathBuf` → 始终 `/` 分隔的对象键，不用平台分隔符）才有可验证的起点。

### captcha DB store
- 新增 `CaptchaStore` trait（`insert` / `take` / `purge_expired`）+ `MemoryCaptchaStore` + `DbCaptchaStore`（Toasty，`CaptchaRow` + `captchas` 迁移 `202607260003`）
- **一次一用是原子 claim**：DB 版先读行，再用 `DELETE … WHERE id = ?` 认领并检查影响行数——影响 0 行说明别人先拿走了。读完就无条件相信读到的行会让重复提交花掉同一个挑战两次
- session 流原样保留；`CaptchaFeature::with_store` 才注册 `captcha.challenge`（JSON `{id, svg, expires_in}`）与迁移，不用 store 的项目零变化
- `CaptchaConfig::ttl`（默认 5 分钟，1 秒–1 天）；挑战 id = 128 位 CSPRNG 十六进制
- React：`useStoredCaptcha` / `StoredCaptchaImage`，SVG 以 `data:` URL 内联（服务端字符串**从不**作为 HTML 注入 DOM）
- 顺带：`phoenix_database::Backend::placeholder(n)` 与 `Database::table_name()`（裸 SQL 与 ORM 的表前缀站同一侧）

### Paginated<T> 泛型契约
- 契约生成器认识**框架自己的**泛型 `Paginated<T>` / `CursorPaginated<T>`，发射 `PhoenixPaginated<T>` / `PhoenixCursorPaginated<T>`；可写在 `.action::<_, Paginated<R>>()` 与嵌套字段里。应用自己的泛型 struct 仍然明确报错
- 未使用时不发射，contract hash 不变；应用不能再叫 `Paginated`（名字冲突直接报错）；元数据计数 `u64 → number` 是一处**明确豁免**（行数与被 clamp 的页大小，不可能到 2^53），写在生成注释里
- 两边形态由 `phoenix-database` 的 `wrapper_wire_keys_match_the_typescript_generator` 钉死

### 支付退款 / 对账
- `RefundOrder` / `RefundStatus`（独立状态机）/ `RefundReceipt` / `RefundRecord`；`payment_refunds` 表（`202607260004`），`(provider, out_refund_no)` 幂等
- `PayManager::refund` **先落库再调网关**：网关成功却在返回路上崩溃的场景有据可查。可退额 = 订单金额 − 所有「未失败」退款（在途也占额，杜绝重复提交超退）；网关报错标记 `Failed` 释放额度，行仍保留
- 订单状态机退款臂改为双向：全部退款失败则 `Refunding → Paid`（钱没动，订单确实还是已付）
- 微信 APIv3 退款 / 退款查询；支付宝 `alipay.trade.refund` / `fastpay.refund.query`（「成功但空体」= 该退款不存在，映射 `RefundNotFound`，**不是**零元退款）
- 对账：`Bill` / `BillEntry` / `Discrepancy` / `Reconciliation` + 纯函数 `reconcile`；`PaymentRecord.paid_at`（**只打一次戳**）+ `PaymentStore::paid_within` 提供本地侧。**双向**比对（账单有我们没有 / 我们有账单没有 / 金额或状态不符）
- 微信账单：签名票据 → 签名 GET 取 CSV，**先校验票据公布的 SHA1 摘要再解析**；支付宝账单是 ZIP，只暴露 `bill_url()`，`download_bill` 保持 `NotImplemented`（不为一个字段引入解压依赖）
- `parse_bill_csv` 认中英文表头、剥微信反引号；**未知状态值报错而不是当成已付**——那正是对账要抓的错
- 时区不在本 crate：`day_start` 由调用方给

### 请求体方向加密（原 phase 2）
- `PHX1` 帧新增**方向绑定**：`AAD = 帧头 ++ key_id ++ ("req"|"res")`。没有它，同一会话密钥下请求帧与响应帧是可互换的密文，抓到的响应能原样回放成请求体并通过认证；Rust 与 TS 两侧各有一条跨方向回放必败的用例
- 中间件在 **handler 之前**解帧、还原 `Content-Type`（随 `X-Phoenix-Content-Type` 上行）、清掉帧标记，`Json<T>` / `Validated<T>` 无感
- **失败一律关闭**：篡改 / 错密钥 / 跨方向 / 截断 / 过期 / 无会话一律 400，超 `max_request_frame` 在解密前 413；未标记加密的请求逐字节原样通过
- 客户端 `sealRequest` 在会话过期时返回 `null`，调用方回退明文而不是发一个服务端注定打不开的帧

### 跨实例广播 RedisBroadcaster（原 phase 2）
- `phoenix-redis::RedisBroadcaster`：`publish` = Redis `PUBLISH`（I/O detach，不阻塞 Hub），`subscribe` = `SUBSCRIBE` 流（断线 1s 退避重订阅）
- `PeerFrame` 目标改为 `PeerTarget::{Channel, Key}`，新增 `Hub::send_to_key`。**定向发送按身份不按连接 id**：`ConnectionId` 是每个 Hub 自己的句柄，在另一节点上没有含义，而一个用户常在多台实例上有多条连接
- `HubId` 不再是裸自增（混入进程启动时间与 pid）：两个进程都从 `1` 开始会让彼此的帧被当成自己的回声丢掉
- 线上 JSON 格式显式写死、不跟随内存类型重构；控制帧不跨实例转发；诚实标注 pub/sub 是 fire-and-forget，本地送达从不依赖 Redis

### 顺带修好的既有缺陷
- `phoenix-storage` 编译失败（见开头）+ 全仓 `duration_suboptimal_units` 等 clippy 违规
- 两处**过时的测试期望**：`make:auth` 的迁移计数（一条迁移在注册表里出现 3 次：标记注释 / `pub mod` / `all()` 条目，断言写的 2）、命令面测试的迁移条数（`make:auth` 后是 3 条）。改成断言真正的不变量（只注册一个模块、只有一个迁移文件）而不是数子串

### 全量门禁
- `cargo test --workspace` 86 个测试二进制全绿；`cargo clippy --workspace --all-targets` 零告警；`cargo fmt --all --check` 干净
- JS：@apizero/react 128、react-ssr 7、vite 18、blog 6 全绿；`npm run ci:node`（含 typecheck + client/SSR 生产构建）通过
- 真实 Redis：`PHOENIX_TEST_REDIS_URL=… cargo test -p phoenix-redis --test broadcast_contracts` 4 项全绿
- 状态：完成（待用户 push）；后续（已在下一条收口）：退款异步通知、支付宝账单 ZIP 解压、加密传输会话表跨进程共享

## 2026-07-26：最后三项后续收口（退款回调 / 账单解压 / 跨进程加密会话）

### 微信退款异步通知
- 退款回调**走独立 URL**，且 URL 是每笔退款请求带上去的，所以配置项（`refund_notify_url`）、路由（`pay.notify.wechat.refund`）都独立
- `verify_refund_notify`：先验签解密再取字段；`event_type` 必须是 `REFUND.*`——投到退款路由的**支付**回调直接拒绝（两种 resource 结构不同，混用会把支付事件写进退款记录）
- `PayManager::handle_refund_notify` 幂等（网关会重投）；**回调金额与库里那笔对不上直接报错**，不静默记录；只报「仍在处理」的回调被确认但不算迁移；落地后同步订单状态
- `REFUND.ABNORMAL` → `Failed`：它表示钱**没有**退出、需人工处理，挂 pending 等于永远等不到结果

### 支付宝账单 ZIP 解压
- 自研最小 ZIP 读取器（中央目录 + 本地头 + stored/DEFLATE，无加密/ZIP64/分卷）：所有长度对缓冲区边界检查，**解压总量先封顶再解**（256 MiB），声称膨胀到 1 TB 的账单是被拒绝而不是被尝试
- DEFLATE 用 `flate2`——它本来就在 lock 里（MySQL / Redis 驱动带的），没有引入新的第三方 crate
- **GBK 不转码**：转码需要一张本 crate 不该携带的编码表。改为**在字节层匹配**表头与状态值（每个别名同时带 UTF-8 与 GBK 两种拼写），而真正读取的列在两种编码下都是 ASCII；`parse_bill_csv_bytes` 是驱动入口
- ZIP 里明细与汇总两个成员**不靠文件名猜**（文件名同样是 GBK）：每个成员都喂给解析器，取行数最多的那个
- 假网关测试用真实 GBK 字节 + 真实 ZIP 结构，端到端验证

### 加密传输会话跨进程共享
- 抽出 `SecureSessionStore` 接缝（`MemorySecureSessionStore` 默认 / `RedisSecureSessionStore`）；查表可能跨网络，故中间件把会话解析移进 async 块
- **诚实标注：这里存的是密钥material**，不是普通缓存。能读 `phoenix:secure:*` 就能解密所有在线页面会话的流量——走 TLS + AUTH、优先关闭持久化、`session_ttl` 保持短。不需要实例互换时，**粘性路由 + 进程内存储仍是保证更强的一档**
- 明确不做「再套一层主密钥加密」：会把主密钥分发变成新的最弱环节，且不改变「能读 Redis 就能解密」这个事实
- 存储读不出来时按会话不存在处理（fail closed），不退回明文
- 真实 Redis 契约测试：在 A 握手、请求打到 B 仍拿到密文；并用进程内存储做**反例对照**（不共享，所以那种部署必须粘性路由）

### 文档 / 教程 / README
- PAYMENTS / SECURE_TRANSPORT / REDIS / FEATURES 更新；README 特性一览与文档索引补齐业务型 Feature、实时、加密传输、调度、i18n
- 教程新增选修 `advanced/番外-业务能力速览.md`（五个业务能力各自解决什么、**不**解决什么）；13 章修掉已删除的 `database.toml` 陈述

### 全量门禁
- `cargo test --workspace` 全绿；`cargo clippy --workspace --all-targets` 零告警；`cargo fmt --all --check` 干净
- JS 159 项全绿
- 真实 Redis：广播 4 项 + 跨进程加密会话 3 项全绿
- 状态：完成（待用户 push）；后续：邮件真实 SMTP、队列生产驱动、服务端 partial props 求值、正式安全评审

## 2026-07-26：模型写法极简化 + 批量造数据

### `#[phoenix::model]`：关系只写「关联哪个模型」
- Toasty 能表达一切关系，但要求把外键字段、`key = …`、`references = …` 全写出来。95% 的情况永远是「这行属于那行，按 `<名>_id` 关联」，这个属性就补这 95%
- 自动补：表名（类型名 → snake_case → 复数）、`#[key] #[auto] pub id: i64`、`#[derive(Debug, Model)]`、`#[belongs_to]` 的外键字段与映射
- **每一条都能单独接管**：写了 `#[table]` / 自己的 `#[key]` / `#[belongs_to(key = …)]` / 外键字段本身，宏就不碰那一部分；可空关系（`Deferred<Option<T>>`）自动给可空外键
- `has_many` / `has_one` **原样透传**：配对由对方的 `belongs_to` 推断，没有约定可加。单向关联天然支持
- 展开就是普通 Toasty 模型，没有暗门——复合外键 / `pair` / `via` 照写不误
- 边界写在文档最前：自动键类型是 `i64`，用别的键类型必须自己声明外键——宏看不到对方模型的键类型，猜错会变成 Toasty derive 内部一个难懂的类型错误

### CLI：选类型 + 选模型
- `px make:model Post --belongs-to=User --has-many=Comment --migration --factory`
- 三种关系是枚举（`RelationKind`），不是自由文本；`--belongs-to` 不给模型名直接报错并给出用法

### 批量造数据（仅开发/测试）
- `phoenix::factory!` 宏 + `Factory` / `FactoryWith<A>` trait + `Seeder`；闭包返回 Toasty create builder，宏负责执行——**这样 builder 的生成类型永远不用被命名**
- `create_with` 接一个参数（通常是父模型主键），`px make:model --belongs-to=X --factory` 生成的工厂自动带上这个参数
- Faker 自研、无新依赖：可 seed（`Seeder::seeded(2026)` 让失败的 fixture 可重放）、`unique_email` / `unique` **靠单调计数器而不是随机**（随机局部名几千行就撞，报出来是莫名其妙的唯一约束错误）、支持 zh-CN 姓名（姓在前无空格）而用户名邮箱始终 ASCII
- **生成的联系方式打不通**：邮箱只落 example.* / test.local，手机号固定 `1380013xxxx` 文档段——测试数据不该能触达真人
- **两道闸门**，因为任何一道单独都不够：Cargo feature `factory`（不开就不编译）+ `Seeder::new` 运行时拒绝 production/prod/staging（feature 可能被 `--all-features` 顺手打开）。环境判定抽成纯函数——2024 edition 改环境变量是 `unsafe`，本工作区 forbid

### 验证
- 宏 9 项单测（约定 / 覆盖 / 可空 / 透传 / 错误参数）
- 门面 6 项集成测试：真实 SQLite 上跑通关系、可空外键、父子播种、固定种子可复现、locale
- CLI 生成的项目**真的 cargo check**：开 / 不开 `factory` 两种组合都编译通过

### 全量门禁
- `cargo test --workspace` 88 个测试二进制全绿；clippy 零告警；fmt 干净

## 2026-07-26：教程回填新写法 + `px seed` 修好一个静默失败

### 教程更新（老章节回填）
- **03 项目地图**：补 `database/seeders/`（播种入口 + 工厂，仅开发/测试）
- **09 模型迁移与 CRUD**：必做步骤改成新写法——先看生成的模型有多短（表名/主键/derive 都是约定），再加一步 1.5「加一个关联」（两条命令，`Note` 里只多一行 `#[belongs_to]`），新增第 5 步「批量造点数据」（两道闸门、唯一列为什么用计数器、固定种子可重放）
- **13 Feature 与测试**：测试需要成批数据时用工厂而不是手写十几个 `create!`；固定种子让失败用例可原样重放
- 15–20 与「番外 · 业务能力速览」在上一轮已随支付回调 / 账单解压 / 加密会话共享一并刷新，本轮无需再动

### 顺手修掉一个静默失败
写教程时发现：`px seed` 直接 `cargo run --bin phoenix-manage -- seed`，**不带 `--features factory`**。工厂文件是 `#![cfg(feature = "factory")]`，于是被整体编译掉——播种「成功」，一行都没插。这种错最难查，因为没有任何错误信息。

改成：`px seed` 读项目 Cargo.toml，声明了 `factory` 就自动带上。**不能无条件加**——工厂出现之前生成的项目没有这个 feature，而未知 feature 对 Cargo 是硬错误，会把 `px seed` 直接搞坏。测试两条路径都钉住（有 feature 带上、没 feature 回退）。

### 全量门禁
- `cargo test --workspace` 88 个测试二进制全绿；clippy 零告警；fmt 干净
- `npm run ci:node` 全绿（128 + 7 + 18 + 6 测试、typecheck、client/SSR 生产构建）
- 状态：完成并已 push

## 2026-07-27：打包产物禁止泄漏 Vite 开发 origin

- 根因：部分应用在已挂生产 manifest 后仍按 `AppConfig::vite_dev_url()`（`APP_ENV!=production` 即有值）覆盖 `<script type="module">`，导致 `px release` / staging 在 `.env` 含 `VITE_DEV_URL=http://127.0.0.1:5173` 时把开发机地址写进 HTML。
- 修复：
  1. `phoenix-view`：`default_script_src` 仅在 `PHOENIX_VITE_DEV` 时读 `VITE_DEV_URL`
  2. 脚手架 / smoke：manifest 优先；**仅** `PHOENIX_VITE_DEV` 覆盖 Vite HMR；`px dev` 仍可加载 hashed CSS 防 FOUC
  3. 文档：`CONFIG.md` / `RELEASE_PIPELINE.md` 明确「打包 ≠ `APP_ENV`，资源生命周期看 `PHOENIX_VITE_DEV`」
- 验收：`cargo test -p phoenix-view --lib`；`cargo test -p px-cli --lib scaffold::`；my_blog staging `/login` 在 `APP_ENV=development` + `VITE_DEV_URL=…5173` 下仍输出 `/assets/phoenix-ubIx1D6k.js`，无 `5173`
- 版本：`phoenix-view 0.1.5`、`px-cli 0.1.12`
- 状态：已完成@378b312
