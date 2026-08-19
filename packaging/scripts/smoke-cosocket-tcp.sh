#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-cosocket-tcp}"
suffix="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
suffix="${suffix//[^A-Za-z0-9_.-]/-}"
network="hoplite-cosocket-${suffix}"
echo_container="hoplite-cosocket-echo-${suffix}"
app_container="hoplite-cosocket-app-${suffix}"
body_file="$(mktemp)"
headers_file="$(mktemp)"

cleanup() {
  docker rm -f "$app_container" "$echo_container" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file"
}
trap cleanup EXIT INT TERM

diagnose() {
  echo '--- containers ---' >&2
  docker ps -a --filter "name=${suffix}" >&2 || true
  echo '--- echo logs ---' >&2
  docker logs "$echo_container" >&2 || true
  echo '--- Hoplite logs ---' >&2
  docker logs "$app_container" >&2 || true
  echo '--- Hoplite error log ---' >&2
  docker exec "$app_container" sh -c \
    'cat /app/.hoplite/error.log 2>/dev/null || true' >&2 || true
  echo '--- generated Nginx configuration ---' >&2
  docker exec "$app_container" sh -c \
    'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
}

docker network create "$network" >/dev/null

docker run --detach \
  --name "$echo_container" \
  --network "$network" \
  python:3.12-alpine \
  python -u -c '
import socket
import threading
import time

listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("0.0.0.0", 19091))
listener.listen(64)
print("echo-ready", flush=True)

def split_send(connection, first, second):
    connection.sendall(first)
    time.sleep(0.05)
    connection.sendall(second)
    time.sleep(5)

def serve(connection):
    try:
        data = bytearray()
        while not data.endswith(b"\n"):
            chunk = connection.recv(4096)
            if not chunk:
                break
            data.extend(chunk)
        if data == b"receiveany\n":
            connection.sendall(b"part-more")
            time.sleep(5)
        elif data == b"receiveuntil\n":
            split_send(
                connection,
                b"alpha--bou",
                b"ndary--beta--boundary--omega")
        elif data == b"receiveuntil-inclusive\n":
            split_send(connection, b"alpha--bou", b"ndary--omega")
        elif data == b"receiveuntil-chunked\n":
            split_send(connection, b"abcdef--bou", b"ndary--tail")
        elif data == b"shutdown-send\n":
            while connection.recv(4096):
                pass
            connection.sendall(b"after-fin\n")
        elif data:
            connection.sendall(data)
    finally:
        connection.close()

while True:
    connection, _ = listener.accept()
    threading.Thread(target=serve, args=(connection,), daemon=True).start()
' >/dev/null

echo_ip=''
for _ in $(seq 1 50); do
  echo_ip="$(docker inspect \
    --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' \
    "$echo_container" 2>/dev/null || true)"
  if [[ -n "$echo_ip" ]] \
    && docker logs "$echo_container" 2>&1 | grep -Fq echo-ready; then
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$echo_container" \
      2>/dev/null || true)" != true ]]; then
    diagnose
    exit 1
  fi
  sleep .1
done
if [[ -z "$echo_ip" ]]; then
  echo 'TCP echo peer did not become ready.' >&2
  diagnose
  exit 1
fi

docker run --detach \
  --name "$app_container" \
  --network "$network" \
  -p 127.0.0.1::8080 \
  "$image" >/dev/null

port=''
for _ in $(seq 1 50); do
  port="$(docker port "$app_container" 8080/tcp 2>/dev/null \
    | head -n 1 | awk -F: '{print $NF}' || true)"
  if [[ -n "$port" ]]; then
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$app_container" \
      2>/dev/null || true)" != true ]]; then
    diagnose
    exit 1
  fi
  sleep .1
done
if [[ -z "$port" ]]; then
  echo 'Docker did not publish the Hoplite fixture port.' >&2
  diagnose
  exit 1
fi

base="http://127.0.0.1:${port}"

request_expect() {
  local path="$1"
  local expected="$2"
  local identity="$3"
  local status
  status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 3 \
    --header "x-cosocket-host: ${echo_ip}" \
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
    echo "$path omitted its native event-loop identity header." >&2
    diagnose
    exit 1
  fi
}

ready=false
for _ in $(seq 1 60); do
  if request_status="$(curl --silent --show-error \
      --connect-timeout 1 \
      --max-time 2 \
      --header "x-cosocket-host: ${echo_ip}" \
      --output "$body_file" \
      --write-out '%{http_code}' \
      "$base/cosocket/echo" 2>/dev/null)" \
    && [[ "$request_status" == 200 ]] \
    && [[ "$(cat "$body_file")" == ping ]]; then
    ready=true
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$app_container" \
      2>/dev/null || true)" != true ]]; then
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  echo 'Hoplite cosocket fixture did not become ready.' >&2
  diagnose
  exit 1
fi

request_expect /cosocket/echo ping tcp-event-loop
request_expect /cosocket/receiveany part tcp-receiveany
request_expect /cosocket/receiveuntil 'alpha|beta|omega' tcp-receiveuntil
request_expect \
  /cosocket/receiveuntil-inclusive \
  'alpha--boundary--|omega' \
  tcp-receiveuntil-inclusive
request_expect \
  /cosocket/receiveuntil-chunked \
  'abc|def|true|tail' \
  tcp-receiveuntil-chunked
request_expect \
  /cosocket/shutdown-send \
  after-fin \
  tcp-shutdown-send

for request in $(seq 1 5); do
  request_expect /cosocket/echo ping tcp-event-loop
done

printf 'Validated TCP receive, receiveany, receiveuntil, and send shutdown through %s.\n' \
  "$image"
