#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture day1 input
run dir:input="$T/input" kv:counts="$T/counts.kv"
expect - <<'EOF'
/home 3
/pricing 2
/blog/kio-1-0 1
/docs/start 1
EOF
rm -rf "$T/input"; fixture day2 input
run dir:input="$T/input" kv:counts="$T/counts.kv"
expect - <<'EOF'
/docs/start 4
/home 3
/pricing 3
/blog/kio-1-0 1
/docs/api 1
/signup 1
EOF
