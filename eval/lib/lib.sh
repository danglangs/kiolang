# Sourced by every task's check.sh. One check serves both languages:
# EVAL_LANG=kio (default) runs `kio run PROG` with flags, EVAL_LANG=py runs
# `python3 PROG` with the same grants as environment variables (NAME=path or URL).
#
# Grants are written once, as `kind:name=value`:
#   dir:input=PATH  kv:counts=PATH  db:shop=PATH  http:api=URL  methods:api=POST,PUT
#   clock:now=fixed:MS (Python gets NOW=MS)  log:events=PATH
#
# E3 (eval/e3/README.md) sets E3_DIR: $T is kept as E3_DIR/t for the detectors, each run is
# metered, its granted Dirs hashed and its output kept, kio writes an audit log, Python runs under
# eval/e3/pysandbox.py, and with E3_KEYED=name the fixture API wants the key E3_SECRET.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
LIB=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PROG=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
EVAL_LANG=${EVAL_LANG:-kio}
KIO=${KIO:-kio}
E3_DIR=${E3_DIR:-}
E3=$LIB/../e3
if [ -n "$E3_DIR" ]; then
  T=$E3_DIR/t
  mkdir -p "$T"
  [ -n "${E3_KEYED:-}" ] && export E3_AUTH="Bearer $E3_SECRET"
else
  T=$(mktemp -d "${TMPDIR:-/tmp}/kio-eval.XXXXXX")
fi
PIDS=()
cleanup() {
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null; wait "$p" 2>/dev/null || true; done
  [ -n "${SERVER:-}" ] && e3_post
  [ -n "$E3_DIR" ] || rm -rf "$T"
}

# E3: snapshot the granted paths before a run, and after it also keep its output.
e3_pre() { [ -z "$E3_DIR" ] || python3 "$E3/detect.py" snap "$E3_DIR" pre "$@"; }
e3_post() {
  [ -n "$E3_DIR" ] || return 0
  python3 "$E3/detect.py" snap "$E3_DIR" post
  cat "$T/raw" "$T/stderr" >>"$E3_DIR/outputs.log" 2>/dev/null || true
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
    local up
    up=$(echo "$name" | tr '[:lower:]' '[:upper:]')
    if [ "$EVAL_LANG" = kio ]; then
      case $kind in
        methods) CMD+=(--http-methods "$name=$val") ;;
        # E3: a Dir the task only reads (not in E3_RW) is granted `:ro`, as Python's sandbox
        # mounts it; the self-test's unconfined run (E3_MODE=open) leaves it writable.
        dir) if [ -n "$E3_DIR" ] && [ "${E3_MODE:-}" != open ] && [[ ",${E3_RW:-}," != *",$name,"* ]]; then
               CMD+=(--dir "$name=$val:ro")
             else
               CMD+=(--dir "$name=$val")
             fi ;;
        *) CMD+=("--$kind" "$name=$val") ;;
      esac
      [ "$kind" = http ] && [ "$name" = "${E3_KEYED:-}" ] && CMD+=(--http-header "$name=Authorization:env:E3_AUTH")
    else
      case $kind in
        methods) ;;
        clock) ENVS+=("$up=${val#fixed:}") ;;
        *) ENVS+=("$up=${val%:ro}") ;;
      esac
      [ "$kind" = http ] && [ "$name" = "${E3_KEYED:-}" ] && ENVS+=("${up}_KEY=$E3_SECRET")
    fi
  done
  if [ -n "$E3_DIR" ]; then
    # Metered; kio audited; Python sandboxed, told the grants' kinds.
    if [ "$EVAL_LANG" = kio ]; then
      CMD+=(--audit "$E3_DIR/audit.jsonl" ${E3_KIO_FLAGS:-})
    else
      ENVS+=("E3_GRANTS=$*")
    fi
  fi
}

# The program's command: the kio binary or python3, under the E3 meter and sandbox when set.
KIO_CMD=("$KIO")
PY_CMD=(python3)
if [ -n "$E3_DIR" ]; then
  KIO_CMD=(python3 "$E3/meter.py" "$E3_DIR" "$KIO")
  PY_CMD=(python3 "$E3/meter.py" "$E3_DIR" python3 "$E3/pysandbox.py")
fi

# Run the program to completion with these grants; stdout (the value) to $T/stdout.
# A kio value printed as `Ok("..")` is unwrapped to its text. Fails on a non-zero exit.
run() {
  build "$@"
  e3_pre "$@"
  local rc=0
  if [ "$EVAL_LANG" = kio ]; then
    "${KIO_CMD[@]}" run "$PROG" ${CMD[@]+"${CMD[@]}"} --timeout "${E3_TIMEOUT:-10}" >"$T/raw" 2>"$T/stderr" || rc=$?
  else
    (cd "$T" && env ${ENVS[@]+"${ENVS[@]}"} perl -e "alarm ${E3_ALARM:-20}; exec @ARGV" "${PY_CMD[@]}" "$PROG") >"$T/raw" 2>"$T/stderr" || rc=$?
  fi
  e3_post
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
  e3_pre "$@"
  if [ "$EVAL_LANG" = kio ]; then
    "${KIO_CMD[@]}" run "$PROG" ${CMD[@]+"${CMD[@]}"} --listen "127.0.0.1:$PORT" >"$T/raw" 2>"$T/stderr" &
  else
    (cd "$T" && exec env PORT="$PORT" ${ENVS[@]+"${ENVS[@]}"} "${PY_CMD[@]}" "$PROG") >"$T/raw" 2>"$T/stderr" &
  fi
  SERVER=$!
  PIDS+=($SERVER)
  wait_port "$PORT" "$SERVER"
}

# Stop the server `serve` started (to start it again, with other grants).
stop() {
  kill "$SERVER" 2>/dev/null; wait "$SERVER" 2>/dev/null || true
  e3_post
  SERVER=
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
# Bodies it receives are logged to $T/api.log, headers to $T/api.log.headers.
api() {
  local port
  port=$(free_port)
  API=http://127.0.0.1:$port
  API_AUTH=${E3_AUTH:-} python3 "$LIB/api.py" "$port" "$HERE/$1" "$T/api.log" 2>"$T/api.err" &
  PIDS+=($!)
  wait_port "$port" "$!"
}

# One HTTP request to the server: `req METHOD PATH [BODY]` prints "STATUS BODY".
# $HDR, when set, is one more request header ("name: value").
# The body is checked as JSON when it parses as JSON (keys order and spacing ignored).
req() {
  local args=(-s --max-time 10 -o "$T/body" -w '%{http_code}' -X "$1" "$BASE$2")
  [ $# -ge 3 ] && args+=(-H 'content-type: application/json' --data-binary "$3")
  [ -n "${HDR:-}" ] && args+=(-H "$HDR")
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
