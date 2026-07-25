# render-modes-smoke

同一个 Phoenix-rs Rust + React 应用同时演示三种页面渲染模式。四条路由共用
Rust 契约、Vite 构建产物和 React 页面协议，便于在开发与发布前快速自测。

| 路由 | 模式 | 首屏行为 | 可见验收点 |
| --- | --- | --- | --- |
| `/` | Islands | 演示入口 | 可跳转至三种模式 |
| `/spa` | SPA（也常被称为 CPA） | 浏览器完成首次渲染 | 源码中的 `#phoenix-root` 为空，仅含 hydration 数据 |
| `/islands` | Islands | 服务端生成页面，只激活计数器 | `Count: 7` 按钮可点击递增 |
| `/ssr` | SSR | 服务端生成完整页面，再整页 hydration | 查看源代码已有页面业务标题 |

## 开发

```bash
cp .env.example .env
px dev
```

打开 <http://127.0.0.1:3000/>。`px dev` 先构建 client 与 renderer bundle，
再启动应用；Rust、React、Vite 配置和依赖文件变更时会自动重建。

## 生产构建与本地启动

```bash
npm run build
cargo run -- serve
```

`npm run build` 会依次执行 `build:client` 和 `build:ssr`；`npm run types`
从 Rust 的 `#[phoenix::contract]` 生成 TypeScript 类型，`npm run typecheck`
会先生成类型再执行 TypeScript 检查。不要手改 `views/generated/`。

## 命令行验收

服务启动后，分别检查三种页面的响应头和源码：

```bash
for path in spa islands ssr; do
  curl -sS -D - -o /dev/null "http://127.0.0.1:3000/$path" \
    | rg -i '^x-phoenix-render-mode:'
done

curl -sS http://127.0.0.1:3000/spa | rg '<div id="phoenix-root" data-render-mode="spa"></div>'
curl -sS http://127.0.0.1:3000/ssr | rg 'SSR page is ready'
curl -sS http://127.0.0.1:3000/islands | rg 'Islands page is ready'
```

预期 `X-Phoenix-Render-Mode` 依次为 `spa`、`islands`、`ssr`。在浏览器中使用
“查看网页源代码”：SPA 的 `#phoenix-root` 为空（页面 props 会安全序列化在
hydration 数据中），SSR 与 Islands 含对应标题；访问
`/islands` 后点击 `Count: 7`，计数应递增，证明仅该 island 在浏览器端激活。

## Release

```bash
px release --version 0.1.0 --tarball
```

发布包应由 `px release:install` 安装到目标环境；不要直接覆盖服务目录。发布前先运行
`npm run build`，再按上面的 curl 与浏览器检查确认三种模式均可用。
