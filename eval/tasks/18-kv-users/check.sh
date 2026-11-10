#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" kv:users="$T/users.kv"
expect - <<'EOF'
ok u1
ok u2
{"id":"u1","name":"ann","email":"ann@example.com"}
missing u3
ok u2
ok u3
count 3
ok u3
missing u3
bad frobnicate u1
count 2
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" kv:users="$T/users.kv"
expect - <<'EOF'
{"id":"u2","name":"robert","email":"bob@example.com"}
missing u3
count 2
missing u9
ok u1
{"id":"u1","name":"ann","email":"ann@new.example"}
ok u10
count 3
bad get
EOF
