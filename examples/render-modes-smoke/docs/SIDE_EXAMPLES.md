# Phoenix dual-path side acceptance report

Date: 2026-07-25  
Repo: `/Users/shcodegojo/Project/Phoenix`  
PATH used: `/usr/bin:/bin:/opt/homebrew/bin:$HOME/.cargo/bin`

## Summary

| Item | Result |
|------|--------|
| blog example (`phoenix-blog-example`) | **PASS** |
| multi-app example (`phoenix-multi-app-example`) | **PASS** |
| redis crate tests (`phoenix-redis`) | **SKIP** |
| PostgreSQL contract (`phoenix-database` postgresql) | **SKIP** |
| MySQL contract (`phoenix-database` mysql) | **SKIP** |

---

## 1) Blog example — PASS

### Commands

```bash
cd /Users/shcodegojo/Project/Phoenix
cargo build -p phoenix-blog-example --release
APP_ADDR=127.0.0.1:3001 ./target/release/phoenix-blog-example
curl -sS -w "%{http_code}" http://127.0.0.1:3001/health
curl -sS -w "%{http_code}" http://127.0.0.1:3001/login
curl -sS -w "%{http_code}" -X POST http://127.0.0.1:3001/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"x@example.com","password":"bad"}'
# stop: kill only the release binary PID for phoenix-blog-example
```

### Notes

- No `examples/blog/.env.example` present; binary uses `APP_ADDR` (default `127.0.0.1:3000`). Started on **3001**.
- Binary: `target/release/phoenix-blog-example`
- Build: Finished `release` profile successfully.

### Smoke

| Request | Status | Body / note |
|---------|--------|-------------|
| `GET /health` | **200** | `{"route":"health","status":"healthy"}` |
| `GET /login` | **405** | Method not allowed (route is POST-only) — endpoint present |
| `POST /login` | **403** | `CSRF token mismatch` — login route reachable without CSRF |

Process stopped after smoke. Only port 3001 was used for this item.

---

## 2) Multi-app example — PASS

### Commands

```bash
cd /Users/shcodegojo/Project/Phoenix
cargo build -p phoenix-multi-app-example --release
./target/release/phoenix-multi-app-example
curl -sS http://127.0.0.1:3000/
curl -sS http://127.0.0.1:3000/app
curl -sS http://127.0.0.1:3000/admin
curl -sS http://127.0.0.1:3000/app/account
curl -sS http://127.0.0.1:3000/admin/users
# stop: kill only the release binary PID for phoenix-multi-app-example
```

### Notes

- Binary hardcodes bind address `127.0.0.1:3000` in `examples/multi-app/src/main.rs` (no `APP_ADDR`).
- Prefer port **3002** not possible without rebuild; **3000 was free**, so server ran on 3000. No pre-existing listener on 3000.
- Binary: `target/release/phoenix-multi-app-example`
- Build: Finished `release` profile successfully.

### Smoke

| Request | Status | Body |
|---------|--------|------|
| `GET /` | **200** | `Official website [website]` |
| `GET /app` | **200** | `Customer frontend [frontend]` |
| `GET /admin` | **200** | `Administration [admin]` |
| `GET /app/account` | **200** | `Customer frontend [frontend]` |
| `GET /admin/users` | **200** | `Administration [admin]` |

Process stopped after smoke.

---

## 3) Redis / PostgreSQL / MySQL crate tests

CI reference (`.github/workflows/ci.yml`):

```bash
# Redis
PHOENIX_TEST_REDIS_URL=redis://127.0.0.1:6379/0 \
  cargo test --locked -p phoenix-redis --test contracts --features jwt

# PostgreSQL
PHOENIX_TEST_POSTGRES_URL=postgresql://phoenix:phoenix_test_password@127.0.0.1:5432/phoenix_test \
  cargo test --locked -p phoenix-database --test toasty_integration \
  --no-default-features --features postgresql \
  postgresql_crud_relations_and_pagination_when_configured -- --exact

# MySQL
PHOENIX_TEST_MYSQL_URL=mysql://phoenix:phoenix_test_password@127.0.0.1:3306/phoenix_test \
  cargo test --locked -p phoenix-database --test toasty_integration \
  --no-default-features --features mysql \
  mysql_crud_relations_and_pagination_when_configured -- --exact
```

### Env / service probe

| Check | Result |
|-------|--------|
| `REDIS_URL` / `PHOENIX_TEST_REDIS_URL` | unset |
| `DATABASE_URL` / `PHOENIX_TEST_POSTGRES_URL` / `PHOENIX_TEST_MYSQL_URL` | unset |
| `127.0.0.1:6379` (redis) | connection refused |
| `127.0.0.1:5432` (postgres) | connection refused |
| `127.0.0.1:3306` (mysql) | connection refused |

Docker services were **not** started (per instructions).

| Suite | Result | Reason |
|-------|--------|--------|
| `cargo test -p phoenix-redis --test contracts --features jwt` | **SKIP** | no `PHOENIX_TEST_REDIS_URL` / `REDIS_URL`; redis not listening |
| PostgreSQL `toasty_integration` exact test | **SKIP** | no `PHOENIX_TEST_POSTGRES_URL`; postgres not listening |
| MySQL `toasty_integration` exact test | **SKIP** | no `PHOENIX_TEST_MYSQL_URL`; mysql not listening |

---

## Overall

**Side-path examples: PASS (2/2).**  
**Optional DB/redis contract tests: SKIP (3/3, no env/service).**
