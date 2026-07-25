# render-modes-smoke

同一个 Phoenix-rs 应用同时演示三种 React 渲染模式、SQLite Notes，以及本地零外部依赖的框架能力（WS / SSE / Metrics / JWT / Auth / Password / Storage / Queue / Mail / Plugin）。用作 **开发态（`px dev`）** 与 **编译产物（`px release` + `bin/` serve）** 的双路径验收靶场。

| 路由 / 命令 | 能力 | 验收点 |
| --- | --- | --- |
| `/` `/spa` `/islands` `/ssr` | Islands / SPA / SSR | `X-Phoenix-Render-Mode` |
| `/notes` + `POST /notes` | SQLite | 读写 `storage/app.sqlite` |
| `GET /hello` · `greet` | Plugin / FeatureSet | JSON `smoke-hello`；CLI 打印 |
| `GET /features/sse` | SSE | `text/event-stream` + `data: hello` |
| `GET /features/ws` | WebSocket | `ping` → `pong` |
| `GET /internal/metrics` | Metrics | Prometheus 含 `phoenix_http_requests_total` |
| `POST /features/password/hash\|verify` | Password | hash 可校验；错密失败 |
| `POST /features/jwt/token` · `GET /features/jwt/me` | JWT | Bearer 200；坏 token 401 |
| `GET /features/admin` | Auth / RBAC | admin 200 / guest 403 |
| `POST/GET /features/storage` | Storage | 落盘；`../` 拒绝 |
| `POST /features/queue/ping` | Queue | Memory worker ack |
| `POST/GET /features/mail/*` | Mail | MemoryTransport 计数 ≥1 |

默认 Cargo features：`sqlite, password, jwt, auth, websocket, sse, metrics, storage, queue, mail`。`tls` / `redis` 为可选，默认不启用。

验收日志样例见 [`docs/FEATURE_VERIFY.md`](docs/FEATURE_VERIFY.md)；旁路示例（blog / multi-app / redis·db SKIP）见 [`docs/SIDE_EXAMPLES.md`](docs/SIDE_EXAMPLES.md)。

## 开发

```bash
cp .env.example .env
npm install
px migrate
px status
px dev
```

打开 <http://127.0.0.1:3000/>。另开终端跑统一验收：

```bash
./scripts/verify-features.sh
# 或 BASE_URL=http://127.0.0.1:3000 VERIFY_OUT=/tmp/phoenix-feature-verify-dev ./scripts/verify-features.sh
cargo run --quiet -- greet   # → smoke-hello
```

`RATE_LIMIT_REQUESTS` 建议 ≥300（`.env.example` 已给），否则脚本连打可能被限流。

### 手动 Notes 验收（可选）

```bash
curl -sS -c /tmp/smoke-cookies -D /tmp/smoke-headers -o /dev/null http://127.0.0.1:3000/notes
CSRF=$(rg -i '^x-csrf-token:' /tmp/smoke-headers | awk '{print $2}' | tr -d '\r')
curl -sS -b /tmp/smoke-cookies -H "Content-Type: application/json" \
  -H "X-CSRF-Token: $CSRF" -d '{"name":"dev-note-1"}' \
  http://127.0.0.1:3000/notes
```

## 生产构建与本地启动

```bash
npm run build
cargo run --bin phoenix-manage -- migrate
cargo run -- serve
./scripts/verify-features.sh
```

## Release 产物验收

```bash
px release --version 0.2.0 --tarball
STAGING=dist/releases/0.2.0/staging
mkdir -p "$STAGING/storage"   # 本地 staging 烟测需空目录；正式 install 会链到 shared/storage
# cwd 必须是 staging 根（或从 bin/ 启动，console 会 chdir 到 release 根）
(cd "$STAGING" && ./bin/phoenix-manage migrate)
(cd "$STAGING/bin" && ./render-modes-smoke serve)
# 另开终端：
./scripts/verify-features.sh
./dist/releases/0.2.0/staging/bin/render-modes-smoke greet
```

## 渲染模式抽查

```bash
for path in spa islands ssr; do
  curl -sS -D - -o /dev/null "http://127.0.0.1:3000/$path" \
    | rg -i '^x-phoenix-render-mode:'
done
```
