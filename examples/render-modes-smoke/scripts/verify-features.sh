#!/usr/bin/env bash
# Dual-path feature smoke checks against a running render-modes-smoke server.
set -euo pipefail
export PATH="/usr/bin:/bin:/opt/homebrew/bin:${HOME}/.cargo/bin:${PATH}"

BASE="${BASE_URL:-http://127.0.0.1:3000}"
WS_URL="${WS_URL:-ws://127.0.0.1:3000/features/ws}"
OUT="${VERIFY_OUT:-/tmp/phoenix-feature-verify}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
: >"$OUT/results.txt"

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

csrf_session() {
  curl -sS -c "$OUT/cookies" -D "$OUT/headers" -o /dev/null "$BASE/notes"
  CSRF="$(rg -i '^x-csrf-token:' "$OUT/headers" | awk '{print $2}' | tr -d '\r')"
  if [[ -z "${CSRF:-}" ]]; then
    echo "failed to obtain CSRF token" >&2
    exit 1
  fi
}

post_json() {
  local path="$1"
  local body="$2"
  curl -sS -b "$OUT/cookies" \
    -H "Content-Type: application/json" \
    -H "X-CSRF-Token: $CSRF" \
    -d "$body" \
    -w "\nHTTP:%{http_code}" \
    "$BASE$path"
}

echo "Verifying features at $BASE"
csrf_session

# Core render modes
for path_mode in spa:spa islands:islands ssr:ssr; do
  path="${path_mode%%:*}"
  want="${path_mode##*:}"
  mode="$(curl -sS -D - -o /dev/null "$BASE/$path" | rg -i '^x-phoenix-render-mode:' | awk '{print $2}' | tr -d '\r' || true)"
  if [[ "$mode" == "$want" ]]; then
    record PASS "render.$path" "mode=$mode"
  else
    record FAIL "render.$path" "got=$mode want=$want"
  fi
done

# Plugin greeter
hello="$(curl -sS "$BASE/hello")"
if echo "$hello" | rg -q 'smoke-hello'; then
  record PASS plugin.hello "$hello"
else
  record FAIL plugin.hello "$hello"
fi

# SSE
sse_headers="$(curl -sS -D - --max-time 2 -o "$OUT/sse.body" "$BASE/features/sse" || true)"
if echo "$sse_headers" | rg -qi 'text/event-stream' && rg -q 'data: hello' "$OUT/sse.body"; then
  record PASS features.sse
else
  record FAIL features.sse
fi

# WebSocket
if node "$SCRIPT_DIR/ws_ping.mjs" "$WS_URL" >/dev/null 2>"$OUT/ws.err"; then
  record PASS features.ws
else
  record FAIL features.ws "$(tr '\n' ' ' <"$OUT/ws.err")"
fi

# Metrics
metrics="$(curl -sS "$BASE/internal/metrics")"
if echo "$metrics" | rg -q 'phoenix_http_requests_total'; then
  record PASS features.metrics
else
  record FAIL features.metrics
fi

# Password
hash_resp="$(post_json /features/password/hash '{"password":"correct horse battery"}')"
hash_body="${hash_resp%$'\n'HTTP:*}"
hash_code="${hash_resp##*$'\n'HTTP:}"
HASH="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["hash"])' "$hash_body" 2>/dev/null || true)"
if [[ "$hash_code" == "200" && -n "$HASH" ]]; then
  record PASS features.password.hash
else
  record FAIL features.password.hash "code=$hash_code"
fi
verify_ok="$(post_json /features/password/verify "$(python3 -c 'import json,sys; print(json.dumps({"password":"correct horse battery","hash":sys.argv[1]}))' "$HASH")")"
verify_body="${verify_ok%$'\n'HTTP:*}"
if echo "$verify_body" | rg -q '"ok":\s*true'; then
  record PASS features.password.verify
else
  record FAIL features.password.verify "$verify_body"
fi
verify_bad="$(post_json /features/password/verify "$(python3 -c 'import json,sys; print(json.dumps({"password":"wrong","hash":sys.argv[1]}))' "$HASH")")"
verify_bad_body="${verify_bad%$'\n'HTTP:*}"
if echo "$verify_bad_body" | rg -q '"ok":\s*false'; then
  record PASS features.password.reject
