# 技术决策记录

本文件记录需要跨阶段保留的决定。状态包括“已接受”“待验证”和“延后”。

## ADR-001：框架是 Laravel 风格，不是 Laravel 兼容层

- 状态：已接受
- 决定：复现清晰目录、控制器、请求验证、模型、迁移、命名路由和便利响应等体验，不承诺 PHP API 或运行时行为兼容。
- 原因：Rust 的类型、所有权和异步模型不同，逐字模仿会制造运行时魔法并降低可维护性。

## ADR-002：React + TypeScript 是默认视图

- 状态：已接受
- 决定：默认页面位于 `views/pages/`，使用 Vite 构建；同时允许 `.jsx`，但文档和示例以 `.tsx` 为主。
- 原因：TypeScript 能与 Rust 的类型化 props 方向配合，并为新项目提供更早的错误反馈。

## ADR-003：采用版本化页面协议连接控制器与 React

- 状态：已接受
- 决定：控制器返回页面名和结构化 props，框架生成完整 HTML 或页面协议响应。协议与具体 React 实现解耦。
- 原因：开发者不必重复建设 REST API，同时保留完整刷新、局部导航和未来其他客户端适配的可能性。

## ADR-004：Toasty 位于 Phoenix 数据库适配层之后

- 状态：已接受
- 决定：以 Toasty 为 ORM 基础，但应用示例优先依赖 Phoenix 重导出和扩展接口。
- 原因：满足产品方向，同时隔离 Toasty 在早期版本中的 API 波动。
- 验证结果：Toasty 0.8 的 SQLite CRUD、关系、事务和游标分页已通过真实集成测试；PostgreSQL 使用相同测试契约并由环境变量启用真实实例。Phoenix 只封装连接、后端元数据、迁移和测试隔离，不复制查询构建器。
- 迁移边界：Toasty 0.8 原生迁移运行时只有 apply/status，不覆盖 down、校验和或互斥锁。Phoenix 因此维护独立的 `phoenix_migrations` 状态表与执行器；模型查询仍完全使用 Toasty。

## ADR-022：迁移锁与事务按数据库能力实现

- 状态：已接受
- 决定：SQLite 在专用连接上用 `BEGIN IMMEDIATE` 包裹一次命令的全部迁移；PostgreSQL 使用固定 advisory lock，并让每个迁移在独立事务中提交。迁移定义的 SHA-256 覆盖 ID、名称和 up/down 全部语句。
- 原因：SQLite 的数据库级写锁适合整批原子执行；PostgreSQL advisory lock 可跨多个迁移事务保持会话级互斥，避免长事务持有全部 schema 变更。
- 边界：迁移 SQL 是目标数据库 SQL，不承诺同一 DDL 文本跨数据库可移植。生产 PostgreSQL 契约测试需要显式提供隔离测试库 URL。

## ADR-005：HTTP 核心直接使用 Hyper

- 状态：已接受
- 决定：Phoenix 直接建立在 Hyper 1.x 之上，自行定义路由、处理器、提取器、响应转换和中间件门面。Tokio、`hyper-util`、`http`、`http-body` 与 `http-body-util` 可作为必要的底层配套；Tower 只考虑后续互操作适配，不作为核心。
- 原因：这是项目明确的核心技术约束，并允许框架完整控制 Laravel 风格的公共 API，而不继承更上层 Web 框架的行为。
- 代价：需要自行承担路由正确性、body 限制、中间件顺序、错误映射、升级连接和协议边界的设计与测试。
- 验证条件：HTTP/1.1 与 HTTP/2 基础请求、body 上限、断连、优雅关闭、流式 body、panic 隔离和并发压力测试通过。

## ADR-006：不把发送到浏览器的数据描述为对用户加密

- 状态：已接受
- 决定：P0 依赖 TLS、服务端会话和字段白名单。可选应用层安全信封只能承诺指定威胁模型下的完整性/保密性，不能对最终浏览器用户保密。
- 原因：浏览器必须获得明文才能渲染，前端持有解密能力就不构成对用户的秘密。

## ADR-007：首版不提供代码生成 CLI

- 状态：已接受
- 决定：迁移执行能力属于 P0，但项目/控制器/模型/迁移文件生成器延后。
- 原因：先稳定文件约定与公共 API，避免 CLI 固化尚未验证的结构。

## ADR-008：默认采用显式应用状态，不做反射式容器

- 状态：已接受
- 决定：通过应用构建器、状态类型、trait 和构造器组织依赖。
- 原因：保持编译期检查、IDE 可导航性和可预测启动失败。

## ADR-009：项目工作名称需要发布前复审

- 状态：待验证
- 决定：内部暂用 Phoenix；技术预览发布前检查 crates.io、包管理器、域名、商标和与 Elixir Phoenix 的混淆风险。
- 原因：重名会长期影响发现性和社区沟通，越晚改名成本越高。

## ADR-010：Rust 是前后端数据契约的唯一来源

