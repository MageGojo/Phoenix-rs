# React 渲染模式

## 1. 产品目标

Phoenix 的 React 层支持 SPA、SSR 和 Islands。三种模式共享页面查找、Rust/TypeScript 契约、路由、控制器、验证错误、资源清单和安全序列化，开发者不需要为每种模式重新编写一套后端接口。

默认模式是 Islands，单次页面响应可以显式覆盖：

```rust
Page::new("dashboard/show", props).spa();
Page::new("articles/show", props).ssr();
Page::new("docs/show", props); // 默认 Islands
```

当前实现由页面响应显式覆盖默认值。路由级和应用配置级覆盖留给后续配置层；生产环境不允许因为 renderer 故障静默改变语义。

## 2. 模式对比

| 模式 | 首屏 HTML | 浏览器执行 | 适合场景 | 运行要求 |
| --- | --- | --- | --- | --- |
| SPA | 应用壳与初始页面数据 | Hydrate/Render 整个应用，后续局部导航 | 后台、工作台、强交互应用 | Rust 服务 + 静态资源 |
| SSR | React 在服务端生成完整 HTML | Hydrate 整个页面，后续可局部导航 | SEO、内容站、电商详情页 | Rust 服务 + 持久 JS renderer |
| Islands | 服务端生成完整 HTML | 只激活标记的交互岛 | 文档、博客、内容为主且局部交互 | Rust 服务 + 支持 Islands 的 JS renderer |

Islands 不是 React Server Components 的别名。P1 先实现独立 hydration root 的 Islands；RSC 涉及不同协议、打包和缓存模型，不进入当前承诺。

## 3. SPA

流程：

```text
Hyper 请求 -> 控制器 -> PageEnvelope -> HTML shell + props
                                  -> React 启动并渲染页面
后续 Link/Form -> 页面协议请求 -> 替换页面组件与 props
```

要求：

- 浏览器禁用 JavaScript 时可以显示明确的降级内容，但不承诺 SPA 功能可用。
- History、滚动恢复、加载状态、错误边界和资源版本刷新由 `phoenix-react` 管理。
- SPA 不要求生产环境运行 Node.js，适合显式选择为强交互页面模式。

## 4. SSR

流程：

```text
Hyper 请求 -> 控制器 -> PageEnvelope -> renderer 池
                                      -> React HTML + head + hydration data
                                      -> Hyper 流式/完整响应
浏览器 -> hydrateRoot -> 后续 SPA 导航
```

当前实现是一个长期运行的单 worker Node.js renderer，而不是每个请求启动 Node 进程。Phoenix 使用按行 JSON 的版本化内部协议传入完整 `PageEnvelope`、URL 和 locale；`asset_version` 与 `contract_hash` 随信封传递。子进程启动时完成协议握手，渲染中退出会自动重启并重试一次。

单 worker 同时也是明确的第一版并发上限。请求等待渲染槽位和 Node 响应共用 2 秒 deadline，超过后返回 `503 Service Unavailable`，不会静默切换为 SPA。应用可以通过 `PHOENIX_SSR_ENTRY` 覆盖服务端 bundle 路径。

SSR 必须定义：

- renderer 超时、崩溃、重启、并发上限与背压。
- React streaming 能力与 Hyper response body 的连接方式。
- `Head`、HTTP 状态码、重定向和错误页的合并规则。
- hydration 数据的安全编码与 CSP nonce。
- 浏览器与服务端输出不一致时的诊断信息。
- 无 Node 生产环境可以显式选择 SPA；SSR 部署不能继续承诺“只有一个 Rust 二进制”。

Renderer 接口保持可替换，后续可以评估嵌入式 JavaScript runtime 或远程 renderer，但 P1 不同时维护多个实现。

## 5. Islands

页面仍是 `.tsx`，普通组件只输出服务端 HTML；只有通过 Phoenix API 标记的组件会生成浏览器入口和 hydration 描述：

```tsx
import { island } from "@phoenix/react/islands";
import { SearchBox } from "../components/search-box";

const SearchIsland = island(SearchBox);

export default function DocsPage({ article }: DocsPageProps) {
  return (
    <main>
      <article>{article.body}</article>
      <SearchIsland source="docs" />
    </main>
  );
}
```

Vite 插件为每个 island 生成稳定 ID 和独立入口。服务端 renderer 返回页面 HTML 与 island 描述；浏览器只下载并激活页面实际使用的 islands。

要求：

- island props 必须实现独立契约并安全序列化。
- island 不能依赖未显式声明的父级 React Context；跨 island 状态通过 URL、服务端、事件或显式共享 store 处理。
- 相同 island 在一页出现多次时，每个实例有稳定且不冲突的 hydration key。
- 没有交互的组件不进入客户端 bundle。
- island 内部可使用 hooks，服务端专用代码不能被打入浏览器。

## 6. 统一页面协议

三种模式共享 `PageEnvelope`：

```json
{
  "protocol": 1,
  "render_mode": "spa",
  "page": "users/show",
  "props": {},
  "shared": {},
  "errors": {},
  "flash": {},
  "contract_hash": "...",
  "asset_version": "...",
  "request_id": "...",
  "islands": []
}
```

`islands` 仅在 Islands renderer 输出时存在实际内容。页面业务 props 的 JSON 语义不因渲染模式变化。

## 7. 构建产物

Vite 至少生成：

- SPA 浏览器入口与页面 chunks。
- SSR 服务端 bundle。
- Islands 服务端 bundle、island 浏览器入口与 island manifest。
- 资源 manifest、页面 manifest 和契约 hash。

构建器对服务端专用模块和浏览器专用模块设置明确边界。导入 Node/Rust 服务端能力的模块进入客户端 bundle 时必须构建失败。

## 8. 缓存与安全

- SSR/Islands HTML 的缓存键必须包含 URL、locale、认证可见性、资源版本和应用声明的 vary 维度。
- 默认不缓存带认证用户私有 props 的 HTML。
- 页面和 island hydration JSON 使用上下文安全编码，不通过字符串拼接写入脚本。
- renderer 被视为受限内部组件，不接触数据库凭证、会话密钥或任意环境变量。
- SSR 请求不得执行来自用户输入的模块路径；页面与 island ID 只能来自构建 manifest。

## 9. 分阶段交付

1. 当前：统一 `PageEnvelope`、三种渲染语义、局部导航、React 启动器、持久单 worker renderer、超时、崩溃恢复，以及成员目录逐岛 hydration 验证。
2. 下一步：Vite 页面发现、生产 manifest、多 worker renderer 池、Head 和结构化错误处理。
3. 稳定前：独立 island 入口、bundle 预算、缓存、CSP nonce 和部署验证。
4. 1.0：三种模式的部署文档、性能基线、安全测试和同页面契约一致性测试。

## 10. 验收标准

- 同一控制器和 Props struct 可以切换 SPA/SSR，React 页面无需复制。
- SSR 首屏 HTML 包含业务内容，hydration 后不产生不匹配警告。
- Islands 页面只下载实际出现的交互岛代码，不下载完整页面应用包。
- renderer 不可用时按配置快速失败或显式降级，不悬挂 Hyper 请求。
- 三种模式返回相同状态码、验证错误、flash 和业务 props 语义。
