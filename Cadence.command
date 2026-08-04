#!/bin/zsh

set -u

APP_DIR="$(cd -- "$(dirname -- "$0")" && pwd -P)"
HOST="127.0.0.1"
BASE_PORT="${CADENCE_PORT:-4174}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
NO_OPEN="${CADENCE_NO_OPEN:-0}"
LOG_FILE="${TMPDIR:-/tmp}/cadence-server.log"
SERVER_PID=""

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  print -u2 "Cadence needs python3 to run its local server."
  exit 1
fi

cadence_url() {
  print -r -- "http://${HOST}:$1/"
}

is_cadence_running() {
  local probe_port="$1"
  local page
  page="$(curl -fsS --max-time 1 "$(cadence_url "$probe_port")" 2>/dev/null)" || return 1
  [[ "$page" == *"Cadence"* && "$page" == *"local track review"* ]]
}

port_in_use() {
  lsof -nP -iTCP:"$1" -sTCP:LISTEN -t >/dev/null 2>&1
}

PORT=""
REUSE_SERVER=0
for ((offset = 0; offset < 20; offset += 1)); do
  candidate=$((BASE_PORT + offset))
  if is_cadence_running "$candidate"; then
    PORT="$candidate"
    REUSE_SERVER=1
    break
  fi
  if ! port_in_use "$candidate"; then
    PORT="$candidate"
    break
  fi
done

if [[ -z "$PORT" ]]; then
  print -u2 "Could not find a free local port for Cadence."
  exit 1
fi

URL="$(cadence_url "$PORT")"

if (( REUSE_SERVER )); then
  print "Cadence is already running at $URL"
  [[ "$NO_OPEN" == "1" ]] || open "$URL"
  exit 0
fi

cd "$APP_DIR/web" || exit 1
"$PYTHON_BIN" -m http.server "$PORT" --bind "$HOST" >"$LOG_FILE" 2>&1 &
SERVER_PID=$!

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

READY=0
for attempt in {1..50}; do
  if curl -fsS --max-time 1 "$URL" >/dev/null 2>&1; then
    READY=1
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    break
  fi
  sleep .1
done

if (( ! READY )); then
  print -u2 "Cadence server failed to start. Log: $LOG_FILE"
  tail -n 20 "$LOG_FILE" 2>/dev/null || true
  exit 1
fi

print "Cadence is running at $URL"
print "Close this Terminal window to stop the local server."
[[ "$NO_OPEN" == "1" ]] || open "$URL"

wait "$SERVER_PID"
