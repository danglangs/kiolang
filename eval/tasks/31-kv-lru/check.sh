#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" kv:cache="$T/cache.kv"
expect - <<'EOF'
a=1
evict b
b miss
c=ünïcödé ✓
a=1
a= leading space
bad line 13
bad line 14
bad line 15
bad line 16
bad line 17
e=5
evict d
cache: f,e,a
stats hits 5 misses 1 evictions 2
EOF
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" kv:cache="$T/cache.kv"
expect - <<'EOF'
evict a
e=5
a miss
evict f
f miss
e=55
e miss
cache: h,g
stats hits 7 misses 4 evictions 4
EOF
