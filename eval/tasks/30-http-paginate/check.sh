#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
api routes.json
run http:api="$API" methods:api=POST
expect - <<'EOF'
sum-7f3a
EOF
want='{"largest":3,"orders":8,"paid_cents":{"EUR":5250,"GBP":1,"USD":7999},"refunded":2}'
[ "$(grep -c '^POST ' "$T/api.log" 2>/dev/null)" = 1 ] || fail "expected exactly one POST, the API saw: $(cat "$T/api.log" 2>/dev/null)"
got=$(grep '^POST /summaries ' "$T/api.log" | cut -d' ' -f3- | python3 "$LIB/canon.py")
[ "$got" = "$want" ] || { echo "FAIL: the POSTed summary differs"; echo "  want: $want"; echo "  got:  $got"; exit 1; }
