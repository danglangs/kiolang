#!/bin/bash
# The same check, run against a Python program.
EVAL_LANG=py exec "$(dirname "$0")/check.sh" "$@"
