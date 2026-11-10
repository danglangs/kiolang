#!/bin/bash
. "$(dirname "$0")/../../lib/lib.sh"
python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.executescript(open(sys.argv[2]).read()); c.commit()' "$T/shop.db" "$HERE/shop.sql"
run db:shop="$T/shop.db:ro"
expect - <<'EOF'
US customers=3 orders=5 revenue=30101 top=acme
DE customers=2 orders=3 revenue=16050 top=stark
UK customers=2 orders=2 revenue=5000 top=umbrella
total orders=10 revenue=51151
EOF
