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

### 客户端导航

Vite 生成的浏览器入口会调用 `startPhoenix({ pages, islands })`。启动器在 SPA/SSR 首屏挂载整页 React root，在 Islands 首屏只 hydrate 实际 island；第一次局部导航后，三种模式都由同一个客户端 root 替换页面组件和 props：

```tsx
import { Link, navigate } from "@apizero/react";

export function ProjectLinks() {
  return (
    <nav>
      <Link href="/projects">项目</Link>
      <Link href="/projects?view=archived" replace preserveScroll>
        已归档
      </Link>
    </nav>
  );
}

await navigate("/projects/new", { preserveFocus: true });
```

`Link` 和文档级委托只接管同源、无修饰键的主键点击。外链、`download`、非 `_self` target、`rel="external"` 和浏览器打开新标签操作保持原生行为；同源链接需要显式完整加载时使用 `<Link href="..." reloadDocument>`。Active 状态可用 `match="exact" | "prefix"` 与 `activeClassName`（详见 [REACT_DX_HOOKS.md](REACT_DX_HOOKS.md)）。**Islands hydrate** 时每个岛已注入 `PhoenixPageProvider` + `navigationContext`（与整页同一 navigator），岛内 `Link` 正常拦截；**若无 Provider**（裸渲染 `Link`），不 `preventDefault`，浏览器原生跳转（详见 [REACT_DX_PERF.md](REACT_DX_PERF.md) §9）。每次局部访问发送 `X-Phoenix-Page: 1`；新访问会取消旧请求，并用请求序号阻止不遵守 `AbortSignal` 的旧响应覆盖新页面。响应协议版本不一致，或当前页已知的 `asset_version` / `contract_hash` 在响应中缺失或发生变化，会在加载新组件前执行完整浏览器导航，避免旧 bundle 渲染新契约；当前页尚无 identity 时可以采用响应首次给出的值。

成功导航会同步更新 `#phoenix-page`、受控 `PageHead`、React 页面、History 和 URL。普通访问默认滚动到顶部并把焦点移到 `autofocus`、`main` 或主标题；hash、`preserveScroll` 和 `preserveFocus` 可以覆盖该行为。History 条目保存滚动坐标与带 `id`/`data-phoenix-focus-key` 的焦点位置，`popstate` 重取页面后恢复它们。`replace` 使用 `replaceState`，其余访问使用 `pushState`。

运行时在 `document` 上发送 `phoenix:navigation-start`、`phoenix:navigation-success`、`phoenix:navigation-hard`、`phoenix:navigation-error` 和 `phoenix:navigation-finish` 事件。`<ProgressBar />` 与 `useNavigating()` 订阅同一组事件。手动启动时也可以传入 `fetcher`、`decrypt`、`onNavigationError` 和测试用 `hardNavigate`；测试或卸载时使用 `stopPhoenix()` 清理监听器与 React roots。

### 页面 hooks 与 Redirect

业务组件通过 hooks 读当前信封，不必手解析 `#phoenix-page`：

```tsx
import {
  Form,
  Link,
  ProgressBar,
  redirect,
  useCsrfToken,
  useFlash,
  useNavigating,
  useNavigator,
  usePage,
  useShared,
} from "@apizero/react";
import type { MembersPageProps, PhoenixSharedProps } from "../generated/contracts.js";

function Header() {
  const { page, props, errors } = usePage<MembersPageProps>();
  const shared = useShared<PhoenixSharedProps>();
  const { flash, consume } = useFlash<{ notice?: string }>();
  const csrf = useCsrfToken();
  const navigator = useNavigator();
  const { processing } = useNavigating();
  return (
    <>
      <ProgressBar />
      <Link href="/members" match="prefix" activeClassName="is-active">
        Members ({page})
      </Link>
      <button disabled={processing} onClick={() => void navigator.reload()}>
        刷新
      </button>
      {flash.notice ? <p onClick={() => consume("notice")}>{flash.notice}</p> : null}
      <span hidden>{csrf}</span>
      <pre>{JSON.stringify({ props, shared, errors })}</pre>
    </>
  );
}
```

`flash` 由服务端在下一跳清空；`consume` 只在当前页本地遮蔽已读 key。`redirect(url)` 默认 `replace: true`。`Form` 可设 `redirectTo`：先 `onSuccess`，未抛错再 `redirect`。完整约定见 [REACT_DX_HOOKS.md](REACT_DX_HOOKS.md)。

### 强类型表单与字段错误

生成的 Rust action 可以直接作为 `Form` 的 action。输入值、`setField` 字段和值类型以及成功结果都从 Rust 契约保持类型约束：

