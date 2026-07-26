# React 前端指南

`@apizero/react` 的日常用法，一页讲完。原则：**大道至简**——下面这些 API 覆盖 90% 的业务代码；没提到的导出都属于底层或测试设施，业务里用不到。

心智模型一句话：**页面 = Rust 控制器给的 props，React 只管渲染与交互**。类型来自契约生成（`views/generated/`，禁止手改）；URL 与提交用生成的 `routes` / action，不硬编码。

## 0. 速查表

| 要做的事 | 用什么 |
| --- | --- |
| 渲染页面数据 | 组件参数 + `views/generated/contracts.ts` 类型 |
| 站内跳转 | `<Link href="...">` |
| 读当前页 / 共享数据 / 闪存 | `usePage()` / `useShared()` / `useFlash()` |
| 调 JSON action（结果留在当前页） | `useForm` / `<Form>` |
| 页面表单（成功后换页） | `<PageForm>` |
| 导航进度 / 状态 | `<ProgressBar />` / `useNavigating()` |
| 只刷新部分 props | `useNavigator().reload({ only: [...] })` |
| 进入视口再加载重数据 | `<WhenVisible data="...">` |
| 表单草稿防丢 | `<Form remember="...">` / `useRemember` |
| 危险操作确认 | `confirm="确定？"`（`Link` / `Form` / `PageForm` 都有） |
| 图形验证码 | `<CaptchaImage />` / `useCaptcha()`（配 [CAPTCHA.md](CAPTCHA.md)） |

## 1. 启动（脚手架已写好，通常不用动）

```tsx
import { startPhoenix } from "@apizero/react";

startPhoenix({ pages, islands });
```

之后同源链接自动变成局部导航（只换页面内容，不整页刷新）。三种渲染模式（SPA / Islands / SSR）共用这一套；**同模式页面之间是局部导航**。以下情况运行时会自动退回整页浏览器导航，你不需要处理：目标页渲染模式不同、服务端 `asset_version` / `contract_hash` 变化（例如刚发了新版）、页面协议版本不一致——都是为了避免旧 bundle / 错误模式渲染新页面。

## 2. 页面组件：props 即契约

```tsx
import type { MembersPageProps } from "../generated/contracts.js";

export default function MembersIndex({ members, total }: MembersPageProps) {
  return <h1>共 {total} 人</h1>;
}
```

改了 Rust Props 后类型报错？跑 `npm run types`（或等 Vite 刷新）。**不要**手改 `views/generated/`。

## 3. 导航与命名路由 URL

URL 不硬编码——生成的 `routes` 里每个非 action 路由都是可调用的 URL 构造器，动态参数带类型：

```tsx
import { Link, redirect, urlFor } from "@apizero/react";
import { members, users } from "../generated/routes.js";

<Link href={members.index()} match="prefix" activeClassName="is-active">成员</Link>
<Link href={users.show({ id: member.id })} prefetch="hover">{member.name}</Link>
<Link href={members.index({ query: { page: 2 } })} />   // → /members?page=2

redirect(users.show({ id: 9 }));                         // 代码里跳转
urlFor("users.show", { id: 9 });                         // 手动逃生口，语义同 Rust Router::url
```

参数会自动 percent-encode，缺参数直接抛错（与服务端 `Router::url` 同语义）；路径来自页面信封的路由表，SSR/Islands/SPA 下都可用。路径不是字符串字面量的路由（如 `.get(member_path, …)`）会生成宽松类型的构造器，参数照传即可。

`Link` 其它日常用法：

```tsx
<Link href="/danger" confirm="确定离开？" />
<Link href="/exports/big.csv" reloadDocument />  // 显式走整页加载
```

激活态默认加 `aria-current="page"`；`match` 默认 `"exact"`。**action 路由（POST）不要放路径参数**——按约定 id 走请求体，直接调用 `members.store({ … })`。

## 4. 页面数据 hooks

