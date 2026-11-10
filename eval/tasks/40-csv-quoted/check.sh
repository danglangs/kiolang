#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
mkdir "$T/out"
run dir:input="$T/input" dir:out="$T/out"
expect - <<'EOF'
north revenue 12.50 items 2 top gadget, large
south revenue 25.99 items 5 top gadget, large
west revenue 1.00 items 10 top thing
records 15 bad 8: 5, 6, 7, 8, 11, 12, 13, 15
EOF
[ -f "$T/out/clean.csv" ] || fail "no out/clean.csv"
if ! cmp -s "$T/out/clean.csv" "$HERE/want_clean.csv"; then
  echo "FAIL: out/clean.csv differs (bytes; CR shown as \\r)"
  diff <(sed 's/\r/\\r/g' "$HERE/want_clean.csv") <(sed 's/\r/\\r/g' "$T/out/clean.csv") | head -30
  exit 1
fi
