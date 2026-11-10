#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
api routes.json
run http:api="$API"
expect - <<'EOF'
Airport unavailable
Harbor min=10.1 max=13.4 avg=11.8 n=3
North Ridge min=-4.2 max=1.8 avg=-0.5 n=4
Old Mill no data
Summit min=-0.4 max=-0.3 avg=-0.3 n=2
EOF