```tsx
import { usePage, useShared, useFlash, useNavigating } from "@apizero/react";

const { props, errors } = usePage<MembersPageProps>();
const shared = useShared<PhoenixSharedProps>();   // 布局级共享数据（当前用户等）
const { flash } = useFlash<{ notice?: string }>(); // 一次性提示，服务端下一跳自动清
const { processing } = useNavigating();            // 导航中？配合骨架屏/禁用按钮
```

页面组件、布局、岛屿组件里都能用（每个岛注入了同一个 navigator 与页面上下文）。

## 5. 表单：两条路，按去向选

| | `useForm` / `<Form>` | `<PageForm>` |
| --- | --- | --- |
| 提交到 | 生成的 typed action（JSON） | 页面 URL |
| 成功后 | 拿到 `Output`，留在当前页（可 `redirectTo`） | 服务端返回新页面，直接换页 |
| 422 | 自动回填 `form.errors` | 同左 |
| 适合 | 增删改、局部交互、破坏性操作 | 传统「提交完去列表页」的整页表单 |

经验法则：**结果留在当前页用 `Form`，提交完去别的页用 `PageForm`**。破坏性操作（删除等）一律走 typed action + `confirm`，不做假链接。

```tsx
import { Form, FieldError } from "@apizero/react";
import { members } from "../generated/routes.js";
import { StoreMemberInputFields } from "../generated/contracts.js";

<Form
  action={members.store}
  initialValues={{ name: "" }}
  fields={StoreMemberInputFields}
  remember="members.create"        // 草稿存 sessionStorage，成功后自动清
  onSuccess={(member) => console.log(member.id)}
>
  {(form) => (
    <>
      <input {...form.field("name")} />
      <FieldError errors={form.errors} name="name" />
      <button disabled={form.processing}>保存</button>
    </>
  )}
</Form>
```

`form.field("name")` 由契约生成的字段表驱动，自动带 `name` / `value` / `onChange` / `required` / `aria-invalid`。服务端永远是最终校验者（422 即正常流程，不是异常）。

**文件上传**：`data` 里出现 `File` / `FileList` 时，`Form`/`useForm` 自动改用 multipart 提交（对应服务端 `Multipart<T>` 提取器），并暴露 `form.progress`（0–1）：

```tsx
<Form action={coverAction} initialValues={{ cover: null as File | null, title: "" }}>
  {(form) => (
    <>
      <input type="file" accept="image/*"
        onChange={(e) => form.setField("cover", e.target.files?.[0] ?? null)} />
      {form.progress !== null && <progress value={form.progress} max={1} />}
      <button disabled={form.processing}>上传</button>
    </>
  )}
</Form>
```

线格式约定：标量 → 文本域、`File` → 文件域、文件数组 → 重复同名域、嵌套对象 → JSON 字符串。手动调用用 `uploadRust("posts.cover", { cover }, { onUploadProgress })`。教程见 `docs/tutorial/intermediate/番外-文件上传.md`。

```tsx
import { PageForm } from "@apizero/react";

<PageForm action="/posts" method="post" initialValues={{ title: "" }} confirm="确定发布？">
  {(form) => (
    <>
      <input {...form.field("title")} />
      <button disabled={form.processing}>发布</button>
    </>
  )}
</PageForm>
```

## 5.5 分页

服务端用 `page_paginate` 返回 `Paginated<T>`（见 [DATABASE.md](DATABASE.md) 分页节），React 端拿 `meta` 直接渲染：

```tsx
import { Pagination, type PaginatedData } from "@apizero/react";
import { members } from "../generated/routes.js";

function MembersPage({ page }: { page: PaginatedData<Member> }) {
  return (
    <>
      {page.data.map((member) => <Row key={member.id} member={member} />)}
      <Pagination
        meta={page.meta}
        href={(n) => members.index({ query: { page: n } })}
      />
    </>
  );
}
```

