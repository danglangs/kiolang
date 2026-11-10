#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" kv:limits="$T/limits.kv"
expect - <<'EOF'
1794225600 alice allow
1794225601 alice allow
1794225602 bob allow
1794225610 alice allow
1794225611 alice deny
1794225640 bob allow
1794225655 alice deny
1794225659 carol allow
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" kv:limits="$T/limits.kv"
expect - <<'EOF'
1794225659 bob allow
1794225659 bob deny
1794225659 carol allow
1794225660 alice allow
1794225661 alice allow
1794225662 alice allow
1794225663 alice deny
1794225719 alice deny
1794225720 alice allow
EOF
