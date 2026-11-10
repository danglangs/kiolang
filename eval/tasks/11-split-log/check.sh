#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
mkdir "$T/out"
run dir:input="$T/input" dir:out="$T/out"
expect expected.txt
for f in error warn info; do expect_file "$T/out/$f.log" "want/$f.log"; done
[ -z "$(tail -c 1 "$T/out/error.log")" ] || fail "out/error.log must end in a newline"
[ -s "$T/out/warn.log" ] || fail "out/warn.log is empty"
