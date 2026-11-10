#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 input
run dir:input="$T/input" kv:board="$T/board.kv"
expect want1.txt
rm -rf "$T/input"; fixture run2 input
run dir:input="$T/input" kv:board="$T/board.kv"
expect want2.txt