- 状态：已接受
- 决定：Request DTO、页面 Props、Shared Props 与 Resource 通过 `#[phoenix::contract(...)]` 自动生成 TypeScript 类型、页面映射和 action 签名；React 不重复声明相同字段接口。客户端表单规则描述属于独立后续能力，不由当前类型生成器虚假推导。
- 原因：减少字段漂移，让重命名、可选性、枚举和验证变化在构建阶段暴露。
- 边界：数据库模型不直接导出；客户端验证不代替服务端验证；页面布局和控件选择不自动生成。

## ADR-011：契约使用命名空间和方向隔离

- 状态：已接受
- 决定：当前契约 ID 使用 `namespace + name + direction`。Serde 处理后的字段名是唯一 wire name；任何命名空间、rename、flatten 或 alias 冲突都会导致构建失败。兼容性由生成内容的 `contract_hash` 和生产 manifest 握手验证；显式版本字段在出现并行协议版本需求前不进入公共 API。
- 原因：允许不同业务模块拥有同名类型，同时防止静默覆盖和敏感输入被误用为输出。

## ADR-012：React 支持 SPA、SSR 与 Islands

- 状态：已接受
- 决定：三种模式共享 `PageEnvelope`、数据契约和 TSX 页面体系；默认使用 Islands，页面可以显式切换 SPA 或 SSR。第一版协议和参考 renderer 同时覆盖三种模式，生产 renderer 池与 Vite 自动构建继续独立验证。
- 原因：覆盖后台强交互、SEO 内容页和低 JavaScript 内容站，而不复制后端业务接口。
- 边界：Islands 指独立 hydration roots，不等同于 React Server Components。

## ADR-018：页面协议加密是显式可选适配器

- 状态：已接受
- 决定：普通页面协议默认使用明文 JSON 并依赖 TLS。需要保护可信中间链路时，可以显式注入 `PayloadCodec`；内置实现使用 AES-256-GCM、随机 nonce、用途绑定、短时效和 `key_id`。初始 HTML 不加密。
- 原因：大多数应用只需要 TLS；把密钥管理和额外密文层设为默认会增加复杂度，也会制造“浏览器用户看不到 props”的错误预期。
- 边界：密钥不得硬编码或嵌入公开前端。最终浏览器需要明文才能渲染，应用层信封不能对最终用户保密。

## ADR-013：SSR 与 Islands 默认使用持久 JS renderer

- 状态：已接受
- 决定：使用可配置 worker 池的长期 Node.js renderer，通过版本化完整响应/流式分帧协议与 Rust 服务通信；不按请求启动进程。容量等待与渲染共享 deadline，I/O 故障的完整响应最多替换 worker 后重试一次。
- 原因：React 官方服务端能力首先存在于 JavaScript 运行时，持久进程能控制延迟和资源成本。
- 已验证：完整/流式 SSR、成员目录 Islands、逐岛 hydration、多 worker 并发、超时淘汰、崩溃恢复、资源版本与契约 hash 握手、健康快照、显式关闭和 hydration 数据安全编码。
- 剩余验证：Head、CSP nonce、流中错误语义、hydration 诊断和部署指标导出。

## ADR-014：首个路由器使用 matchit 作为内部路径树

- 状态：已接受
- 决定：Phoenix 自己定义路由、名称、分组、中间件和响应语义，内部使用 matchit `0.8.x` 完成路径模式匹配。
- 原因：路径树属于成熟算法问题，复用专用库可以把实现精力放在框架体验与契约上。
- 边界：matchit 类型不进入应用公共 API；Laravel 风格参数统一写成 `{user}`。

## ADR-015：第一实现切片先交付 HTTP/1.1

- 状态：已接受
- 决定：先验证 Hyper 监听、body 限制、分发与优雅关闭，HTTP/2、升级连接和 streaming 后续单独验证。
- 原因：减少第一阶段协议面，同时不虚假宣称尚未测试的能力。

## ADR-016：安全边界默认失败关闭并返回通用错误

- 状态：已接受
- 决定：错误 JSON MIME、非法路径编码和超限 body 在进入控制器前拒绝；业务 panic 被隔离并映射为不含内部细节的 500。
- 原因：边界输入不应被宽松修复后交给业务，panic 内容也不应成为客户端诊断信息。

## ADR-017：安全响应头由显式中间件提供

- 状态：已接受
- 决定：保留轻量 `SecurityHeaders` 基线，并由 `phoenix-security::SecurityPolicy` 提供可配置 CSP、HSTS 与浏览器权限策略。CORS、Host、代理、限流、Session 和 CSRF 保持独立中间件，应用按文档顺序组合。
- 原因：部分安全头依赖 HTTPS、Vite 开发模式和嵌入策略，过早硬编码会破坏正确应用。

## ADR-024：会话状态留在服务端，代理信任从 TCP peer 开始

- 状态：已接受
- 决定：默认 Session Cookie 只携带高熵随机 ID，业务值保存在 `SessionStore`；登录或权限变化使用 `regenerate()`，注销使用 `invalidate()`。转发 IP 只有在 Hyper 写入的直连 TCP peer 被显式信任时才解析。
- 原因：服务端 Session 便于撤销和敏感数据隔离；从不可伪造的连接元数据开始逐 hop 验证，避免直接相信客户端提供的 XFF。
- 边界：内置 store 是单进程内存实现，适合开发和单实例服务。多实例部署必须接入共享持久化 store；TLS 终止与可信 scheme 仍由部署层和后续中间件负责。

