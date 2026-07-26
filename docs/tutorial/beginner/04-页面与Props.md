# 04 · 页面与 Props

← [03 项目地图](./03-项目地图.md) · 下一章 → [05 路由与控制器](./05-路由与控制器.md)

## 目标

走通「Rust Props → 生成 TS 类型 → React 页面」最小闭环。

## 必做

保持 `px dev`（或改完后重启）。

### 1. 改 Rust Props

打开 `app/props/home_props.rs`（名称以你项目为准），修改 `title` / `description` 的**默认值并不在这里**——默认值在控制器里构造。  
Props 结构体字段决定契约；先确认字段存在，例如：

```rust
pub struct HomeProps {
    pub title: String,
    pub description: String,
}
```

### 2. 改控制器传入的数据

打开 `app/controllers/home_controller.rs`，把 `HomeProps { title, description }` 改成你的文案，例如：

- title: `学习 Phoenix-rs`
- description: `初级第四章：页面与 Props`

保存后等 Rust 重启。

### 3. 改 React 页面

打开 `views/pages/home.tsx`，确保渲染 `title` / `description`（脚手架已有）。可加一行静态说明，例如「这是我的练习应用」。

### 4. 刷新浏览器

首页应显示新文案。若类型报错：在应用根执行 `npm run types`（或依赖 Vite 刷新），**不要**手改 `views/generated/contracts.ts`。

## 讲解

```text
控制器构造 Page("home", HomeProps { … })
        ↓
Rust #[phoenix::contract(page, …)]
        ↓
views/generated/contracts.ts 中的 HomeProps
        ↓
views/pages/home.tsx 使用同名字段
```

渲染模式（`.spa()` / `.islands()` / `.ssr()`）在控制器 Page 链上选择；初级保持脚手架默认即可。

## 验收

- [ ] 浏览器可见你改过的 title/description
- [ ] 未手改 `views/generated/`

## 延伸阅读

- `docs/REACT.md`（React 日常 API 一页速查）
- `docs/RENDERING.md`（先别深挖三种模式）
- `docs/CONTRACTS.md` 入门段

## 下一章预告

新增一条命名路由与控制器方法。
