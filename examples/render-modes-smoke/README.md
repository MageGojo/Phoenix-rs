# render-modes-smoke

同一个 Phoenix-rs Rust + React 应用同时演示三种页面渲染模式，并启用 SQLite
做开发 / 发布产物的数据库读写烟测。

| 路由 | 模式 / 能力 | 可见验收点 |
| --- | --- | --- |
| `/` | Islands 入口 | 可跳转至三种模式与 Notes |
| `/spa` | SPA（也常被称为 CPA） | `#phoenix-root` 空壳 |
| `/islands` | Islands | `Count: 7` 可点击递增 |
| `/ssr` | SSR | 查看源代码已有业务标题 |
| `/notes` | SQLite 列表页 | 读 `storage/app.sqlite` |
| `POST /notes` | JSON create | 写入 notes 表并返回 `201` |

## 开发

```bash
cp .env.example .env
px migrate
px status
px dev
```

打开 <http://127.0.0.1:3000/>。`px migrate` 会创建 `storage/app.sqlite`
并应用 `notes` 表迁移。

### 数据库验收（开发态）

```bash
# 取 CSRF + session
curl -sS -c /tmp/smoke-cookies -D /tmp/smoke-headers -o /dev/null http://127.0.0.1:3000/notes
CSRF=$(rg -i '^x-csrf-token:' /tmp/smoke-headers | awk '{print $2}' | tr -d '\r')

# 写入
curl -sS -b /tmp/smoke-cookies -H "Content-Type: application/json" \
  -H "X-CSRF-Token: $CSRF" \
  -d '{"name":"dev-note-1"}' \
  http://127.0.0.1:3000/notes

# 读回
curl -sS http://127.0.0.1:3000/notes | rg 'dev-note-1'
```

## 生产构建与本地启动

```bash
npm run build
cargo run --bin phoenix-manage -- migrate
cargo run -- serve
```

## Release 产物验收

```bash
px release --version 0.1.0 --tarball
STAGING=dist/releases/0.1.0/staging
mkdir -p "$STAGING/storage"
# 在制品根目录执行 migrate（cwd 必须是 staging，不是 bin/）
(cd "$STAGING" && ./bin/phoenix-manage migrate && ./bin/phoenix-manage status)
# 也可从 bin/ 启动应用：console 会 chdir 到 release 根
(cd "$STAGING/bin" && ./render-modes-smoke serve)
```

然后对 `http://127.0.0.1:3000/notes` 重复上面的 CSRF + POST + GET 验收。

## 渲染模式验收

```bash
for path in spa islands ssr; do
  curl -sS -D - -o /dev/null "http://127.0.0.1:3000/$path" \
    | rg -i '^x-phoenix-render-mode:'
done
```
