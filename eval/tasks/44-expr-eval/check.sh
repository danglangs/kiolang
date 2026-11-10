#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
run dir:input="$T/input"
expect - <<'EOF'
1: 7
2: 9
3: x = 7
4: y = -14
5: 3
6: -3
7: -1
8: 1
9: 512
10: -4
11: -9223372036854775808
12: error: overflow
13: error: negative exponent
14: 0
15: error: overflow
16: error: overflow
17: -9223372036854775808
18: error: overflow
19: error: overflow
20: error: unknown variable big
21: error: unknown variable z
22: error: division by zero
23: error: division by zero
24: error: syntax error
25: error: syntax error
26: error: syntax error
27: error: syntax error
28: error: syntax error
29: error: unknown function foo
30: error: wrong number of arguments to min
32: -21
33: x = 8
34: 8
35: error: syntax error
36: 1
37: error: division by zero
38: error: syntax error
39: 3
40: error: unknown function foo
41: 9000000000000000000
42: -9000000000000000000
43: 9223372030926249001
EOF
