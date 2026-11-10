#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" db:crm="$T/crm.db"
expect - <<'EOF'
inserted 3 updated 1 unchanged 1 rejected 2
contacts 3
missing phone: bob@example.com
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" db:crm="$T/crm.db"
expect - <<'EOF'
inserted 2 updated 2 unchanged 1 rejected 0
contacts 5
missing phone: frank@example.com
missing phone: grace@example.com
EOF
