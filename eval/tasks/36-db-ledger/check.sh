#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
ledger() { rm -rf "$T/input"; fixture "run$1" input; run dir:input="$T/input" db:db="$T/ledger.db" log:log="$T/ledger.log" clock:clock="fixed:$2"; }
ledger 1 1794301200000
expect - <<'EOF'
ok 7 failed 10 duplicate 1
Zoë 0 last 2026-11-10T09:00:00Z
alice 25 last 2026-11-10T09:00:00Z
bob 800 last 2026-11-10T09:00:00Z
carol 700 last 2026-11-10T09:00:00Z
EOF
ledger 2 1798761599999
expect - <<'EOF'
ok 2 failed 9 duplicate 1
alice 25 last 2026-11-10T09:00:00Z
bob 750 last 2026-12-31T23:59:59Z
carol 750 last 2026-12-31T23:59:59Z
EOF
ledger 3 1835438400000
expect - <<'EOF'
ok 1 failed 3 duplicate 1
alice 25 last 2026-11-10T09:00:00Z
bob 750 last 2026-12-31T23:59:59Z
carol 750 last 2026-12-31T23:59:59Z
erin 9223372036854775807 last 2028-02-29T12:00:00Z
EOF
expect_file "$T/ledger.log" want.log
