# Phoenix React / `@phoenix/react` (agent cheat sheet)

Browser entry calls `startPhoenix({ pages, islands })` (Vite plugin generates registries).

## Imports

```tsx
import {
  Link, navigate, redirect,
  Form, PageForm, FieldError, useForm, usePageForm,
  usePage, useShared, useFlash, useCsrfToken, useNavigator, useNavigating,
  useOptimisticAction, useRemember,
  ProgressBar, NavigationStatusBanner, PhoenixDevOverlay,
  WhenVisible, PhoenixErrorBoundary,
} from "@phoenix/react";
import { posts } from "../generated/routes.js";
import type { PostsIndexProps, StorePostInput } from "../generated/contracts.js";
import { StorePostInputFields } from "../generated/contracts.js";
```

Do **not** edit `views/generated/*`.

## Navigation

```tsx
<Link href="/posts" match="prefix" activeClassName="is-active" prefetch="hover" confirm="离开？">
  Posts
</Link>

await navigate("/posts/1", { replace, preserveScroll, preserveFocus, only: ["comments"] });
await redirect("/posts"); // default replace: true
navigator.reload({ only: ["comments"], preserveScroll: true });
```

- Prefetch: `hover` | `mount` | `viewport` — GET only; does not overwrite current CSRF.
- Partial: headers `X-Phoenix-Only` / `X-Phoenix-Except`; client merges **props** keys.
- No navigator (bare Link): native full navigation (Islands hydrate **do** inject navigator).

Events: `phoenix:navigation-start|success|hard|error|finish`.

## Page hooks

```tsx
const { page, props, errors, routes } = usePage<PostsIndexProps>();
const shared = useShared<PhoenixSharedProps>();
const { flash, consume } = useFlash<{ notice?: string }>();
const csrf = useCsrfToken();
const { processing, progress } = useNavigating();
```

Must be under `PhoenixPageProvider` (installed by navigator for pages/islands).

## JSON action forms

```tsx
<Form
  action={posts.store}
  initialValues={{ title: "" }}
  fields={StorePostInputFields}
  confirm="提交？"
  remember="posts.create"
  redirectTo="/posts"
  onSuccess={() => {}}
>
  {(form) => (
    <>
      <input {...form.field("title")} />
      <FieldError errors={form.errors} name="title" />
      <button disabled={form.processing}>Save</button>
    </>
  )}
</Form>
```

- 422 → `form.errors`; `setField` clears that field’s errors.
- `remember` → sessionStorage draft; cleared on success.

## Page protocol forms (Inertia-like)

```tsx
<PageForm
  action="/posts"
  method="post"
  initialValues={{ title: "" }}
  replace
  preserveScroll
  confirm="发布？"
>
  {(form) => /* same field helpers */}
</PageForm>
```

Success → visit/render new envelope; 422 → stay + field errors.

## Optimistic updates

```tsx
const { data, run, pending, error } = useOptimisticAction(posts.store, {
  initialData: items,
  onMutate: (input, current) => [...current, optimistic(input)],
  onSuccess: (out, _in, current) => /* replace */,
  onError: () => {}, // default rollback snapshot
});
```

## Partial UI

```tsx
<WhenVisible data="comments" fallback={<Spinner />}>
  {(comments) => <List items={comments} />}
</WhenVisible>
```

## Resilience / DX chrome

```tsx
<ProgressBar />
<NavigationStatusBanner />
<PhoenixDevOverlay />           {/* dev only by default */}
<PhoenixErrorBoundary>...</PhoenixErrorBoundary>
```

`PhoenixOptions`: `errorFallback`, `pageLoadTimeoutMs`, `pageLoadFallback`, `onNavigationError`.

## Islands

```tsx
{/* views/pages/posts/show.tsx */}
<LikeButton client:load postId={id} />
```

Call Rust:

```tsx
await posts.store({ title }); // generated typed action
```

## Scroll regions

```html
<div data-phoenix-scroll-region="main">...</div>
```

Restored with history snapshot alongside window scroll.

## Protocol reminders

- Page fetches: `X-Phoenix-Page: 1`
- Actions: JSON + optional `X-CSRF-Token` from envelope
- Asset/contract mismatch → hard navigation
- Encrypted page payloads need `decrypt` in `startPhoenix`
