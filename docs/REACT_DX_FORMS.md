# React DX · 表单与提交（P2）

本轮补齐 Inertia 风格页面表单、契约字段绑定、confirm、轻量乐观更新。  
**不做**：Method spoofing Link（DELETE/PUT 继续走命名 action）；完整 Multipart 上传进度（后置）。

## 1. 页面型 Form（Inertia 风格）

现有 `Form` 只走 JSON `callRust` action。新增并行路径：

```tsx
import { PageForm, usePageForm } from "@phoenix/react";

<PageForm
  action="/posts"
  method="post"
  initialValues={{ title: "" }}
  replace
  preserveScroll
  confirm="确定发布？"
>
  {(form) => (/* ... */)}
</PageForm>
```

### 传输

- 请求头：`X-Phoenix-Page: 1`；有 CSRF 则带 `X-CSRF-Token`
- `method`: `get` | `post` | `put` | `patch` | `delete`（默认 `post`）
- body：非 GET 时默认 `application/json` 序列化 `data`；可选后续扩展 `as="form"` urlencoded
- 成功（2xx + page envelope）：经 `navigator` 渲染新页（等同 visit 成功路径），支持 `replace` / `preserveScroll` / `preserveFocus`
- **422**：不换页；把 `{ errors: { field: [{ rule, message }] } }` 或页面信封 `errors` 回填到 `form.errors`
- 其它 4xx/5xx：记入 `failure`，抛错或 `onError`

### VisitOptions 扩展

`navigate` / `PhoenixNavigator.visit` 增加可选：

```ts
method?: "get" | "post" | "put" | "patch" | "delete";
data?: Record<string, unknown>;
```

底层 `fetchPage` 支持带 method/body 的页面协议请求。

## 2. 契约驱动 `form.field`

`phoenix-vite` 对每个 **input** 契约额外生成运行时字段表（首版只含 name / ts 类型标签 / required）：

```ts
// views/generated/contracts.ts（生成）
export const StoreMemberInputFields = {
  name: { name: "name", type: "string", required: true },
} as const satisfies FormFieldMap;
```

React：

```tsx
const form = useForm(members.store, { name: "" }, { fields: StoreMemberInputFields });
<input {...form.field("name")} />
// => { name, value, required, onChange, "aria-invalid", "data-phoenix-field": "name" }
```

首版不做客户端验证规则同步（CONTRACTS §7 仍成立：服务端是最终验证者）。

## 3. confirm

- `<Link href="..." confirm="确定离开？" />`
- `<Form confirm="..." />` / `<PageForm confirm="..." />`
- 使用 `window.confirm`；取消则不导航/不提交
- 可注入 `confirmFn` 便于测试

## 4. 明确不做（本轮）

| 项 | 理由 |
| --- | --- |
| Method spoofing Link | 破坏性写操作继续走 typed action；避免假 `<a method=delete>` |
| `onUploadProgress` / Multipart | 后置；等 storage + 真实上传案例再做 |

## 5. `useOptimisticAction`

```ts
const { run, data, error, pending } = useOptimisticAction(members.store, {
  onMutate: (input, current) => [...current, optimisticItem(input)],
  onError: (error, rollback) => rollback(),
});
```

- 调用前应用 `onMutate` 得到乐观值
- 成功：用服务端返回值替换（或 `onSuccess`）
- 失败：调用 rollback（保存 mutate 前快照）
- 不是通用状态库；状态只在 hook 内

## 6. 文件边界

| 文件 | 职责 |
| --- | --- |
| `page-client.ts` | 页面协议 POST/PUT/… |
| `navigation.tsx` | VisitOptions method/data；Link confirm |
| `page-form.tsx` | `usePageForm` / `PageForm` |
| `forms.tsx` | Form confirm；`field()` 接入 |
| `fields.ts` | `form.field` 类型与 props 生成 |
| `optimistic.ts` | `useOptimisticAction` |
| `phoenix-vite/contracts.ts` | 生成 `*Fields` 常量 |
| `docs/REACT_DX_FORMS.md` | 本文件 |

## 7. 验收

- phoenix-react：PageForm 422 回填、成功换页、confirm 取消、optimistic rollback
- phoenix-vite：input 契约生成 `XxxFields`；测试断言
- Method spoofing / 上传进度不在测试范围
