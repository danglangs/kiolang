#!/bin/bash
# Run every task's reference solutions against its checks: ref.kio with check.sh,
# ref.py with check_py.sh. KIO names the kio binary (default: `kio` on PATH).
# Usage: eval/check_refs.sh [TASK_DIR..]
KIO=${KIO:-kio}
case $KIO in */*) KIO=$(cd "$(dirname "$KIO")" && pwd)/$(basename "$KIO") ;; esac
export KIO
tasks=("$@")
[ ${#tasks[@]} -eq 0 ] && tasks=("$(dirname "$0")"/tasks/*/)
failed=0
for d in "${tasks[@]}"; do
  d=${d%/}
  for pair in "check.sh ref.kio" "check_py.sh ref.py"; do
    set -- $pair
    if out=$("$d/$1" "$d/$2" 2>&1); then
      echo "ok   $(basename "$d") $2"
    else
      echo "FAIL $(basename "$d") $2"
      echo "$out" | sed 's/^/     /' | head -30
      failed=$((failed + 1))
    fi
  done
done
[ $failed -eq 0 ] && echo "all references pass" || echo "$failed failed"
[ $failed -eq 0 ]
