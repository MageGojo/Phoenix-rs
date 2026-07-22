# 项目与架构

## 1. 架构目标

Phoenix 采用模块化单体框架结构。应用开发者通常只依赖顶层 `phoenix` crate；内部组件保持独立，以便测试边界和替换底层实现。框架不隐藏 Rust 类型系统，而是提供更短、更一致的常用路径。

## 2. 推荐技术基线

以下是规划基线，需在第一个技术验证阶段锁定具体版本：

| 领域 | 选择 | 原因 |
| --- | --- | --- |
| 异步运行时 | Tokio | 生态成熟，与 Toasty 同属 Tokio 生态背景 |
| HTTP 核心 | Hyper 1.x + hyper-util | 用户指定；直接控制连接、请求/响应和服务边界 |
| 路由与处理器 | Phoenix 自研 | 在 Hyper 之上实现 Laravel 风格路由、参数绑定、提取器和控制器适配 |
| 序列化 | Serde / serde_json | Rust 结构化数据事实标准 |
| ORM | Toasty | 用户指定；支持 SQL/NoSQL 方向、关系与迁移能力 |
| 前端构建 | Vite | React/TypeScript 开发体验成熟，清单机制适合生产资源解析 |
| 前端 | React + TypeScript | 默认页面方案，同时允许 `.jsx` |
| 日志与追踪 | tracing | 异步 Rust 的通用可观测性方案 |
| 密码与密钥 | RustCrypto 生态的审计实现 | 禁止自制密码算法，具体 crate 经过安全评估后决定 |

当前查询到的 Toasty 公开版本为 `0.8.0`，其 crate 元数据显示需要 Rust `1.95`，并提供 SQLite、PostgreSQL、MySQL、Turso、DynamoDB 相关驱动以及 `migration` feature。版本和 API 在采用前必须通过本地 spike 验证，不能仅依赖文档描述。

## 3. 模块边界

```text
应用代码
  -> phoenix（统一导出与 prelude）
      -> phoenix-core（应用、配置、错误、生命周期）
      -> phoenix-http（请求、响应、Cookie、上传）
      -> phoenix-routing（路由、命名 URL、参数绑定）
      -> phoenix-validation（请求 DTO 与验证错误）
      -> phoenix-view（页面协议、Vite 清单、React 响应）
      -> phoenix-database（Toasty 门面、事务、分页、迁移）
      -> phoenix-security（会话、CSRF、安全头、密文信封）
      -> phoenix-testing（请求、数据库与页面断言）

浏览器
  -> phoenix-react（启动器、导航、表单、错误与共享 props）
```

### `phoenix`

稳定的应用入口，重导出常用 trait、类型和宏。公共文档与示例优先只使用该 crate，减少开发者理解内部拆分的成本。

### `phoenix-core`

负责应用构建器、服务容器边界、配置加载、环境、统一错误、启动和优雅关闭。首版不实现运行时反射式依赖注入；依赖通过显式 `AppState`、trait 和构造器连接。

### `phoenix-http`

直接在 Hyper 1.x、`http` 与 `http-body` 类型之上提供稳定门面，包括 Tokio 连接适配、请求 body 归一化、内容类型判断、大小限制、JSON/Form/Multipart 提取、Cookie、重定向、下载和统一错误映射。`hyper-util` 与 `http-body-util` 只承担连接/runtime 适配和 body 工具，不定义应用层 API。

### `phoenix-routing`

负责路由注册、分组、前缀、名称、参数约束、资源路由、处理器调用与反向 URL 生成。路由树和处理器 trait 由 Phoenix 定义，最终作为 Hyper `Service` 的请求分发入口。路由命名冲突必须在启动时失败，不允许静默覆盖。

### `phoenix-validation`

把“解析、规范化、授权、验证”组织为请求对象。验证成功后交给控制器的是强类型 DTO；验证失败映射为稳定字段错误。底层验证库可以替换，但错误协议属于 Phoenix 公共契约。

### `phoenix-view`

负责页面名解析、props 序列化、共享数据、初始 HTML、Vite 开发代理/生产 manifest、资源版本和页面错误。该模块只定义 React 无关的页面协议，React 行为由客户端包实现。

### `phoenix-database`

封装 Toasty 初始化、连接配置、模型约定、事务、分页、迁移执行与测试隔离。Phoenix 不尝试重新实现 ORM；仅补齐统一配置、错误、Web 集成和 Laravel 风格常用路径。

### `phoenix-security`

负责会话、CSRF、Cookie 默认值、安全响应头、密钥派生/轮换接口和可选安全信封。所有密码学能力使用成熟库，并由测试向量验证。

### `phoenix-testing`

提供应用测试客户端、认证态、CSRF、数据库事务、React 页面名/props、重定向和验证错误断言。

## 4. 请求生命周期

```text
连接/TLS 终止
  -> 可信代理与请求 ID
  -> 安全头、日志、panic/错误边界
  -> 会话与 CSRF
  -> 路由匹配与参数绑定
  -> 请求对象解析、授权、验证
  -> 控制器
  -> Toasty 查询/事务
  -> React 页面、JSON、重定向或文件响应
  -> 指标与结构化访问日志
```

