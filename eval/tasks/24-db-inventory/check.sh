#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" db:stock="$T/stock.db"
expect - <<'EOF'
receive-17 applied
ship-9 rejected
ship-10 applied
receive-17 skipped
adjust-1 applied
typo-1 rejected
widget 27
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" db:stock="$T/stock.db"
expect - <<'EOF'
ship-10 skipped
ship-11 applied
receive-18 applied
ship-12 rejected
gizmo 7
widget 1
EOF
