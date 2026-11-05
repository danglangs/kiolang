#!/bin/sh
# kio demo: run from the repo root after `cargo install --path host`.
# Each step pauses; press Enter to continue. Record the terminal in one take.
set -u
cd "$(dirname "$0")/../.."
rm -f examples/tour/out/total.txt
step() { printf '\n\033[1m%s\033[0m\n' "$1"; printf '$ %s\n' "$2"; read -r _; sh -c "$2"; }

step "1. An agent wrote a program. What can it touch?" \
     "kio check examples/tour/summarize.kio"
step "2. The operator grants exactly two directories, and it runs." \
     "kio run examples/tour/summarize.kio --dir input=examples/tour/in --dir out=examples/tour/out && cat examples/tour/out/total.txt"
step "3. A helper that looks pure but phones home: the report shows it." \
     "kio check examples/tour/leaky.kio"
step "4. A helper using the network without being given it: rejected before running." \
     "kio check examples/tour/sneaky.kio"
step "5. A path out of the granted directory: refused by the host, and logged." \
     "kio run examples/tour/escape.kio --dir input=examples/tour/in --audit /tmp/kio-audit.jsonl; grep '"event":"call"' /tmp/kio-audit.jsonl"
step "6. An infinite loop: stopped by fuel." \
     "kio run examples/tour/forever.kio --fuel 50000000"
printf '\nAI can write the code; the operator controls what it can touch.\n'
