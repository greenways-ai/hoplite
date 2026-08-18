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

listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("0.0.0.0", 19091))
listener.listen(64)
print("echo-ready", flush=True)

def serve(connection):
    try:
        data = bytearray()
        while not data.endswith(b"\n"):
            chunk = connection.recv(4096)
            if not chunk:
                break
            data.extend(chunk)
        if data:
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
ready=false
last_status='000'
for _ in $(seq 1 60); do
  last_status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 2 \
    --header "x-cosocket-host: ${echo_ip}" \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/cosocket/echo" || true)"
  if [[ "$last_status" == 200 ]] \
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
  echo "Hoplite cosocket fixture did not become ready; status: $last_status" >&2
  echo '--- last headers ---' >&2
  cat "$headers_file" >&2 || true
  echo '--- last body ---' >&2
  cat "$body_file" >&2 || true
  diagnose
  exit 1
fi

if ! tr -d '\r' < "$headers_file" \
  | grep -Fqi 'x-hoplite-cosocket: tcp-event-loop'; then
  echo 'Cosocket response omitted its native event-loop identity header.' >&2
  diagnose
  exit 1
fi

for request in $(seq 1 5); do
  last_status="$(curl --silent --show-error \
    --max-time 3 \
    --header "x-cosocket-host: ${echo_ip}" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/cosocket/echo")"
  if [[ "$last_status" != 200 ]] \
    || [[ "$(cat "$body_file")" != ping ]]; then
    echo "Cosocket request $request failed after the initial dispatch." >&2
    diagnose
    exit 1
  fi
done

printf 'Validated request-scoped TCP cosockets through %s on port %s.\n' \
  "$image" "$port"
