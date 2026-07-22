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

## ADR-007：在目录和公共 API 稳定后提供代码生成 CLI

- 状态：已接受
- 决定：早期切片先延后生成器；目录、路由、Request/Resource 契约和迁移 API 验证后，正式提供 `px new` 与 controller/model/migration/request/resource/middleware/page/island 生成命令。
- 原因：现在生成内容已经由真实外部项目编译、TypeScript、生产构建和 HTTP 运行时验收固定，不再是过早固化猜测。
- 命令名：公开二进制统一为 `px`，不同时维护 `phoenix` 别名；Phoenix crate、npm package 和框架品牌名不受影响。
- 边界：CLI 只更新显式 `<phoenix:...>` 托管区块，默认拒绝覆盖业务文件；`--force` 必须由开发者明确选择。迁移 SQL 和业务查询仍需按实际需求修改。

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
- 剩余验证：流中错误语义与 hydration 诊断；Head、CSP nonce 和部署指标导出已验证。

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
- 边界：`SessionMiddleware::new(SessionStore, ...)` 保留单进程兼容路径；多实例部署使用 `SessionMiddleware::distributed` 注入原子共享 backend。内置 memory backend 是 contract reference，不跨进程；TLS 终止与可信 scheme 仍由部署层负责。

## ADR-025：约定路由在编译期发现，开发进程按组回收

- 状态：已接受
- 决定：`mount_routes!()` 在编译期按文件名排序发现 `routes/*.rs`，每个文件统一导出 `routes()`；resource/alias/model binding 是普通 Rust builder 与 middleware，不引入运行时反射。`px dev` 在 Unix 上给 Rust/Vite 分配独立进程组并以 TERM/KILL 两阶段回收。
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

## ADR-026：内置监听器默认自动服务 HTTP/1.1 与 HTTP/2

- 状态：已接受
- 决定：使用 hyper-util `server::conn::auto` 在同一 TCP 监听器识别 HTTP/1.1 与 HTTP/2 preface；应用可显式限制为单一协议。
- 原因：默认升级不破坏现有 HTTP/1.1 应用，同时让反向代理或受控内网客户端直接使用 HTTP/2。
- 边界：明文自动识别不是 TLS/ALPN。HTTPS 证书、ALPN、cipher policy 与外部终止配置必须由独立 TLS/部署层完成。

## ADR-027：日志初始化与 HTTP 访问事件保持分层

- 状态：已接受
- 决定：`phoenix-logging` 只安装和配置 tracing subscriber；`phoenix-security::AccessLog` 负责产生经过隐私约束的请求事件。
- 原因：输出目的地/格式与业务事件是不同职责，分层后应用可以替换 collector 而不复制 HTTP 脱敏规则。
- 边界：默认日志不得记录 query、Header 值、Cookie、Authorization、JWT 或业务 payload。需要额外字段时由应用显式添加并承担脱敏责任。

## ADR-028：JWT、可逆加密和密码哈希使用独立 API

- 状态：已接受
- 决定：JWT 首版固定 HS256 并强制 `kid`、256 bit 最小 secret 与标准 claim 校验；应用数据使用 AES-256-GCM 和关联数据；用户密码使用 Argon2id PHC string。
- 原因：JWT 是签名容器而不是密文，密码哈希也不应被包装成可逆加密。独立类型能减少开发者误用，并让轮换和验证策略显式可测。
- 边界：JWT 不存放秘密业务数据且不得记录到日志；AES-GCM key 和 JWT secret 必须从仓库外注入；密码重置使用一次性短时 token，不解密旧密码。

## ADR-029：多应用在编译路由后组合，不共享可变全局上下文

- 状态：已接受
- 决定：每个 `ApplicationModule` 独立应用路由 scope、中间件和 State，再由组合 Router 按 Host/路径选择；组合层汇总只读命名路由表并注入 `ApplicationContext`。
- 原因：官网、前台和后台可以复用框架进程与公共库，同时避免用一个可变全局“当前应用”造成并发串线。独立 Router 也允许不同 Host 上存在相同相对路径。
- 边界：应用模块不是进程级安全沙箱；认证、授权、数据库权限与 secrets 仍需分别配置。相同 selector、应用名或全局命名路由在启动时失败。

## ADR-030：开发宏必须展开为稳定 builder API

- 状态：已接受
- 决定：`routes!` 与 `applications!` 只消除静态重复声明，展开目标分别是 `Routes` 和 `ApplicationModule` builder；不为控制器、中间件或依赖注入创建第二套运行时模型。
- 原因：宏能加速常见路径，同时普通 Rust API 保持可调试、可组合和 IDE 可导航。动态条件不适合塞入 DSL。
- 边界：新宏必须有 doctest 和展开行为测试；宏错误不能吞掉底层类型错误。复杂逻辑应退回 builder，而不是继续扩张语法。

## ADR-026：应用状态与页面外围协议使用显式强类型 API

