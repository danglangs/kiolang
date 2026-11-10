#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
run dir:input="$T/input"
expect - <<'EOF'
atrium 2026-11-11
  busy 00:00-24:00
  free none
atrium 2026-11-12
  busy 18:00-19:00
  free 09:00-18:00
blue 2026-11-10
  busy 09:00-11:00, 13:00-13:20, 13:40-14:00
  free 11:00-13:00, 14:00-18:00
  conflict ann/bob 09:30-10:00
red 2026-11-10
  busy 08:00-09:45, 12:00-12:30, 17:45-24:00
  free 09:45-12:00, 12:30-17:45
  conflict gus/hal 12:10-12:20
  conflict ann/gus 12:15-12:25
  conflict ann/hal 12:15-12:20
  conflict ann/hal 12:20-12:25
  conflict gus/hal 12:20-12:25
skipped: 13, 14, 15, 16, 17, 18
EOF
rm -rf "$T/input"; fixture empty input
run dir:input="$T/input"
expect - <<'EOF'
skipped: none
EOF
