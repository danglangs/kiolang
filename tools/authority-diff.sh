#!/bin/sh
# usage: tools/authority-diff.sh BASE_REF ENTRY.kio [--detail]
#
# The authority ENTRY.kio (in the working tree) adds over its version at BASE_REF:
# `kio check ENTRY --base <BASE_REF's ENTRY>`, with BASE_REF checked out by `git worktree add`
# into a temporary directory that is removed afterwards. Exit 0: no authority added; 3: some
# added; 1: a side does not check; 2: usage or a missing file. KIO names the binary (default kio).
set -eu
detail=
if [ $# -eq 3 ] && [ "$3" = --detail ]; then
    detail=--detail
elif [ $# -ne 2 ]; then
    echo "usage: $0 BASE_REF ENTRY.kio [--detail]" >&2
    exit 2
fi
base_ref=$1
entry=$2
kio=${KIO:-kio}

top=$(git rev-parse --show-toplevel)
case $entry in
    "$top"/*) rel=${entry#"$top"/} ;;
    /*) echo "$0: $entry is outside the repository $top" >&2; exit 2 ;;
    *) rel=$(git rev-parse --show-prefix)$entry ;;
esac

tmp=$(mktemp -d)
cleanup() {
    git worktree remove --force "$tmp/base" >/dev/null 2>&1 || true
    rm -rf "$tmp"
}
trap cleanup EXIT
git worktree add --quiet --detach "$tmp/base" "$base_ref"

if [ ! -f "$tmp/base/$rel" ]; then
    echo "$0: $rel does not exist at $base_ref" >&2
    exit 2
fi
status=0
"$kio" check "$entry" --base "$tmp/base/$rel" $detail || status=$?
exit $status
