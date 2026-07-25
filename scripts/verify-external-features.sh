#!/usr/bin/env bash
# Start Docker test services (if needed), run previously-SKIPPED / gap contract tests,
# optional TLS smoke, then print a PASS/FAIL/SKIP table.
#
# Teardown when finished:
#   docker compose -f docker-compose.test-services.yml down -v
#   # optional: colima stop
set -euo pipefail
export PATH="/usr/bin:/bin:/opt/homebrew/bin:${HOME}/.cargo/bin:${PATH}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.test-services.yml}"
OUT="${VERIFY_OUT:-/tmp/phoenix-external-verify}"
mkdir -p "$OUT"
: >"$OUT/results.txt"

# Prefer Colima socket when present.
if [[ -z "${DOCKER_HOST:-}" && -S "${HOME}/.colima/default/docker.sock" ]]; then
  export DOCKER_HOST="unix://${HOME}/.colima/default/docker.sock"
fi

export PHOENIX_TEST_REDIS_URL="${PHOENIX_TEST_REDIS_URL:-redis://127.0.0.1:16379/0}"
export PHOENIX_TEST_POSTGRES_URL="${PHOENIX_TEST_POSTGRES_URL:-postgresql://phoenix:phoenix_test_password@127.0.0.1:15432/phoenix_test}"
export PHOENIX_TEST_MYSQL_URL="${PHOENIX_TEST_MYSQL_URL:-mysql://phoenix:phoenix_test_password@127.0.0.1:13306/phoenix_test}"

pass=0
fail=0
skip=0

record() {
  local status="$1"
  local name="$2"
  local detail="${3:-}"
  printf '%-6s %s %s\n' "$status" "$name" "$detail" | tee -a "$OUT/results.txt"
  case "$status" in
    PASS) pass=$((pass + 1)) ;;
    FAIL) fail=$((fail + 1)) ;;
    SKIP) skip=$((skip + 1)) ;;
  esac
}

run_test() {
  local name="$1"
  shift
  local log="$OUT/${name}.log"
  if "$@" >"$log" 2>&1; then
    record PASS "$name"
  else
    record FAIL "$name" "see $log"
    tail -20 "$log" | sed 's/^/  | /' || true
  fi
}

ensure_docker() {
  if ! docker info >/dev/null 2>&1; then
    if command -v colima >/dev/null 2>&1; then
      echo "Starting Colima…"
      colima start
      export DOCKER_HOST="unix://${HOME}/.colima/default/docker.sock"
    else
      echo "Docker daemon not available" >&2
      exit 1
    fi
  fi
}

wait_tcp() {
  local host="$1" port="$2" tries="${3:-60}"
  for _ in $(seq 1 "$tries"); do
    if (echo >/dev/tcp/"$host"/"$port") >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

ensure_services() {
  ensure_docker
  echo "Bringing up $COMPOSE_FILE …"
  docker compose -f "$COMPOSE_FILE" up -d --wait
  wait_tcp 127.0.0.1 16379
  wait_tcp 127.0.0.1 15432
  wait_tcp 127.0.0.1 13306
}

ensure_tls_certs() {
  local dir="$ROOT/examples/render-modes-smoke/storage/certs"
  mkdir -p "$dir"
  if [[ ! -f "$dir/certificate.pem" || ! -f "$dir/private-key.pem" ]]; then
    openssl req -x509 -newkey rsa:2048 -nodes \
      -keyout "$dir/private-key.pem" \
      -out "$dir/certificate.pem" \
      -days 30 \
      -subj "/CN=127.0.0.1" \
      -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" \
      >/dev/null 2>&1
  fi
}

tls_smoke() {
  local name="$1"
  local bin="$2"
  local cwd="$3"
  local addr="${APP_TLS_ADDR:-127.0.0.1:3443}"
  local log="$OUT/${name}.serve.log"
  local certs="$cwd/storage/certs"
  mkdir -p "$certs"
  if [[ ! -f "$certs/certificate.pem" ]]; then
    ensure_tls_certs
    cp -f "$ROOT/examples/render-modes-smoke/storage/certs/"*.pem "$certs/" 2>/dev/null || true
  fi
  local port="${addr##*:}"
  lsof -ti:"$port" | xargs kill -9 2>/dev/null || true
  (
    cd "$cwd"
    APP_TLS_ADDR="$addr" \
      APP_TLS_CERT="$certs/certificate.pem" \
      APP_TLS_KEY="$certs/private-key.pem" \
      "$bin" serve
  ) >"$log" 2>&1 &
  local pid=$!
  local ok=0
  for _ in $(seq 1 60); do
    if curl -sk --max-time 2 "https://127.0.0.1:${port}/hello" | rg -q 'smoke-hello'; then
      ok=1
      break
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      break
    fi
    sleep 0.5
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  if [[ "$ok" -eq 1 ]]; then
    record PASS "$name"
  else
    record FAIL "$name" "see $log"
    tail -40 "$log" | sed 's/^/  | /' || true
  fi
}

echo "== external feature verify =="
echo "REDIS_URL=$PHOENIX_TEST_REDIS_URL"
echo "POSTGRES_URL=$PHOENIX_TEST_POSTGRES_URL"
echo "MYSQL_URL=$PHOENIX_TEST_MYSQL_URL"
ensure_services

run_test redis.contracts \
  cargo test --locked -p phoenix-redis --test contracts --features jwt

run_test postgres.contract \
  cargo test --locked -p phoenix-database --test toasty_integration \
  --no-default-features --features postgresql \
  postgresql_crud_relations_and_pagination_when_configured -- --exact

run_test mysql.contract \
  cargo test --locked -p phoenix-database --test toasty_integration \
  --no-default-features --features mysql \
  mysql_crud_relations_and_pagination_when_configured -- --exact

run_test jwt.refresh_revoke \
  cargo test --locked -p phoenix-crypto --lib refresh

run_test auth.rbac_abac \
  cargo test --locked -p phoenix-auth --lib

run_test testing.testapp \
  cargo test --locked -p phoenix-testing --lib

run_test runtime.tls_unit \
  cargo test --locked -p phoenix-runtime --lib tls

# Dual-path TLS against smoke (dev binary + release binary if present)
SMOKE="$ROOT/examples/render-modes-smoke"
ensure_tls_certs
(
  cd "$SMOKE"
  cargo build --features tls --bin render-modes-smoke >"$OUT/smoke-tls-build.log" 2>&1
)
tls_smoke tls.dev_binary "$SMOKE/target/debug/render-modes-smoke" "$SMOKE"

(
  cd "$SMOKE"
  cargo build --release --features tls --bin render-modes-smoke >"$OUT/smoke-tls-release-build.log" 2>&1
)
tls_smoke tls.release_binary "$SMOKE/target/release/render-modes-smoke" "$SMOKE"

echo
echo "SUMMARY pass=$pass fail=$fail skip=$skip"
echo "Results: $OUT/results.txt"
echo "Teardown: docker compose -f $COMPOSE_FILE down -v"
if [[ "$fail" -gt 0 ]]; then
  exit 1
fi
