#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" db:shop="$T/shop.db"
expect - <<'EOF'
imported 5 skipped 1 rejected 2
widget 6250 2
gadget 4999 1
sprocket 3050 1
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" db:shop="$T/shop.db"
expect - <<'EOF'
imported 3 skipped 2 rejected 1
gadget 9998 2
widget 7500 3
gizmo 5994 2
EOF