```tsx
import { FieldError, Form } from "@apizero/react";
import { members } from "../generated/routes.js";
import type { Member, StoreMemberInput } from "../generated/contracts.js";

export function MemberForm() {
  return (
    <Form<StoreMemberInput, Member>
      action={members.store}
      initialValues={{ name: "" }}
      redirectTo="/members"
      onSuccess={(member) => console.info(member.id)}
    >
      {(form) => (
        <>
          <input
            name="name"
            value={form.data.name}
            onChange={(event) => form.setField("name", event.currentTarget.value)}
            aria-invalid={Boolean(form.errors.name)}
          />
          <FieldError errors={form.errors} name="name" />
          <button type="submit" disabled={form.processing}>保存</button>
        </>
      )}
    </Form>
  );
}
```

`Form` 自动把新的提交连接到 `AbortSignal`，重复提交会取消前一请求。422 响应中的 `{ errors: { field: [{ rule, message }] } }` 会映射为 `form.errors`；`FieldError` 输出该字段第一条消息，`setField` 会清除正在编辑字段的旧错误。`useForm` 提供同一状态机的 hook 形式，并暴露 `submit`、`cancel`、`reset`、`clearErrors`、`failure` 和 `wasSuccessful`。底层 `RustCallError` 继续保留 `status`、原始 `details` 和规范化后的 `fieldErrors`。

## 4. SSR

流程：

```text
Hyper 请求 -> 控制器 -> PageEnvelope -> renderer 池
                                      -> React HTML + head + hydration data
                                      -> Hyper 流式/完整响应
浏览器 -> hydrateRoot -> 后续 SPA 导航
```

当前实现使用可配置数量的长期 Node.js renderer worker，而不是每个请求启动 Node 进程。Renderer protocol v2 使用版本化按行 JSON 与流式分帧协议传入完整 `PageEnvelope`、URL、locale 和顶层 `csp_nonce`；`asset_version` 与 `contract_hash` 随信封传递。Nonce 不进入 `PageEnvelope`。每个子进程启动时校验协议、浏览器资源版本与契约 hash，旧 v1 worker 会失败关闭；完整渲染发生 I/O 故障时会替换 worker 并重试一次。

`RendererConfig::with_workers` 设置池容量；等待 worker 和 Node 响应共用 deadline，超过后淘汰协议状态不确定的 worker并快速失败，不会静默切换为 SPA。`warm_up()` 在接流量前完成全部握手，`health()` 返回 ready/active/rendered/failure/restart/timeout 指标，`shutdown()` 停止接单并回收子进程。

`Page::respond_streaming_with_renderer` 使用 React `renderToPipeableStream`，把同一请求 nonce 交给 React 后将 HTML chunk 通过 `ResponseBody::Stream` 直接交给 Hyper；真实 TCP 测试固定了 chunked response，跨 Rust/官方 Node renderer 的 E2E 固定了 React Suspense 恢复脚本的 nonce。只有 renderer `Complete` 帧携带 island 描述后，Phoenix 才写入带同一 nonce、且上下文安全的 hydration 信封和版本化浏览器入口；启动、协议或中途渲染失败只终止文档，不启动客户端。

SSR 必须定义：

- `Head`、HTTP 状态码、重定向和错误页的合并规则。
- hydration 数据使用脚本上下文安全编码，并与 CSP Header 使用同一请求级 nonce。
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

`@apizero/vite` 在编译期把指令转换为 island 边界，并按 `views/pages/`、`views/islands/` 生成虚拟注册表和入口。服务端 renderer 渲染页面时自动收集组件名、稳定 ID 与 props；Rust 将结果写入 `PageEnvelope.islands`。浏览器入口读取信封后，只动态 import 实际出现的 island。

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
- Islands 首屏 hydrate 为每个岛包一层 `PhoenixPageProvider` + `navigationContext`，因此岛内可直接用 `usePage` / `useNavigator` / `<Link>`；文档级 click 委托仍覆盖非 React 锚点。

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

`islands` 仅在 Islands renderer 输出时存在实际内容。页面业务 props 的 JSON 语义不因渲染模式变化。CSP nonce 是文档执行上下文，不属于业务协议，因此不会出现在该 JSON、共享 props 或 contract hash 中。

## 7. 构建产物

### 配置 client 与 renderer 构建

浏览器构建使用默认插件：

```ts
// vite.config.ts
import { defineConfig } from "vite";
import { phoenix } from "@apizero/vite";

export default defineConfig({
  plugins: [phoenix()],
});
```

renderer 使用独立配置：