`Pagination` 默认窗口式页码（1 … 4 5 **6** 7 8 … 20）、`preserveScroll`、当前页 `aria-current`；单页时不渲染。游标分页（`nextCursor`）配 `<WhenVisible>` 做无限滚动：进视口后带 cursor 重新拉取并在应用层累加列表。

## 6. 局部更新与懒加载

```tsx
const navigator = useNavigator();
navigator.reload({ only: ["comments"] });   // 只刷新 props.comments
navigator.reload({ except: ["heavyChart"] });

<WhenVisible data="comments" fallback={<p>加载中…</p>}>
  {(comments) => <CommentList comments={comments} />}
</WhenVisible>
```

`WhenVisible` 进入视口后自动 `reload({ only: [data] })`——首屏别算重数据，交给它。

## 6.4 视图 i18n

服务端按 `Accept-Language` 协商 locale 并把该 locale 的翻译目录注入页面信封（见 [I18N.md](I18N.md)），前端用 `useTranslations` 取:

```tsx
import { useTranslations } from "@apizero/react";

function Hello({ name }: { name: string }) {
  const { t, locale } = useTranslations();
  return <p lang={locale}>{t("greeting", { name })}</p>;  // zh-CN → 你好，小明！
}
```

`t(key, params)` 插值 `{name}` 占位符,缺 key 回退 key 原文——与服务端 `translate` 逐字一致。locale 缺省 `"en"`,`<html lang>` 也由协商结果驱动。

## 6.5 加密传输（一键）

```tsx
startPhoenix({ pages, islands, secure: true });
```

开启后：启动时先与服务端做一次 ECDH 握手协商**每会话密钥**，之后页面导航的**响应体**与提交的**请求体**都走 **AES-256-GCM 二进制帧**（`application/vnd.phoenix.secure`），两侧自动识别并解密——业务组件零改动，`Json<T>` / `Validated<T>` 等提取器也无感。密钥经 Web Crypto 派生为**不可导出密钥**，不进 bundle。握手失败默认回退明文；要强制加密用 `secure: true, secureRequired: true`。服务端需同步开启（`secure_transport(...)`，见 [SECURE_TRANSPORT.md](SECURE_TRANSPORT.md)）。

**诚实边界**：浏览器为渲染必须能解密，所以这不是端到端加密、无法对终端用户隐藏内容——它是 TLS 之上的纵深防御（防被动抓包/日志/爬虫/bundle 捞 key），不替代 HTTPS。带 `File` 的表单走 multipart + XHR（为了进度条），**不加密**，仍只由 TLS 保护。

## 7. 全局点缀（放布局里，一次搞定）

```tsx
<ProgressBar />              // 导航进度条
<NavigationStatusBanner />   // 断网 / 导航失败提示
<PhoenixDevOverlay />        // 开发角标：页面名、契约 hash、路由名（生产自动隐藏）
```

页面树默认已被 `PhoenixErrorBoundary` 包住；要换兜底 UI，传 `startPhoenix({ errorFallback })`。

## 8. 乐观更新（确有需要再用）

```ts
const { run, pending } = useOptimisticAction(members.store, {
  onMutate: (input, current) => [...current, optimisticItem(input)],
  onError: (error, rollback) => rollback(),
});
```

它不是状态库，状态只活在 hook 里。大多数场景 `form.processing` + 服务端返回值就够了。

## 深挖去哪

| 主题 | 文档 |
| --- | --- |
| 渲染模式 / 页面协议 / 硬导航规则 | [RENDERING.md](RENDERING.md) |
| 契约与生成物 | [CONTRACTS.md](CONTRACTS.md) |
| 表单/hooks/性能的设计取舍（内部设计文档） | [REACT_DX_FORMS.md](REACT_DX_FORMS.md) · [REACT_DX_HOOKS.md](REACT_DX_HOOKS.md) · [REACT_DX_PERF.md](REACT_DX_PERF.md) |
| 按序上手 | [tutorial/README.md](tutorial/README.md) |
