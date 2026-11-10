#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
run dir:input="$T/input"
expect - <<'EOF'
line 1: ok
line 2: $.id: less than 1
line 2: $.items: fewer than 1 items
line 2: $.name: shorter than 2
line 3: $.extra: unexpected field
line 3: $.id: expected integer
line 3: $.items[0].qty: greater than 99
line 3: $.items[0].sku: shorter than 1
line 3: $.items[1].qty: expected integer
line 3: $.items[1].sku: missing required field
line 3: $.items[2]: expected object
line 4: invalid JSON
line 6: $: expected object
line 7: $.active: expected boolean
line 7: $.name: longer than 8
line 7: $.score: greater than 100
line 7: $.tags[1]: longer than 3
line 7: $.tier: not in enum
line 8: ok
line 9: $.id: missing required field
line 9: $.items: missing required field
line 10: ok
line 11: $.items: expected array
line 11: $.note: expected null
line 12: ok
line 13: $.id: expected integer
line 13: $.score: less than -10
line 13: $.tags: expected array
line 14: $.items[10].qty: less than 1
line 14: $.items[1].qty: less than 1
valid 4 of 13
EOF
