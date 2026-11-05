#!/bin/sh
# kio demo: run from the repo root after `cargo install --path host`.
# Each step pauses; press Enter to continue. Record the terminal in one take.
set -u
cd "$(dirname "$0")/../.."
rm -f examples/tour/out/total.txt
step() { printf '\n\033[1m%s\033[0m\n' "$1"; printf '$ %s\n' "$2"; read -r _; sh -c "$2"; }

step "1. An agent wrote a program. What can it touch?" \
     "cat examples/tour/summarize.kio && kio check examples/tour/summarize.kio"
step "2. The operator grants exactly two directories, and it runs." \
     "kio run examples/tour/summarize.kio --dir input=examples/tour/in --dir out=examples/tour/out && cat examples/tour/out/total.txt"
step "3. summarize takes log: Http, so it may use the network. The report shows that it does." \
     "cat examples/tour/leaky.kio && kio check examples/tour/leaky.kio"
step "4. Same helper without log: Http in its signature. It cannot reach the network: rejected." \
     "cat examples/tour/sneaky.kio && kio check examples/tour/sneaky.kio"
step "5. A path out of the granted directory: refused by the host, and logged." \
     "cat examples/tour/escape.kio && kio run examples/tour/escape.kio --dir input=examples/tour/in --audit /tmp/kio-audit.jsonl; grep '"event":"call"' /tmp/kio-audit.jsonl"
step "6. An infinite loop: stopped by fuel." \
     "kio run examples/tour/forever.kio --fuel 50000000"
printf '\nAI can write the code; the operator controls what it can touch.\n'
