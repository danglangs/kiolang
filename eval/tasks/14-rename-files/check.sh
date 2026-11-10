#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
mkdir "$T/out"
run dir:input="$T/input" dir:out="$T/out"
expect expected.txt
(cd "$T/out" && ls) >"$T/names"
(cd "$HERE/want" && ls) | diff - "$T/names" >/dev/null || { echo "FAIL: files in out differ"; echo "want: $(cd "$HERE/want" && ls | tr '\n' ' ')"; echo "got:  $(tr '\n' ' ' <"$T/names")"; exit 1; }
for f in $(cat "$T/names"); do cmp -s "$HERE/want/$f" "$T/out/$f" || fail "out/$f has the wrong contents"; done
