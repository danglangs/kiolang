#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
mkdir "$T/out"
run dir:input="$T/input" dir:out="$T/out"
expect expected.txt
expect_file "$T/out/index.csv" want/index.csv
