#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
api routes.json
fixture run1 input
run dir:input="$T/input" kv:state="$T/state.kv" http:api="$API" methods:api=PUT
expect - <<'EOF'
line 6 invalid
line 8 invalid
line 12 invalid
r1 synced
r2 synced
r3 rejected 422
r4 failed
r5 synced
r7 synced
r8 rejected 409
synced 4 unchanged 0 rejected 2 failed 1
EOF
puts() { grep -o '^PUT /records/[^ ]*' "$T/api.log" 2>/dev/null | sort | uniq -c | awk '{print $3, $1}' | tr '\n' ' '; }
got=$(puts)
want="/records/r1 1 /records/r2 2 /records/r3 1 /records/r4 3 /records/r5 1 /records/r7 1 /records/r8 2 "
[ "$got" = "$want" ] || fail "PUTs per record after run 1: want '$want', got '$got'"
body() { grep "^PUT /records/$1 " "$T/api.log" | tail -1 | cut -d' ' -f3- | python3 "$LIB/canon.py"; }
[ "$(body r5)" = '{"id":"r5","name":"epsilon","version":2}' ] || fail "the body sent for r5: $(body r5)"
[ "$(body r7)" = "$(echo '{"id":"r7","name":"ünïcödé","version":1}' | python3 "$LIB/canon.py")" ] || fail "the body sent for r7: $(body r7)"
grep -q 'name' "$T/api.log" || fail "no bodies sent"

rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" kv:state="$T/state.kv" http:api="$API" methods:api=PUT
expect - <<'EOF'
r1 unchanged
r2 synced
r3 rejected 422
r4 synced
r5 unchanged
r7 unchanged
r8 rejected 409
r9 synced
synced 3 unchanged 3 rejected 2 failed 0
EOF
got=$(puts)
want="/records/r1 1 /records/r2 3 /records/r3 2 /records/r4 4 /records/r5 1 /records/r7 1 /records/r8 3 /records/r9 1 "
[ "$got" = "$want" ] || fail "PUTs per record after run 2: want '$want', got '$got'"
[ "$(body r2)" = '{"id":"r2","name":"beta two","version":2}' ] || fail "the body sent for r2 in run 2: $(body r2)"
