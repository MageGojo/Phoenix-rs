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

当前实现使用可配置数量的长期 Node.js renderer worker，而不是每个请求启动 Node 进程。Phoenix 使用版本化按行 JSON 与流式分帧协议传入完整 `PageEnvelope`、URL 和 locale；`asset_version` 与 `contract_hash` 随信封传递。每个子进程启动时校验浏览器资源版本与契约 hash，完整渲染发生 I/O 故障时会替换 worker 并重试一次。

`RendererConfig::with_workers` 设置池容量；等待 worker 和 Node 响应共用 deadline，超过后淘汰协议状态不确定的 worker并快速失败，不会静默切换为 SPA。`warm_up()` 在接流量前完成全部握手，`health()` 返回 ready/active/rendered/failure/restart/timeout 指标，`shutdown()` 停止接单并回收子进程。

`Page::respond_streaming_with_renderer` 使用 React `renderToPipeableStream`，把 HTML chunk 通过 `ResponseBody::Stream` 直接交给 Hyper；真实 TCP 测试固定了 chunked response。renderer 完成帧携带 island 描述，Phoenix 随后写入上下文安全的 hydration 信封和版本化浏览器入口。

SSR 必须定义：

- `Head`、HTTP 状态码、重定向和错误页的合并规则。
- hydration 数据的安全编码与 CSP nonce。
- 浏览器与服务端输出不一致时的诊断信息。
- 无 Node 生产环境可以显式选择 SPA；SSR 部署不能继续承诺“只有一个 Rust 二进制”。

Renderer 接口保持可替换，后续可以评估嵌入式 JavaScript runtime 或远程 renderer，但 P1 不同时维护多个实现。

## 5. Islands

页面仍是普通 `.tsx`。普通组件只输出服务端 HTML；需要浏览器交互的组件加 `client:load`：

```tsx
import SearchBox from "../../islands/search-box";

export default function DocsPage({ article }: DocsPageProps) {
  return (
    <main>
      <article>{article.body}</article>
      <SearchBox client:load source="docs" />
    </main>
  );
}
```

`@phoenix/vite` 在编译期把指令转换为 island 边界，并按 `views/pages/`、`views/islands/` 生成虚拟注册表和入口。服务端 renderer 渲染页面时自动收集组件名、稳定 ID 与 props；Rust 将结果写入 `PageEnvelope.islands`。浏览器入口读取信封后，只动态 import 实际出现的 island。

应用不需要维护 `entry.tsx`、island 注册表或 `renderer.tsx`。Vite 配置只启用插件：

```ts
export default defineConfig({ plugins: [phoenix()] });
```

服务端构建配置使用 `phoenix({ renderer: true })`。SSR 模式下 `client:load` 组件和普通组件一样参与整页 `hydrateRoot`，不会产生嵌套 island root；Islands 模式才生成独立 root。

要求：

- island props 必须实现独立契约并安全序列化。
- `client:load` 必须标记 `views/islands/` 中的 React 组件，不能直接标记原生 DOM 元素。
- island 不能依赖未显式声明的父级 React Context；跨 island 状态通过 URL、服务端、事件或显式共享 store 处理。
- 相同 island 在一页出现多次时，每个实例有稳定且不冲突的 hydration key。
- island 不允许嵌套；需要共享状态的交互区域应作为一个边界。
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

1. 已完成：统一 `PageEnvelope`、三种渲染语义、`client:load`、Vite 自动发现与按需加载。
2. 已完成：版本化生产 manifest、多 worker renderer 池、contract/resource 握手、健康指标、优雅关闭和流式 SSR。
3. 下一步：Head、结构化流错误、CSP nonce 和 hydration 诊断。
4. 稳定前：独立 island 入口、bundle 预算、缓存和部署验证。
5. 1.0：三种模式的部署文档、性能基线、安全测试和同页面契约一致性测试。

## 10. 验收标准

- 同一控制器和 Props struct 可以切换 SPA/SSR，React 页面无需复制。
- SSR 首屏 HTML 包含业务内容，hydration 后不产生不匹配警告。
- Islands 页面只下载实际出现的交互岛代码，不下载完整页面应用包。
- renderer 不可用时按配置快速失败或显式降级，不悬挂 Hyper 请求。
- 三种模式返回相同状态码、验证错误、flash 和业务 props 语义。
