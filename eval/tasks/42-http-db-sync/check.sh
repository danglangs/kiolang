#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
api routes.json
run http:api="$API" db:db="$T/inv.db"
expect - <<'EOF'
pages 2 applied 6 stale 1 invalid 0
stopped at c2 (status 500)
A1 4 Anvil
C3 2 Café crème
D4 1 Drill
EOF
run http:api="$API" db:db="$T/inv.db"
expect - <<'EOF'
pages 1 applied 1 stale 1 invalid 2
stopped at c3 (invalid page)
A1 4 Anvil
C3 2 Café crème
D4 1 Drill
E5 7 Éclair
EOF
run http:api="$API" db:db="$T/inv.db"
expect - <<'EOF'
pages 1 applied 3 stale 0 invalid 0
complete
A1 4 Anvil
B2 10 Bolt v2
D4 1 Drill
E5 7 Éclair
EOF
run http:api="$API" db:db="$T/inv.db"
expect - <<'EOF'
pages 1 applied 0 stale 3 invalid 0
complete
A1 4 Anvil
B2 10 Bolt v2
D4 1 Drill
E5 7 Éclair
EOF
got=$(grep -o '^GET /changes?cursor=[^ ]*' "$T/api.log" | sort | uniq -c | awk '{print $3, $1}' | tr '\n' ' ')
want="/changes?cursor=0 1 /changes?cursor=c1 1 /changes?cursor=c2 2 /changes?cursor=c3 3 "
[ "$got" = "$want" ] || fail "pages fetched over the four runs: want '$want', got '$got'"
