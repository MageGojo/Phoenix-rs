# React DX · 性能、局部更新与健壮性（P3 / P4）

## 非目标

- 本轮**不改** Rust 控制器强制只算部分 props（服务端 partial 求值后置）。
- 前端先定协议头与合并语义；服务端暂时可返回完整信封，客户端按 `only`/`except` 合并。

## P3

### 1. Link prefetch

```tsx
<Link href="/posts/1" prefetch="hover" />  // hover | mount | viewport
```

- 仅 **GET** 预拉页面信封；写入内存 cache（URL → envelope，TTL 默认 30s）。
- **不**把预拉信封的 `csrf_token` 写进当前页；真正 visit 时再用响应 CSRF。
- 加密响应可不缓存或按 decrypt 后缓存明文（实现选：成功解密后可缓存）。
- visit 命中未过期 cache 时可跳过网络（可选；首版：prefetch 只暖缓存，visit 仍请求，但 abort 同 URL 进行中的 prefetch）。
- `viewport`：IntersectionObserver；`mount`：effect 立即拉；`hover`：pointerenter/focus。

### 2. Partial reload API

```ts
navigator.reload({ only: ["comments"] });
navigator.reload({ except: ["heavyChart"] });
navigate(url, { only: ["comments"], preserveScroll: true });
```

请求头（约定）：

- `X-Phoenix-Page: 1`
- `X-Phoenix-Only: comments,sidebar`（逗号分隔）
- `X-Phoenix-Except: heavyChart`

客户端合并（props 为 object 时）：

- `only`：保留当前 props，用响应 props 覆盖 listed keys；其它顶层字段（shared/flash/errors/head/csrf/routes…）仍用响应整份替换，除非 options 指定 `partialShared`（首版：**shared/flash/errors/head 整份替换**，仅 **props** 做 only/except）。
- `except`：用响应 props 覆盖除 listed 外的 keys。

`WhenVisible`：

```tsx
<WhenVisible data="comments" fallback={null}>
  {(comments) => <CommentList comments={comments} />}
</WhenVisible>
```

进入视口后 `reload({ only: [data] })`，从 `usePage().props[data]` 取值。

### 3. Lazy 失败 UI

`requiredComponent` / 页面加载包装：

```tsx
<PageLoadError error={error} onRetry={retry} />  // 默认可替换
```

`PhoenixOptions`：`pageLoadFallback?: ComponentType<{ error; retry }>`；超时可选 `pageLoadTimeoutMs`（默认 15s）。

## P4

### 4. ErrorBoundary

- `PhoenixErrorBoundary` 包住页面树（pageElement 内）。
- `PhoenixOptions.errorFallback` 可替换。
- 导航错误：除 `onNavigationError` 外，设置 `navigationError` 状态供 `OfflineBanner` / 默认条显示。

### 5. Offline / 网络提示

```tsx
<NavigationStatusBanner />  // 订阅 phoenix:navigation-error + online/offline
```

### 6. Remember 表单草稿

```ts
import {
  clearRemembered,
  rememberKey,
  useRemember,
  Form,
} from "@apizero/react";

useRemember(rememberKey("posts.create"), data, setData);
// 或
<Form remember="posts.create" action={...} initialValues={...}>...</Form>
```

- `rememberKey(name)` → `phoenix:remember:<name>`；`readRemembered` / `writeRemembered` / `clearRemembered` 直接读写 `sessionStorage`。
- `useRemember`：mount 时若有草稿则 `setData`；`data` 变化 debounce 300ms 写回；卸载时 flush。
- `<Form remember="...">` 内部接 `useRemember`；**成功 submit 后** `clearRemembered`（可在自定义 `onSuccess` 前清除）。

### 7. 滚动区域

- `[data-phoenix-scroll-region="main"]` 元素的 scrollTop/Left 写入 HistorySnapshot.regions。
- capture/restore 与 window scroll 并行；旧 snapshot 无 regions 仍兼容。

### 8. Dev overlay

```tsx
import { PhoenixDevOverlay } from "@apizero/react";

<PhoenixDevOverlay />                 // 默认 DEV 环境显示
<PhoenixDevOverlay enabled />         // 强制开
<PhoenixDevOverlay enabled={false} /> // 强制关
```

- 启用探测：`import.meta.env.DEV` → 否则 `process.env.NODE_ENV !== "production"` → 再否则仅 `enabled === true`。
- 角标（`data-phoenix-dev-overlay`，fixed 右下角低存在感 monospace）：page、contract_hash、asset_version、`location.href`、`lastVisitUrl`（`phoenix:navigation-start/success` 的 `detail.url`）、当前 pathname 在 `routes` 表中的**完全匹配** name（首版不做参数路由反查）。
- 页面数据：优先 `getPhoenixNavigator()?.page`，否则 `readPage(document)`；订阅导航事件刷新。

### 9. Island 内 Link

- Islands hydrate 时每个 island root 已包 `PhoenixPageProvider` + `navigationContext.Provider`（注入同一 `navigator`），岛内 `<Link>` 与整页 SPA 一样拦截同源点击。
- **无 Provider / 无 navigator**（裸渲染 `<Link>`，例如故事书或单元测试）：`Link` **不** `preventDefault`，走浏览器原生跳转。
- 测试：`navigation.test.tsx` 覆盖「无 navigation context 时不调用 preventDefault」。

## 文件边界（agent）

| Agent | 文件 |
| --- | --- |
| A prefetch+scroll | `prefetch.ts`、`history.ts`、Link prefetch、相关测试 |
| B partial+visible | `VisitOptions` only/except、`when-visible.tsx`、page-client headers、合并逻辑、测试 |
| C errors+lazy | `error-boundary.tsx`、`navigation-status.tsx`、lazy 失败、接入 navigation |
| D remember+dev+docs | `remember.ts`、`dev-overlay.tsx`、RENDERING/本文件 Island 段、Island Link 测试 |

## 验收

- `packages/phoenix-react` 全绿
- 文档：`docs/REACT_DX_PERF.md`、更新 `docs/RENDERING.md` 相关小节
