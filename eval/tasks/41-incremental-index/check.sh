#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
fixture run1 r
run dir:docs="$T/r/docs" dir:queries="$T/r/q" kv:index="$T/index.kv"
expect - <<'EOF'
added a.txt
added b.md
added empty.txt
added sub/c.txt
added sub/deep/d.txt
unchanged 0
the fox: a.txt
café: b.md, sub/c.txt
dog 42: sub/deep/d.txt
brown dog: a.txt
TROT: b.md
unknown: none
brûlée the: sub/c.txt
EOF
rm -rf "$T/r"; fixture run2 r
run dir:docs="$T/r/docs" dir:queries="$T/r/q" kv:index="$T/index.kv"
expect - <<'EOF'
changed a.txt
removed b.md
added new/e.txt
unchanged 3
the fox: new/e.txt
café: sub/c.txt
dog 42: sub/deep/d.txt
brown dog: none
TROT: none
unknown: none
brûlée the: sub/c.txt
cat: a.txt, new/e.txt
slow: a.txt
EOF
rm -rf "$T/r/docs"; mkdir "$T/r/docs"
run dir:docs="$T/r/docs" dir:queries="$T/r/q" kv:index="$T/index.kv"
expect - <<'EOF'
removed a.txt
removed empty.txt
removed new/e.txt
removed sub/c.txt
removed sub/deep/d.txt
unchanged 0
the fox: none
café: none
dog 42: none
brown dog: none
TROT: none
unknown: none
brûlée the: none
cat: none
slow: none
EOF
