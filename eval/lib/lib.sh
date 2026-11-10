# Sourced by every task's check.sh. One check serves both languages:
# EVAL_LANG=kio (default) runs `kio run PROG` with flags, EVAL_LANG=py runs
# `python3 PROG` with the same grants as environment variables (NAME=path or URL).
#
# Grants are written once, as `kind:name=value`:
#   dir:input=PATH  kv:counts=PATH  db:shop=PATH  http:api=URL  methods:api=POST,PUT
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
LIB=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PROG=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
EVAL_LANG=${EVAL_LANG:-kio}
KIO=${KIO:-kio}
T=$(mktemp -d "${TMPDIR:-/tmp}/kio-eval.XXXXXX")
PIDS=()
cleanup() {
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null; wait "$p" 2>/dev/null || true; done
  rm -rf "$T"
}
trap cleanup EXIT

fail() { echo "FAIL: $*"; exit 1; }

# Copy a fixture directory of the task into $T.
fixture() { cp -R "$HERE/$1" "$T/$2"; }

# The command line for the program with these grants, in $CMD (kio) or $ENVS (py).
build() {
  CMD=()
  ENVS=()
  local g kind rest name val
  for g in "$@"; do
    kind=${g%%:*}; rest=${g#*:}; name=${rest%%=*}; val=${rest#*=}
    if [ "$EVAL_LANG" = kio ]; then
      case $kind in
        methods) CMD+=(--http-methods "$name=$val") ;;
        *) CMD+=("--$kind" "$name=$val") ;;
      esac
    else
      local up
      up=$(echo "$name" | tr '[:lower:]' '[:upper:]')
      case $kind in
        methods) ;;
        *) ENVS+=("$up=${val%:ro}") ;;
      esac
    fi
  done
}

# Run the program to completion with these grants; stdout (the value) to $T/stdout.
# A kio value printed as `Ok("..")` is unwrapped to its text. Fails on a non-zero exit.
run() {
  build "$@"
  local rc=0
  if [ "$EVAL_LANG" = kio ]; then
    "$KIO" run "$PROG" ${CMD[@]+"${CMD[@]}"} --timeout 10 >"$T/raw" 2>"$T/stderr" || rc=$?
  else
    (cd "$T" && env ${ENVS[@]+"${ENVS[@]}"} perl -e 'alarm 20; exec @ARGV' python3 "$PROG") >"$T/raw" 2>"$T/stderr" || rc=$?
  fi
  python3 "$LIB/out.py" "$EVAL_LANG" <"$T/raw" >"$T/stdout"
  if [ $rc -ne 0 ]; then
    echo "FAIL: the program exited $rc"
    echo "--- stdout"; head -c 2000 "$T/raw"
    echo "--- stderr"; tail -c 3000 "$T/stderr"
    exit 1
  fi
}

# Compare $T/stdout with the expected text (a file of the task, or stdin with -).
# Trailing spaces and blank lines at the end are ignored.
expect() {
  local want=$1
  [ "$want" = - ] && { cat >"$T/want"; want=$T/want; }
  [ -f "$want" ] || want=$HERE/$want
  if ! python3 "$LIB/same.py" "$want" "$T/stdout"; then
    echo "--- stderr"; tail -c 1500 "$T/stderr"
    exit 1
  fi
}

# Compare a file the program wrote with an expected file.
expect_file() {
  [ -f "$1" ] || fail "missing file $(basename "$1")"
  python3 "$LIB/same.py" "$HERE/$2" "$1" || exit 1
}

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }

# Start the program as an HTTP server on a free port: $PORT, $BASE.
serve() {
  PORT=$(free_port)
  BASE=http://127.0.0.1:$PORT
  build "$@"
  if [ "$EVAL_LANG" = kio ]; then
    "$KIO" run "$PROG" ${CMD[@]+"${CMD[@]}"} --listen "127.0.0.1:$PORT" >"$T/raw" 2>"$T/stderr" &
  else
    (cd "$T" && exec env PORT="$PORT" ${ENVS[@]+"${ENVS[@]}"} python3 "$PROG") >"$T/raw" 2>"$T/stderr" &
  fi
  PIDS+=($!)
  wait_port "$PORT" "$!"
}

wait_port() {
  local i
  for i in $(seq 1 100); do
    python3 -c 'import socket,sys; socket.create_connection(("127.0.0.1",int(sys.argv[1])),0.2)' "$1" 2>/dev/null && return 0
    if [ -n "${2:-}" ] && ! kill -0 "$2" 2>/dev/null; then
      echo "FAIL: the server exited before listening"; tail -c 3000 "$T/stderr"; exit 1
    fi
    sleep 0.1
  done
  echo "FAIL: nothing listening on port $1 after 10 s"; tail -c 3000 "$T/stderr"; exit 1
}

# A fixture HTTP server (eval/lib/api.py) serving a task's routes file: $API.
# Bodies it receives are logged to $T/api.log.
api() {
  local port
  port=$(free_port)
  API=http://127.0.0.1:$port
  python3 "$LIB/api.py" "$port" "$HERE/$1" "$T/api.log" 2>"$T/api.err" &
  PIDS+=($!)
  wait_port "$port" "$!"
}

# One HTTP request to the server: `req METHOD PATH [BODY]` prints "STATUS BODY".
# The body is checked as JSON when it parses as JSON (keys order and spacing ignored).
req() {
  local args=(-s --max-time 10 -o "$T/body" -w '%{http_code}' -X "$1" "$BASE$2")
  [ $# -ge 3 ] && args+=(-H 'content-type: application/json' --data-binary "$3")
  local code
  rm -f "$T/body"
  code=$(curl "${args[@]}") || code=000
  touch "$T/body"
  echo "$code $(python3 "$LIB/canon.py" <"$T/body")"
}

# `check_req METHOD PATH [BODY] -- WANT`: the request's "STATUS BODY" must equal WANT.
check_req() {
  local a=() want
  while [ "$1" != -- ]; do a+=("$1"); shift; done
  want=$(echo "$2" | python3 "$LIB/canon.py" --line)
  local got
  got=$(req "${a[@]}")
  if [ "$got" != "$want" ]; then
    echo "FAIL: ${a[0]} ${a[1]}${a[2]:+ with body ${a[2]}}"
    echo "  want: $want"
    echo "  got:  $got"
    echo "--- server stderr"; tail -c 1500 "$T/stderr"
    exit 1
  fi
}
