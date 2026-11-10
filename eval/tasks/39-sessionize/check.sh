#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
run dir:input="$T/input"
expect - <<'EOF'
ann sessions 2 events 5 longest 00:35:31 pages 4
bob sessions 1 events 4 longest 00:32:00 pages 2
zoë sessions 1 events 2 longest 00:20:00 pages 2
Émile sessions 2 events 3 longest 00:30:00 pages 2
median 1200
top: /home (7), /cart (3), /checkout (1)
malformed 8 duplicates 1
EOF
rm -rf "$T/input"; fixture empty input
run dir:input="$T/input"
expect - <<'EOF'
malformed 0 duplicates 0
EOF
