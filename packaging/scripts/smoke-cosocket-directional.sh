#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-cosocket-directional}"
suffix="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
suffix="${suffix//[^A-Za-z0-9_.-]/-}"
network="hoplite-cosocket-directional-${suffix}"
peer_container="hoplite-cosocket-directional-peer-${suffix}"
app_container="hoplite-cosocket-directional-app-${suffix}"
body_file="$(mktemp)"
headers_file="$(mktemp)"

cleanup() {
  docker rm -f "$app_container" "$peer_container" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file"
}
trap cleanup EXIT INT TERM

diagnose() {
  echo '--- directional containers ---' >&2
  docker ps -a --filter "name=${suffix}" >&2 || true
  echo '--- directional peer logs ---' >&2
  docker logs "$peer_container" >&2 || true
  echo '--- directional Hoplite logs ---' >&2
  docker logs "$app_container" >&2 || true
  docker exec "$app_container" sh -c \
    'cat /app/.hoplite/error.log 2>/dev/null || true' >&2 || true
}

published_port() {
  local port=''
  for _ in $(seq 1 50); do
    port="$(docker port "$app_container" 8080/tcp 2>/dev/null \
      | head -n 1 | awk -F: '{print $NF}' || true)"
    if [[ -n "$port" ]]; then
      printf '%s' "$port"
      return 0
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$app_container" \
        2>/dev/null || true)" != true ]]; then
      return 1
    fi
    sleep .1
  done
  return 1
}

request_expect() {
  local base="$1"
  local path="$2"
  local expected="$3"
  local identity="$4"
  local status
  status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 12 \
    --header "x-cosocket-host: ${peer_ip}" \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base$path" || true)"
  if [[ "$status" != 200 ]] || [[ "$(cat "$body_file")" != "$expected" ]]; then
    echo "$path failed; status: $status" >&2
    echo '--- response headers ---' >&2
    cat "$headers_file" >&2 || true
    echo '--- response body ---' >&2
    cat "$body_file" >&2 || true
    diagnose
    exit 1
  fi
  if ! tr -d '\r' < "$headers_file" \
    | grep -Fqi "x-hoplite-cosocket: $identity"; then
    echo "$path omitted directional identity $identity." >&2
    diagnose
    exit 1
  fi
}

docker network create "$network" >/dev/null

docker run --detach \
  --name "$peer_container" \
  --network "$network" \
  python:3.12-alpine \
  python -u -c '
import socket
import threading
import time

def listener(port):
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", port))
    server.listen(64)
    return server

def read_line(connection):
    data = bytearray()
    while not data.endswith(b"\n"):
        chunk = connection.recv(65536)
        if not chunk:
            break
        data.extend(chunk)
    return bytes(data)

def serve_standard(connection):
    try:
        data = read_line(connection)
        if data == b"directional-shutdown\n":
            while connection.recv(65536):
                pass
            connection.sendall(b"after-directional-fin\n")
        elif data:
            connection.sendall(data)
    finally:
        connection.close()

def serve_slow_write(connection):
    try:
        time.sleep(0.6)
        data = read_line(connection)
        if data:
            connection.sendall(b"write-drained\n")
    finally:
        connection.close()

def serve_blackhole(connection):
    try:
        time.sleep(30)
    finally:
        connection.close()

def accept_forever(server, handler):
    while True:
        connection, _ = server.accept()
        threading.Thread(
            target=handler,
            args=(connection,),
            daemon=True,
        ).start()

standard = listener(19091)
slow_write = listener(19092)
blackhole = listener(19093)
threading.Thread(
    target=accept_forever,
    args=(standard, serve_standard),
    daemon=True,
).start()
threading.Thread(
    target=accept_forever,
    args=(slow_write, serve_slow_write),
    daemon=True,
).start()
print("directional-peer-ready", flush=True)
accept_forever(blackhole, serve_blackhole)
' >/dev/null

peer_ip=''
for _ in $(seq 1 50); do
  peer_ip="$(docker inspect \
    --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' \
    "$peer_container" 2>/dev/null || true)"
  if [[ -n "$peer_ip" ]] \
    && docker logs "$peer_container" 2>&1 \
      | grep -Fq directional-peer-ready; then
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$peer_container" \
      2>/dev/null || true)" != true ]]; then
    diagnose
    exit 1
  fi
  sleep .1
done
if [[ -z "$peer_ip" ]]; then
  echo 'Directional TCP peers did not become ready.' >&2
  diagnose
  exit 1
fi

docker run --detach \
  --name "$app_container" \
  --network "$network" \
  -p 127.0.0.1::8080 \
  "$image" >/dev/null

port="$(published_port || true)"
if [[ -z "$port" ]]; then
  echo 'Docker did not publish the directional fixture port.' >&2
  diagnose
  exit 1
fi
base="http://127.0.0.1:${port}"

ready=0
for _ in $(seq 1 60); do
  status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 3 \
    --header "x-cosocket-host: ${peer_ip}" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/directional/concurrent" 2>/dev/null || true)"
  if [[ "$status" == 200 ]] && [[ "$(cat "$body_file")" == concurrent ]]; then
    ready=1
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$app_container" \
      2>/dev/null || true)" != true ]]; then
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  echo 'Directional Hoplite fixture did not become ready.' >&2
  diagnose
  exit 1
fi

request_expect \
  "$base" \
  /directional/concurrent \
  concurrent \
  tcp-directional-concurrent
request_expect \
  "$base" \
  /directional/busy-read \
  'socket busy reading|busy-read' \
  tcp-directional-busy-read
request_expect \
  "$base" \
  /directional/busy-write \
  'connection in dubious state|socket busy writing|write-drained' \
  tcp-directional-busy-write
request_expect \
  "$base" \
  /directional/shutdown \
  after-directional-fin \
  tcp-directional-shutdown
request_expect \
  "$base" \
  /directional/close \
  'closed|closed|1' \
  tcp-directional-close

curl --silent --show-error \
  --connect-timeout 1 \
  --max-time 0.2 \
  --header "x-cosocket-host: ${peer_ip}" \
  "$base/directional/abandon" >/dev/null 2>&1 || true
sleep .4
request_expect \
  "$base" \
  /directional/concurrent \
  concurrent \
  tcp-directional-concurrent

curl --silent --show-error \
  --connect-timeout 1 \
  --max-time 10 \
  --header "x-cosocket-host: ${peer_ip}" \
  "$base/directional/abandon" >/dev/null 2>&1 &
abandon_pid=$!
sleep .2
if ! docker stop --time 3 "$app_container" >/dev/null; then
  echo 'Worker shutdown did not drain directional cosocket state.' >&2
  diagnose
  exit 1
fi
wait "$abandon_pid" >/dev/null 2>&1 || true

printf 'Validated one concurrent TCP cosocket read and write, same-direction busy results, pending-pool rejection, send shutdown with a live read, explicit close, client-abort cleanup, and worker-exit draining through %s.\n' \
  "$image"