```ts
// vite.ssr.config.ts
import { defineConfig } from "vite";
import { phoenix } from "@apizero/vite";

export default defineConfig({
  plugins: [phoenix({ renderer: true })],
});
```

必须先构建 client，再构建 renderer：

```bash
npm run build:client
npm run build:ssr
```

renderer 构建会读取 client manifest；manifest 缺失、schema 非法或 contract hash 不一致时立即失败。不要并行运行这两个构建，也不要混用不同提交生成的产物。

默认产物：

```text
public/assets/
  phoenix-[hash].js
  chunks/*-[hash].js
  client.css
  phoenix-manifest.json
public/ssr/
  renderer.js
  phoenix-renderer.json
```

`phoenix-manifest.json` 记录 schema、构建版本、contract hash、public path、client 入口、CSS 和 imports；renderer manifest 记录 renderer 版本、相同 contract hash 和入口文件。浏览器入口使用内容 hash，Rust 不应硬编码其文件名。

### 启动生产 renderer

应用启动时先加载两个 manifest，并预热所有 worker：

```rust
use std::time::Duration;

use phoenix::prelude::{
    AssetManifest, NodeRenderer, RendererConfig, RendererManifest,
};

let assets = AssetManifest::load("public/assets/phoenix-manifest.json")?;
let renderer_manifest = RendererManifest::load(
    "public/ssr/phoenix-renderer.json",
)?;

let renderer = NodeRenderer::new(
    RendererConfig::production(
        &assets,
        &renderer_manifest,
        "public/ssr",
    )?
    .with_workers(4)
    .with_timeout(Duration::from_secs(2)),
);

renderer.warm_up().await?;
```

`RendererConfig::production` 同时固定 client asset version 与 contract hash。Rust 发出的 `PageEnvelope`、Node 启动握手和每次渲染都必须一致；漂移时失败关闭，不允许使用旧 renderer 渲染新页面数据。

worker 数量应按 CPU、页面耗时和内存基线压测后设置。deadline 包含等待 worker 容量和 Node 渲染时间，因此不要把它理解为单纯的脚本执行超时。

### 在页面响应中使用生产资源

```rust
use phoenix::prelude::{AssetManifest, NodeRenderer, Page, Request, Response};

pub fn show(
    request: &Request,
    assets: &AssetManifest,
    renderer: &NodeRenderer,
) -> Result<Response, phoenix::view::AssetManifestError> {
    let page = Page::new("articles/show", article_props())
        .ssr()
        .production_assets(assets, "client")?;

    Ok(page.respond_streaming_with_renderer(request, renderer))
}
```

`production_assets(..., "client")` 从 manifest 注入实际 JS/CSS URL，并把 asset version 与 contract hash 写进页面信封。SSR/Islands 完整页面可以使用 `respond_streaming_with_renderer`；带 `X-Phoenix-Page: 1` 的局部导航仍返回原子的页面 JSON。

需要完整缓冲响应时使用 `respond_with_renderer(...).await`。renderer 失败会返回 503，不会静默改成 SPA。流式响应的 Header 一旦发送便不能改写状态，因此生产启动必须先调用 `warm_up()`；流中失败会关闭未完成文档且不会追加 hydration/module script。客户端断开、协议错误和 deadline 会在释放 worker 锁前原子作废对应进程，排队请求只会启动干净 worker，不会读取前一请求的残留帧。

### CSP nonce 与页面缓存

在路由外层安装 `NonceSecurityPolicy` 后，`Page::respond_to`、直接返回 `Page`、完整 renderer 和流式 renderer 都会读取同一个 Request nonce。框架输出以下标记：

```html
<meta property="csp-nonce" nonce="REQUEST_NONCE">
<link rel="stylesheet" href="/assets/app.css" nonce="REQUEST_NONCE">
<script id="phoenix-page" type="application/json" nonce="REQUEST_NONCE">...</script>
<script type="module" src="/assets/app.js" nonce="REQUEST_NONCE"></script>
```

Vite 7 从该 meta 继承 HMR 与动态 style/link nonce；Vite 配置不得写死 `html.cspNonce`。带 nonce 的文档固定为 `private, no-store` 并删除验证器，防止共享缓存重放另一个请求的 nonce。页面协议 JSON 不携带 nonce，也不会被强制改写应用已有缓存策略。

官方 Rust→Node→React Suspense→HTML nonce 链路使用 `npm run test:e2e:ssr-csp` 验证；该命令先构建 workspace 内的官方 React 包，再运行显式集成测试，不读取被忽略的示例构建产物。

