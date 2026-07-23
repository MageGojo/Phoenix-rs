# React DX Hooks（页面状态 / 导航 / Active Link / 进度条）

本轮交付：让业务组件不必手读 `#phoenix-page` 信封，并补齐日常导航 UX。

## 目标 API

```tsx
import {
  Link,
  Form,
  ProgressBar,
  redirect,
  usePage,
  useShared,
  useFlash,
  useCsrfToken,
  useNavigator,
  useNavigating,
} from "@phoenix/react";
import type { MembersPageProps } from "../generated/contracts.js";
import type { PhoenixSharedProps } from "../generated/contracts.js";

function Header() {
  const { page, props, errors, routes } = usePage<MembersPageProps>();
  const shared = useShared<PhoenixSharedProps>();
  const { flash, consume } = useFlash<{ notice?: string }>();
  const csrf = useCsrfToken();
  const navigator = useNavigator();
  const { processing, progress } = useNavigating();

  return (
    <>
      <ProgressBar />
      <Link href="/members" match="prefix" activeClassName="is-active">
        Members
      </Link>
      <Form
        action={members.store}
        initialValues={{ name: "" }}
        redirectTo="/members"
        onSuccess={() => consume("notice")}
      />
    </>
  );
}
```

## 行为约定

### 页面上下文

- `BrowserNavigator` 在 SPA/SSR 整页 root 与 Islands hydrate 时注入同一套 Provider。
- 每次成功 `renderPage` 用新的 `PageEnvelope` 作为 Provider value，驱动 hook 重渲染。
- Hook 在 Provider 外调用时抛明确错误。

### flash

- 服务端负责「下一跳清空」；前端默认只读信封里的 `flash`。
- `useFlash().consume(...keys)` 仅在**当前页本地**遮蔽已读 key，导航后以新信封为准重置。

### 导航状态

- `useNavigating()` 订阅 `phoenix:navigation-start|success|hard|error|finish`。
- `processing`：start 后为 true，finish 后为 false（含 error/hard）。
- `progress`：0–1 的近似进度；无真实字节流时用定时器缓升，finish 时到 1 再复位。

### Active Link

- `match?: "exact" | "prefix"`，默认 `"exact"`。
- 比较当前 `location.pathname`（去尾 `/`，根路径 `/` 特殊处理）与 `href` 的 pathname。
- 激活时合并 `activeClassName`，默认设置 `aria-current="page"`（可用 `aria-current={false}` 关闭）。

### Redirect

- `redirect(url, options?)` → `navigate(url, options)` 的薄封装。
- `Form` / 可选 helper 支持 `redirectTo?: string`；成功后在 `onSuccess` 之前或之后导航（约定：**先 onSuccess，再 redirect**，便于 toast；若 `onSuccess` 抛错则不 redirect）。

## 文件边界

| 文件 | 职责 |
| --- | --- |
| `packages/phoenix-react/src/page-state.tsx` | Page/Flash/Navigating context 与 hooks |
| `packages/phoenix-react/src/progress.tsx` | `ProgressBar` |
| `packages/phoenix-react/src/redirect.ts` | `redirect` |
| `packages/phoenix-react/src/navigation.tsx` | Provider 接入、Active `Link`、`useNavigator` |
| `packages/phoenix-react/src/forms.tsx` | `redirectTo` |
| `packages/phoenix-react/src/index.ts` | 导出 |

## 验收

- 单元测试覆盖：Provider 内外、flash consume、navigating 事件、Active Link 匹配、Form redirectTo、CSRF hook。
- `npm run test --workspace=@phoenix/react`（或包内 `npm test`）通过。
- 更新 `docs/RENDERING.md` 客户端导航小节。
