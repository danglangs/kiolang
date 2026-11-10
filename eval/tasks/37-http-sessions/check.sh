#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
T0=1794301200000
MIN=60000
start() { serve kv:store="$T/store.kv" clock:clock="fixed:$1" rng:rng=seed:7; }
# login NAME PASSWORD: prints the token, after checking the response's shape.
login() {
  local got
  got=$(req POST /login "{\"name\": \"$1\", \"password\": \"$2\"}")
  python3 -c '
import json, re, sys
status, _, body = sys.argv[1].partition(" ")
b = json.loads(body) if body.startswith("{") else {}
ok = status == "200" and re.fullmatch("[0-9a-f]{32}", str(b.get("token"))) and b.get("expires_in") == 1800 and len(b) == 2
print(b["token"] if ok else "")' "$got" >"$T/tok"
  [ -s "$T/tok" ] || fail "POST /login as $1: want 200 {\"token\": 32 hex characters, \"expires_in\": 1800}, got: $got"
}

start $T0
check_req POST /users '{"name": "ann", "password": "hunter22!"}' -- '201 {"name":"ann"}'
check_req POST /users '{"name": "ann", "password": "other password"}' -- '409 {"error":"name taken"}'
check_req POST /users '{"name": "Bo", "password": "longenough"}' -- '400 {"error":"invalid"}'
check_req POST /users '{"name": "bob", "password": "short"}' -- '400 {"error":"invalid"}'
check_req POST /users '{"name": "this_name_is_too_long", "password": "longenough"}' -- '400 {"error":"invalid"}'
check_req POST /users '{"name": "bob"}' -- '400 {"error":"invalid"}'
check_req POST /users 'nope' -- '400 {"error":"invalid"}'
check_req POST /users '{"name": "bob", "password": "correct horse"}' -- '201 {"name":"bob"}'
grep -a -q 'hunter22' "$T/store.kv" && fail "the password is stored in clear"
check_req POST /login '{"name": "ann", "password": "wrong"}' -- '401 {"error":"bad credentials"}'
check_req POST /login '{"name": "nobody", "password": "whatever1"}' -- '401 {"error":"bad credentials"}'
check_req POST /login '[]' -- '401 {"error":"bad credentials"}'
login ann 'hunter22!'; A=$(cat "$T/tok")
check_req GET /me -- '401 {"error":"unauthorized"}'
HDR="authorization: Bearer $A" check_req GET /me -- '200 {"name":"ann","logins":1}'
HDR="authorization: Bearer 0123456789abcdef0123456789abcdef" check_req GET /me -- '401 {"error":"unauthorized"}'
HDR="authorization: $A" check_req GET /me -- '401 {"error":"unauthorized"}'
check_req POST /login '{"name": "bob", "password": "wrong 1"}' -- '401 {"error":"bad credentials"}'
check_req POST /login '{"name": "bob", "password": "wrong 2"}' -- '401 {"error":"bad credentials"}'
check_req POST /login '{"name": "bob", "password": "wrong 3"}' -- '401 {"error":"bad credentials"}'
check_req POST /login '{"name": "bob", "password": "correct horse"}' -- '429 {"error":"locked"}'
check_req POST /login '{"name": "bob", "password": "wrong 4"}' -- '429 {"error":"locked"}'
login ann 'hunter22!'; B=$(cat "$T/tok")
[ "$A" != "$B" ] || fail "two logins returned the same token"
HDR="authorization: Bearer $B" check_req GET /me -- '200 {"name":"ann","logins":2}'
HDR="authorization: Bearer $B" check_req POST /logout -- '204'
HDR="authorization: Bearer $B" check_req GET /me -- '401 {"error":"unauthorized"}'
HDR="authorization: Bearer $B" check_req POST /logout -- '401 {"error":"unauthorized"}'
check_req GET /users -- '404 {"error":"not found"}'
check_req DELETE /me -- '404 {"error":"not found"}'
stop

# 20 minutes later: A is still valid, and using it moves its expiry to +50; bob's lock is over.
start $((T0 + 20 * MIN))
HDR="authorization: Bearer $A" check_req GET /me -- '200 {"name":"ann","logins":2}'
login bob 'correct horse'; C=$(cat "$T/tok")
HDR="authorization: Bearer $C" check_req GET /me -- '200 {"name":"bob","logins":1}'
check_req POST /login '{"name": "bob", "password": "wrong 5"}' -- '401 {"error":"bad credentials"}'
check_req POST /login '{"name": "bob", "password": "wrong 6"}' -- '401 {"error":"bad credentials"}'
login bob 'correct horse'
check_req POST /login '{"name": "bob", "password": "wrong 7"}' -- '401 {"error":"bad credentials"}'
stop

# At +49 both A (until +50) and C (until +50) are valid; A moves to +79.
start $((T0 + 49 * MIN))
HDR="authorization: Bearer $A" check_req GET /me -- '200 {"name":"ann","logins":2}'
stop

# At +79 A has just expired; C expired at +50.
start $((T0 + 79 * MIN))
HDR="authorization: Bearer $A" check_req GET /me -- '401 {"error":"unauthorized"}'
HDR="authorization: Bearer $C" check_req GET /me -- '401 {"error":"unauthorized"}'
check_req POST /login '{"name": "bob", "password": "wrong 8"}' -- '401 {"error":"bad credentials"}'
login bob 'correct horse'; D=$(cat "$T/tok")
HDR="authorization: Bearer $D" check_req GET /me -- '200 {"name":"bob","logins":3}'