## ADR-025：约定路由在编译期发现，开发进程按组回收

- 状态：已接受
- 决定：`mount_routes!()` 在编译期按文件名排序发现 `routes/*.rs`，每个文件统一导出 `routes()`；resource/alias/model binding 是普通 Rust builder 与 middleware，不引入运行时反射。`phoenix dev` 在 Unix 上给 Rust/Vite 分配独立进程组并以 TERM/KILL 两阶段回收。
- 原因：编译期文件发现让缺失目录和非法路由在启动前失败，同时保留 IDE 可导航的普通 Rust 文件；进程组可以清理 Cargo/npm 派生的实际 server，避免只杀父进程留下监听端口。
- 边界：当前只扫描 route 目录第一层的 `.rs` 文件并要求固定 `routes()` 导出；CLI 默认命令面向含 `Cargo.toml` 与 `package.json` 的应用目录。

## ADR-019：浏览器调用 Rust action 使用命名路由和生成函数

- 状态：已接受
- 决定：Rust 路由器自动将命名路由映射加入页面协议；`phoenix-vite` 把绑定了 Input/Output 的 action 生成为可直接调用的 TypeScript 函数。`callRust` 保留为底层传输 API。
- 原因：React 不应依赖容易变化的后端 URL。路由路径调整时，只要稳定名称不变，前端调用无需修改。
- 边界：生成 action 是浏览器到 Rust 服务的 HTTP 封装，不是在浏览器内直接执行 Rust。当前切片只覆盖无路径参数的 POST action；参数化 URL 与其他 HTTP 方法需单独设计。

## ADR-020：Islands 使用编译期客户端指令与 SSR 自动收集

- 状态：已接受
- 决定：业务页面使用 `<Component client:load />` 标记交互边界。`phoenix-vite` 编译该指令，按目录生成页面/island 注册表和入口；SSR renderer 自动收集实际边界的组件名、ID 与 props，Rust 控制器不重复声明 island。
- 原因：让组件保持普通 React 代码，把打包、注册和页面协议元数据收回框架，同时保证 Islands 页面只加载实际使用的交互代码。
- 边界：island props 必须可 JSON 序列化，island 不允许嵌套。共享同一 React 状态的交互必须处于同一边界；SSR 模式继续整页 hydration，不创建局部 root。

## ADR-021：Rust 命名路由生成 TypeScript 属性树

- 状态：已接受
- 决定：`phoenix-vite` 从标准路由目录中的字面量 `.name("...")` 生成 `views/generated/routes.ts`。点分路由名映射为嵌套只读对象；绑定 `.action::<Input, Output>()` 的叶子生成强类型调用函数。
- 原因：裸字符串没有编辑器补全，Rust 路由重命名也不会触发前端错误。生成属性让 `members.store` 可导航、可补全，同时保留 Rust 命名路由作为唯一声明。
- 边界：生成 action 只携带稳定路由名，真实 URL 仍从 `PageEnvelope.routes` 解析。动态路由名在构建时拒绝。

## ADR-023：契约生成遵守 Serde 并对未知形态失败关闭

- 状态：已接受
- 决定：Input、Resource、Page Props 和 Shared Props 使用 `#[phoenix::contract(...)]` 标记；Vite 从 Rust 源码生成 TypeScript，并把 Serde 的 rename、rename_all、flatten、alias、default 和方向性 skip 作为 wire-name 权威规则。
- 原因：过程宏不能安全地直接向业务源码目录写文件；Vite 已经负责页面和路由发现，把生成阶段集中在同一个可观察构建入口可以免除业务 build script。
- 边界：已支持的 Rust/Serde 形态必须生成准确类型；尚未支持的数据 enum、tuple/generic struct、无法解析的嵌套类型和可能越过 JavaScript 安全范围的整数必须中止构建，不允许退化成静默 `any`。生产 client 与 renderer manifest 携带同一 contract hash，Rust worker 握手不一致时失败关闭。

## ADR-026：应用状态与页面外围协议使用显式强类型 API

- 状态：已接受
- 决定：应用依赖由 `StateMiddleware<T>` 放入 Request extensions，并通过 `State<T>` extractor 进入控制器。受控页面元数据与 CSRF 分别使用 `PageHead` 和 `PageEnvelope.csrf_token`；React action 自动转发 token。重定向和文件响应使用 `Redirect`、`Download`，不鼓励业务手写敏感响应头。
- 原因：数据库、配置和客户端需要可测试的显式依赖边界；SEO、CSRF 和下载头同时涉及 HTML/HTTP 上下文，集中在框架能避免每个应用重复实现转义与注入防护。
- 边界：`State<T>` 是显式类型映射，不是运行时服务定位器；`PageHead` 不允许任意 HTML；`Download` 只负责响应安全，文件授权仍由应用控制器完成。