- 状态：已接受
- 决定：应用依赖由 `StateMiddleware<T>` 放入 Request extensions，并通过 `State<T>` extractor 进入控制器。受控页面元数据与 CSRF 分别使用 `PageHead` 和 `PageEnvelope.csrf_token`；React action 自动转发 token。重定向和文件响应使用 `Redirect`、`Download`，不鼓励业务手写敏感响应头。
- 原因：数据库、配置和客户端需要可测试的显式依赖边界；SEO、CSRF 和下载头同时涉及 HTML/HTTP 上下文，集中在框架能避免每个应用重复实现转义与注入防护。
- 边界：`State<T>` 是显式类型映射，不是运行时服务定位器；`PageHead` 不允许任意 HTML；`Download` 只负责响应安全，文件授权仍由应用控制器完成。

## ADR-031：授权默认拒绝，refresh token 一次性轮换

- 状态：已接受
- 决定：RBAC 使用精确权限和启动时编译的角色继承图，ABAC policy 与 RBAC 采用 deny-overrides；没有 allow 时默认拒绝。refresh token 是只持久化 hash 的 opaque secret，每次使用原子轮换；重复使用撤销整个 family，access token 通过 `jti`/`sid` 检查撤销状态。
- 原因：精确 capability 和失败关闭语义避免通配符或未知角色意外扩权；一次性 refresh rotation 能把并发重放转化为可检测的 family 泄露事件。
- 边界：`FileTokenStore` 只适合单进程低吞吐持久化。多实例后端必须原子实现 `rotate_refresh`，store 不可用时 stateful 认证失败关闭；审计事件不得包含 token、资源正文或敏感属性。

## ADR-032：指标 vocabulary 固定并由 Prometheus 跨实例聚合

- 状态：已接受
- 决定：Phoenix registry 只暴露固定 method、status class 和 outcome labels；HTTP middleware、TCP/TLS server 和 renderer 使用同一进程 registry，数据库与队列通过固定枚举 hook 记录。多实例由 Prometheus 按 target 抓取和聚合。
- 原因：任意字符串 label 会让路径、用户、错误或 token 形成高基数时间序列和隐私泄露；进程内原子 counter/gauge 不需要请求路径上的网络依赖。
- 边界：指标端点必须受内部网络或管理应用保护。registry 不存储全局分布式 gauge，renderer snapshot 需要显式刷新，数据库/队列适配器负责在真实操作边界调用 hook。

## ADR-033：分布式限流由 backend 原子决定并默认失败关闭

- 状态：已接受
- 决定：`RateLimitBackend::hit` 必须在单个原子操作中完成窗口初始化/过期、递增和 allow/retry 决策；middleware 默认使用可信客户端 IP key，backend 故障返回 503，只有显式配置才失败开放。
- 原因：应用层“读取计数再写回”在多实例并发下会丢失递增；静默失败开放会在存储故障时移除安全边界。
- 边界：内置 memory backend 只验证共享语义，不跨进程。生产 Redis/数据库适配器必须使用脚本、事务或等价原子原语，并限制 key 长度/基数和 TTL。

## ADR-034：分布式 Session 使用版本化 CAS 与原子 ID 旋转

- 状态：已接受
- 决定：共享 Session backend 对 create/save/rotate/delete 返回明确 collision/conflict/missing 结果；业务写入携带读取版本，登录/权限变化把旧 ID 到新 ID 的迁移作为单个原子操作。只读 load 可延长 TTL 而不提升版本。
- 原因：最后写入者覆盖会在并行请求中静默丢失 Session/CSRF/权限状态；分步删除旧 ID、创建新 ID 会产生 fixation 或双 ID 窗口。
- 边界：memory backend 是 contract reference，不跨进程。HTTP middleware 遇到冲突或 backend 故障必须失败关闭且不得发送未持久化的新 Cookie；业务外部副作用仍需应用自行保证幂等或事务一致性。
- 验证结果：middleware 已把请求级快照接入异步 load/create/CAS save/CAS rotate/CAS delete；冲突映射为 409、存储故障映射为 503，并仅在持久化成功后提交 Cookie。共享 memory backend 的双 Router 测试覆盖跨实例读写、旧 ID 失效、删除、并行写冲突和指标。

## ADR-035：CSP nonce 属于请求文档上下文，不属于页面业务协议

- 状态：已接受
- 决定：`NonceSecurityPolicy` 为每个 Request 生成或复用经过验证的 nonce；`ResponseContext` 让普通/typed Handler 的 `IntoResponse` 在 Request 被消费后仍能安全读取该值。相同 nonce 写入 CSP Header、Vite runtime meta、stylesheet、hydration/module script 和 renderer v2 顶层请求；React 流式 SSR 把它交给 `renderToPipeableStream`。
- 原因：把 nonce 放进 `PageEnvelope` 会污染业务 props、页面导航 JSON、contract hash 和缓存键；构建时固定 Vite `html.cspNonce` 又会让多个请求复用同一值。请求元数据边界既能自动连接框架输出，也能保持 Rust/TypeScript 业务契约稳定。
- 缓存与失败边界：带 nonce 的 HTML 固定为 `private, no-store` 并移除 ETag/Last-Modified；API/页面 JSON 不携带 nonce 且保留自己的缓存策略。重复/unsafe-inline/预置 nonce CSP 在启动时拒绝，下游 CSP 冲突返回 500，renderer v1 与 v2 协议不兼容并明确失败。
- 开发边界：Vite 只接受一个显式 HTTP(S) origin，并推导对应 WebSocket source；凭据、路径、query、控制字符与非 HTTP scheme 被拒绝。第三方资源仍需应用显式审查并扩展基础 `SecurityPolicy`。