else
  record FAIL features.password.reject "$verify_bad_body"
fi

# JWT + Auth
token_resp="$(post_json /features/jwt/token '{"role":"admin"}')"
token_body="${token_resp%$'\n'HTTP:*}"
TOKEN="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["token"])' "$token_body" 2>/dev/null || true)"
if [[ -n "$TOKEN" ]]; then
  record PASS features.jwt.token
else
  record FAIL features.jwt.token "$token_body"
fi
me="$(curl -sS -H "Authorization: Bearer $TOKEN" "$BASE/features/jwt/me")"
if echo "$me" | rg -q 'smoke-user'; then
  record PASS features.jwt.me
else
  record FAIL features.jwt.me "$me"
fi
admin="$(curl -sS -D "$OUT/admin.h" -o "$OUT/admin.body" -H "Authorization: Bearer $TOKEN" "$BASE/features/admin")"
admin_code="$(rg -i '^HTTP/' "$OUT/admin.h" | awk '{print $2}' | tail -1)"
if [[ "$admin_code" == "200" ]] && rg -q '"ok":\s*true' "$OUT/admin.body"; then
  record PASS features.auth.admin
else
  record FAIL features.auth.admin "code=$admin_code"
fi
guest_resp="$(post_json /features/jwt/token '{"role":"guest"}')"
guest_body="${guest_resp%$'\n'HTTP:*}"
GUEST="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["token"])' "$guest_body" 2>/dev/null || true)"
guest_code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $GUEST" "$BASE/features/admin")"
if [[ "$guest_code" == "403" ]]; then
  record PASS features.auth.forbidden
else
  record FAIL features.auth.forbidden "code=$guest_code"
fi
bad_code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer deadbeef' "$BASE/features/jwt/me")"
if [[ "$bad_code" == "401" ]]; then
  record PASS features.jwt.unauthorized
else
  record FAIL features.jwt.unauthorized "code=$bad_code"
fi

# Storage
store_resp="$(post_json /features/storage '{"key":"demo.txt","content":"hello-storage"}')"
store_code="${store_resp##*$'\n'HTTP:}"
got="$(curl -sS "$BASE/features/storage?key=demo.txt")"
if [[ "$store_code" == "200" && "$got" == "hello-storage" ]]; then
  record PASS features.storage
else
  record FAIL features.storage "code=$store_code body=$got"
fi
bad_store="$(post_json /features/storage '{"key":"../escape.txt","content":"nope"}')"
bad_store_code="${bad_store##*$'\n'HTTP:}"
if [[ "$bad_store_code" == "400" ]]; then
  record PASS features.storage.traverse
else
  record FAIL features.storage.traverse "code=$bad_store_code"
fi

# Queue
queue_resp="$(post_json /features/queue/ping '{}')"
queue_body="${queue_resp%$'\n'HTTP:*}"
if echo "$queue_body" | python3 -c 'import json,sys; d=json.load(sys.stdin); raise SystemExit(0 if d.get("acked",0)>d.get("before",-1) else 1)'; then
  record PASS features.queue
else
  record FAIL features.queue "$queue_body"
fi

# Mail
mail_resp="$(post_json /features/mail/send '{}')"
mail_body="${mail_resp%$'\n'HTTP:*}"
sent="$(curl -sS "$BASE/features/mail/sent")"
if echo "$mail_body" | rg -q '"sent"' && echo "$sent" | python3 -c 'import json,sys; raise SystemExit(0 if json.load(sys.stdin).get("count",0)>=1 else 1)'; then
  record PASS features.mail
else
  record FAIL features.mail "send=$mail_body sent=$sent"
fi

# Notes still work
note_resp="$(post_json /notes '{"name":"feature-verify-note"}')"
note_code="${note_resp##*$'\n'HTTP:}"
if [[ "$note_code" == "201" ]]; then
  record PASS notes.create
else
  record FAIL notes.create "code=$note_code"
fi

echo
echo "SUMMARY pass=$pass fail=$fail skip=$skip"
if [[ "$fail" -gt 0 ]]; then
  exit 1
fi
