#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture src src
fixture dst dst
run dir:src="$T/src" dir:dst="$T/dst"
expect expected.txt
diff -r "$HERE/want" "$T/dst" >"$T/diff" || { echo "FAIL: dst after the sync differs from the expected backup"; head -20 "$T/diff"; exit 1; }
diff -r "$HERE/src" "$T/src" >/dev/null || fail "src was changed"
