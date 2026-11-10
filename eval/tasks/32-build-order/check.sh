#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
rm -rf "$T/input"; fixture main input
run dir:input="$T/input"
expect - <<'EOF'
stage 1: args, e, log, net, z
stage 2: Zed, http, ünï
stage 3: web
cycle: a, b, c
cycle: selfie
cycle: x, y
missing: ghost (needed by driver)
missing: weird (needed by uses-weird)
invalid: bad2, weird
blocked: app, cli, d, db, driver, uses-weird
built 9 of 21
EOF
rm -rf "$T/input"; fixture empty input
run dir:input="$T/input"
expect - <<'EOF'
built 0 of 0
EOF
rm -rf "$T/input"; fixture bad input
run dir:input="$T/input"
expect - <<'EOF'
error: not an object
EOF
