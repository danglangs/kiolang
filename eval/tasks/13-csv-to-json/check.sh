#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
mkdir "$T/out"
run dir:input="$T/input" dir:out="$T/out"
expect expected.txt
[ -f "$T/out/users.json" ] || fail "missing out/users.json"
python3 "$LIB/canon.py" <"$T/out/users.json" >"$T/got.json"
python3 "$LIB/canon.py" <"$HERE/want/users.json" >"$T/want.json"
cmp -s "$T/got.json" "$T/want.json" || { echo "FAIL: out/users.json differs"; echo "want: $(cat "$T/want.json")"; echo "got:  $(head -c 3000 "$T/got.json")"; exit 1; }
