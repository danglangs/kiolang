#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture input input
run dir:input="$T/input"
expect expected.txt
