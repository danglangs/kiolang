#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" kv:state="$T/state.kv"
expect - <<'EOF'
applied 4 duplicate 1 rejected 3
acme 1000
globex 500
initech 99
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" kv:state="$T/state.kv"
expect - <<'EOF'
applied 2 duplicate 3 rejected 2
acme 1000
globex -500
initech 99
umbrella 10
EOF