中间件顺序属于框架契约，应由集成测试固定。应用可添加中间件，但框架必须说明哪些层之前或之后允许插入。

### Hyper 核心边界

- Hyper 负责 HTTP/1.1、HTTP/2、连接服务与请求/响应基础类型。
- Tokio 负责监听器、任务调度与优雅关闭；`hyper-util` 连接 Tokio I/O 与 Hyper runtime trait。
- Phoenix 在进入路由前把原始 body 统一为框架请求类型，避免 Hyper body 泛型泄漏到控制器签名。
- Phoenix 自己定义 `Handler`、`IntoResponse`、提取器、路由树和中间件接口，公共 API 不要求应用直接实现 Hyper trait。
- 中间件核心先围绕 Phoenix 请求/响应实现。Tower 互操作可作为后续适配器，不作为 P0 架构前提。
- 升级、流式响应、SSE 和 WebSocket 需要保留 Hyper 原始请求的受控逃生口，但不进入首个垂直切片。

## 5. React 集成

### 开发模式

- Rust 服务器处理业务路由与页面协议。
- Vite 服务器提供 HMR 和前端资源。
- Phoenix 从配置读取 Vite 地址，生成正确的开发脚本标签。
- 页面导航可先以完整文档请求实现，再增加带协议头的局部导航；两种路径必须返回同一页面语义。

### 生产模式

- `vite build` 生成带 hash 的资源和 manifest。
- Rust 应用在启动时读取并校验 manifest，生产环境缺失入口时立即失败。
- 静态资源可以由应用提供，也可由反向代理/CDN 提供；HTML 使用 manifest 中的真实路径。

### 页面发现

- 默认页面根目录为 `views/pages/`。
- 页面逻辑名 `users/show` 映射到 `views/pages/users/show.tsx` 或 `.jsx`。
- 大小写、扩展名冲突和重复页面在构建时失败。
- `views/components/` 与 `views/layouts/` 由页面正常 import，不作为服务端可寻址页面。

## 6. 数据层策略

### 模型

Phoenix 模型应保留 Toasty 的编译期能力。Laravel 式动态方法不应通过字符串或运行时反射照搬；优先提供可推导的查询构建器、关系、分页和批量操作。

### 迁移

迁移需要覆盖：

- 有序、不可重复的迁移 ID。
- schema 版本表与校验和。
- PostgreSQL advisory lock 或等价互斥机制。
- 事务内迁移（数据库支持时）。
- 明确标记不可逆迁移。
- `up`、`down`、状态查询和 dry-run/计划输出的库 API。

首版不提供“生成迁移文件”的 CLI，但必须提供可靠的迁移执行入口。是否直接复用 Toasty CLI 只在 spike 后决定；应用层迁移格式不应过早锁死。

## 7. 配置与部署

- 配置来源按“默认值 < 配置文件 < 环境变量 < 测试覆盖”合并，敏感值不进入仓库。
- 开发默认 SQLite，生产文档推荐 PostgreSQL；支持矩阵由契约测试决定。
- 生产环境要求显式 `APP_KEY`、公开 URL、数据库 URL 和可信代理配置。
- 框架输出单个 Rust 服务二进制与前端静态资源目录，不要求 Node.js 存在于运行时镜像。

## 8. 公共 API 稳定性

- 应用代码不直接依赖内部 crates 的私有类型。
- 所有公共错误使用可匹配的错误类别，同时保留错误来源。
- 宏只用于减少重复且必须产生可理解的编译错误；能用普通 Rust 表达时优先普通 Rust。
- 在 1.0 前维护 breaking change 日志，并为至少一个前一版本提供迁移说明。

## 9. 重要风险

| 风险 | 影响 | 缓解方式 |
| --- | --- | --- |
| Toasty 版本较新且 API 可能变化 | 模型与迁移门面反复调整 | 独立适配层、版本固定、先做 CRUD/关系/迁移 spike |
| 直接使用 Hyper 的实现面较大 | 路由、提取器和中间件容易出现协议或边界缺陷 | 限定 P0 功能、使用 `http` 生态类型、契约测试、模糊测试并保留受控逃生口 |
| Rust 编译错误对新手不友好 | Laravel 用户流失 | 约束泛型外露、错误包装、文档化失败示例、compile-fail 测试 |
| React 与后端双进程开发复杂 | 首次启动失败率高 | 统一配置、健康检查、明确诊断；后续再考虑 CLI 编排 |
| “加密 props”造成错误安全预期 | 敏感数据泄露 | 文档明确浏览器边界、ViewModel 白名单、默认不序列化模型 |
| 框架范围无限扩张 | 无法形成可用版本 | 以博客示例的端到端旅程作为 P0 闸门，P1/P2 独立排期 |
| 项目名称与既有框架冲突 | 搜索、商标和 crate 发布困难 | 技术预览前完成命名调查并允许重命名 |