`Page::respond(false, ...)` 和在路由外直接调用 `Page::into_response()` 没有 Request 上下文，因此不会凭空创建 nonce。启用 nonce policy 的应用应从 Handler 直接返回 `Page`，或使用 `respond_to`/renderer API；CLI 生成代码遵守这一约定。

SPA/SSR 的局部页面协议请求不需要 Node renderer，会直接返回原子 JSON；Islands 局部导航仍调用 renderer 收集实际 island 描述。这个差异只影响 renderer 工作量，不改变 props 语义。

### 静态资源解析

静态请求只能解析 manifest 明确声明的文件：

```rust
let file = assets.resolve_static(
    "public/assets",
    "/assets/phoenix-a1b2c3.js",
)?;
```

不要把 URL 去掉前缀后直接拼到文件系统。`resolve_static` 会拒绝错误 public path、`..`、绝对路径、反斜杠和未在 manifest 中登记的文件。应用可以读取返回路径提供响应，也可以让反向代理/CDN 提供同一目录。

### 健康检查与优雅关闭

```rust
let health = renderer.health();

let ready = health.ready_workers == health.configured_workers
    && !health.shutting_down;

// 应用停止接收新请求并等待现有 HTTP 请求后：
renderer.shutdown().await;
```

建议导出以下指标：`ready_workers`、`active_requests`、`rendered_requests`、`failures`、`restarts` 和 `timeouts`。ready worker 未达到配置数量时 readiness 应失败；单次失败增长可用于告警，但是否退出进程由部署策略决定。

`shutdown()` 会停止接收 renderer 工作并回收子进程。它应纳入 Rust 服务的 shutdown future，并在容器强制终止期限之前完成。

### 产物边界

Vite 生成：

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
3. 已完成：受控 PageHead 在 HTML 与局部页面协议间保持一致。
4. 已完成：请求级 CSP nonce、Vite meta、renderer v2、React 流式 nonce 与 HTML no-store 边界。
5. 下一步：结构化流错误、hydration 诊断和实时/流式请求协议。
5. 稳定前：独立 island 入口、bundle 预算、缓存和部署验证。
6. 1.0：三种模式的部署文档、性能基线、安全测试和同页面契约一致性测试。

## 10. 验收标准

- 同一控制器和 Props struct 可以切换 SPA/SSR，React 页面无需复制。
- SSR 首屏 HTML 包含业务内容，hydration 后不产生不匹配警告。
- Islands 页面只下载实际出现的交互岛代码，不下载完整页面应用包。
- renderer 不可用时按配置快速失败或显式降级，不悬挂 Hyper 请求。
- 三种模式返回相同状态码、验证错误、flash 和业务 props 语义。

## 11. 受控页面元数据

`Page::head(PageHead::new(...))` 把 title、description、canonical、robots 和 Open Graph 写入同一个页面信封。完整 HTML 由 Rust 使用上下文正确的 HTML 转义生成 `<head>`；局部导航读取相同结构，不接受业务注入任意标签、脚本或样式。`Page::csrf_token(...)` 是可选协议字段，供浏览器 action 传输使用，不应进入日志或页面业务 props。

## 12. 自定义 HTML 文档

应用可以定制 React root 外围的 HTML，而不接管 hydration 协议：

```rust
use phoenix::prelude::{
    DocumentSlots, DocumentTemplate, Page, TrustedHtml,
};

let document = DocumentTemplate::from_fn(|context| {
    DocumentSlots::new()
        .language("zh-CN")
        .body_attribute("class", "application-shell")
        .expect("static HTML attribute")
        .root_attribute("class", "application-root")
        .expect("static HTML attribute")
        .head(TrustedHtml::new(format!(
            "<script{} src=\"/theme.js\"></script>",
            context.nonce_attribute(),
        )))
        .before_root(TrustedHtml::new("<header>Application</header>"))
        .after_root(TrustedHtml::new("<footer>Company</footer>"))
});

let response = Page::new("dashboard/show", props)
    .document(document);
```

`DocumentTemplate::from_fn` 每个请求接收只读 `DocumentContext`，因此布局可以按页面名或渲染模式选择 chrome，并给自定义 script/style 使用当前请求的 CSP nonce。需要返回配置错误时使用 `DocumentTemplate::try_from_fn`；失败会映射为不泄露详情的 500。

框架始终拥有 `#phoenix-root`、`data-render-mode`、安全编码的 hydration JSON 和版本化 module 入口。模板只能增加经过转义的 element attributes，以及明确包装为 `TrustedHtml` 的 head/root 前后片段。`TrustedHtml` 会原样进入响应，只用于应用自有静态标记或已经按 HTML 上下文净化的内容；动态 title/meta 继续使用 `PageHead`。
