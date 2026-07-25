# 外部依赖与缺口能力验收（Docker）

用临时 Docker 服务补齐 Redis / PostgreSQL / MySQL，并补测此前双路径矩阵未覆盖的 TLS / JWT refresh / Auth / TestApp。

## 一键

```bash
# 需 Docker（本机用 Colima：`colima start`）
./scripts/verify-external-features.sh
```

脚本会：

1. `docker compose -f docker-compose.test-services.yml up -d --wait`
2. 跑 CI 同款 contract / 缺口单测 / smoke TLS（debug + release 二进制）
3. 输出 PASS 表到 `/tmp/phoenix-external-verify/results.txt`

## 服务端口（避开本机其它栈）

| 服务 | 镜像 | Host 端口 |
| --- | --- | --- |
| Redis | `redis:7.4-alpine` | `16379` |
| PostgreSQL | `postgres:17-alpine` | `15432` |
| MySQL | `mysql:8.4` | `13306` |

环境变量：

```bash
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1:16379/0
PHOENIX_TEST_POSTGRES_URL=postgresql://phoenix:phoenix_test_password@127.0.0.1:15432/phoenix_test
PHOENIX_TEST_MYSQL_URL=mysql://phoenix:phoenix_test_password@127.0.0.1:13306/phoenix_test
```

## 测完清理（只删 Docker）

```bash
docker compose -f docker-compose.test-services.yml down -v
# 可选：关掉整个 Docker VM
colima stop
```

自签证书在 `examples/render-modes-smoke/storage/certs/`（被 `storage/*` ignore，不入库）。

## 2026-07-25 实测结果

| 项 | 结果 |
| --- | --- |
| redis.contracts | PASS |
| postgres.contract | PASS |
| mysql.contract | PASS |
| jwt.refresh_revoke（phoenix-crypto `--lib refresh`） | PASS（3 tests） |
| auth.rbac_abac（phoenix-auth `--lib`） | PASS |
| testing.testapp（phoenix-testing `--lib`） | PASS |
| runtime.tls_unit | PASS |
| tls.dev_binary（`cargo build --features tls` + `https://127.0.0.1:3443/hello`） | PASS |
| tls.release_binary（release + 同上） | PASS |

日志目录：`/tmp/phoenix-external-verify/`。
