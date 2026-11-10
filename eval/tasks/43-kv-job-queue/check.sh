#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
T0=1794301200000
work() { rm -rf "$T/input"; fixture "run$1" input; run dir:input="$T/input" kv:queue="$T/queue.kv" clock:clock="fixed:$((T0 + $2))"; }
work 1 0
expect - <<'EOF'
duplicate j1
invalid line 6
invalid line 10
done j2
retry j4 in 60
retry j1 in 60
retry j3 in 60
done j5
pending 4 done 2 dead 0
EOF
work 2 30000
expect - <<'EOF'
duplicate j2
invalid line 3
done j7
retry j6 in 60
pending 4 done 3 dead 0
EOF
work 3 300000
expect - <<'EOF'
retry j4 in 120
retry j1 in 120
done j3
done j6
pending 2 done 5 dead 0
EOF
work 4 600000
expect - <<'EOF'
done j4
retry j1 in 240
retry j9 in 60
pending 2 done 6 dead 0
EOF
work 5 840000
expect - <<'EOF'
dead j1
done j9
pending 0 done 7 dead 1
EOF
