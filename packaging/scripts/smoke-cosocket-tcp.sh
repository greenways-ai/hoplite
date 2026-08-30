#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-cosocket-tcp}"
suffix="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
suffix="${suffix//[^A-Za-z0-9_.-]/-}"
network="hoplite-cosocket-${suffix}"
socket_volume="hoplite-cosocket-socket-${suffix}"
echo_container="hoplite-cosocket-echo-${suffix}"
source_container="hoplite-cosocket-source-${suffix}"
app_container="hoplite-cosocket-app-${suffix}"
noresolver_container="hoplite-cosocket-noresolver-${suffix}"
cancel_container="hoplite-cosocket-cancel-${suffix}"
body_file="$(mktemp)"
headers_file="$(mktemp)"
resolver_config="$(mktemp)"
blackhole_config="$(mktemp)"
backlog_dir="$(mktemp -d)"

cleanup() {
  docker rm -f \
    "$app_container" \
    "$noresolver_container" \
    "$cancel_container" \
    "$source_container" \
    "$echo_container" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  docker volume rm "$socket_volume" >/dev/null 2>&1 || true
  rm -f \
    "$body_file" \
    "$headers_file" \
    "$resolver_config" \
    "$blackhole_config"
  rm -rf "$backlog_dir"
}
trap cleanup EXIT INT TERM

diagnose_container() {
  local container="$1"
  if docker inspect "$container" >/dev/null 2>&1; then
    echo "--- ${container} logs ---" >&2
    docker logs "$container" >&2 || true
    echo "--- ${container} Hoplite error log ---" >&2
    docker exec "$container" sh -c \
      'cat /app/.hoplite/error.log 2>/dev/null || true' >&2 || true
    echo "--- ${container} Nginx configuration ---" >&2
    docker exec "$container" sh -c \
      'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
  fi
}

diagnose() {
  echo '--- containers ---' >&2
  docker ps -a --filter "name=${suffix}" >&2 || true
  echo '--- echo logs ---' >&2
  docker logs "$echo_container" >&2 || true
  diagnose_container "$app_container"
  diagnose_container "$noresolver_container"
  diagnose_container "$cancel_container"
}

published_port() {
  local container="$1"
  local port=''
  for _ in $(seq 1 50); do
    port="$(docker port "$container" 8080/tcp 2>/dev/null \
      | head -n 1 | awk -F: '{print $NF}' || true)"
    if [[ -n "$port" ]]; then
      printf '%s' "$port"
      return 0
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" \
        2>/dev/null || true)" != true ]]; then
      return 1
    fi
    sleep .1
  done
  return 1
}

wait_ready() {
  local container="$1"
  local base="$2"
  local request_status
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
      return 0
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" \
        2>/dev/null || true)" != true ]]; then
      return 1
    fi
    sleep 1
  done
  return 1
}

request_expect_at() {
  local base="$1"
  local path="$2"
  local expected="$3"
  local identity="$4"
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

request_keepalive_at() {
  local base="$1"
  local host="$2"
  local pool="$3"
  local idle="$4"
  local status
  status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 3 \
    --header "x-cosocket-host: ${host}" \
    --header "x-cosocket-pool: ${pool}" \
    --header "x-cosocket-idle: ${idle}" \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/cosocket/keepalive" || true)"
  if [[ "$status" != 200 ]]; then
    echo "/cosocket/keepalive failed; status: $status" >&2
    cat "$headers_file" >&2 || true
    cat "$body_file" >&2 || true
    diagnose
    exit 1
  fi
  if ! tr -d '\r' < "$headers_file" \
    | grep -Fqi 'x-hoplite-cosocket: tcp-keepalive-pool'; then
    echo '/cosocket/keepalive omitted its native pool identity header.' >&2
    diagnose
    exit 1
  fi
  cat "$body_file"
}

request_keepalive_unix_at() {
  local base="$1"
  local status
  status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 3 \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/cosocket/keepalive-unix" || true)"
  if [[ "$status" != 200 ]]; then
    echo "/cosocket/keepalive-unix failed; status: $status" >&2
    cat "$headers_file" >&2 || true
    cat "$body_file" >&2 || true
    diagnose
    exit 1
  fi
  if ! tr -d '\r' < "$headers_file" \
    | grep -Fqi 'x-hoplite-cosocket: unix-keepalive-pool'; then
    echo '/cosocket/keepalive-unix omitted its native pool identity header.' >&2
    diagnose
    exit 1
  fi
  cat "$body_file"
}

request_backlog_at() {
  local base="$1"
  local pool="$2"
  local backlog_mode="$3"
  local timeout_mode="$4"
  local body headers status
  body="$(mktemp "${backlog_dir}/body.XXXXXX")"
  headers="$(mktemp "${backlog_dir}/headers.XXXXXX")"
  status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 6 \
    --header "x-cosocket-host: ${echo_ip}" \
    --header "x-cosocket-pool: ${pool}" \
    --header "x-cosocket-backlog: ${backlog_mode}" \
    --header "x-cosocket-timeout: ${timeout_mode}" \
    --dump-header "$headers" \
    --output "$body" \
    --write-out '%{http_code}' \
    "$base/cosocket/backlog" || true)"
  if [[ "$status" != 200 ]]; then
    echo "/cosocket/backlog failed; status: $status" >&2
    cat "$headers" >&2 || true
    cat "$body" >&2 || true
    diagnose
    return 1
  fi
  if ! tr -d '\r' < "$headers" \
    | grep -Fqi 'x-hoplite-cosocket: tcp-backlog'; then
    echo '/cosocket/backlog omitted its native backlog identity header.' >&2
    diagnose
    return 1
  fi
  cat "$body"
}

assert_keepalive_pair() {
  local first="$1"
  local second="$2"
  local label="$3"
  local first_connection first_request first_reused
  local second_connection second_request second_reused

  IFS=':|' read -r first_connection first_request first_reused <<<"$first"
  IFS=':|' read -r second_connection second_request second_reused <<<"$second"
  if [[ -z "$first_connection" ]] \
    || [[ "$first_connection" != "$second_connection" ]] \
    || [[ "$first_request" != 1 ]] \
    || [[ "$second_request" != 2 ]] \
    || [[ "$first_reused" != 0 ]] \
    || [[ "$second_reused" != 1 ]]; then
    echo "$label did not reuse one persistent connection: $first / $second" >&2
    diagnose
    exit 1
  fi
}

assert_backlog_fifo() {
  local first="$1"
  local second="$2"
  local third="$3"
  local first_connection first_request first_reused
  local second_connection second_request second_reused
  local third_connection third_request third_reused

  IFS=':|' read -r first_connection first_request first_reused <<<"$first"
  IFS=':|' read -r second_connection second_request second_reused <<<"$second"
  IFS=':|' read -r third_connection third_request third_reused <<<"$third"
  if [[ -z "$first_connection" ]] \
    || [[ "$first_connection" != "$second_connection" ]] \
    || [[ "$first_connection" != "$third_connection" ]] \
    || [[ "$first_request" != 1 ]] \
    || [[ "$second_request" != 2 ]] \
    || [[ "$third_request" != 3 ]] \
    || [[ "$first_reused" != 0 ]] \
    || [[ "$second_reused" != 1 ]] \
    || [[ "$third_reused" != 2 ]]; then
    echo "bounded FIFO backlog did not serialize one pool slot: $first / $second / $third" >&2
    diagnose
    exit 1
  fi
}

connection_id() {
  local result="$1"
  local identifier _request _reused
  IFS=':|' read -r identifier _request _reused <<<"$result"
  printf '%s' "$identifier"
}

reuse_count() {
  local result="$1"
  local _identifier _request reused
  IFS=':|' read -r _identifier _request reused <<<"$result"
  printf '%s' "$reused"
}

docker network create "$network" >/dev/null
docker volume create "$socket_volume" >/dev/null

docker run --detach \
  --name "$echo_container" \
  --network "$network" \
  --network-alias cosocket-echo.test \
  --mount "type=volume,source=${socket_volume},target=/cosocket" \
  python:3.12-alpine \
  python -u -c '
import os
import socket
import threading
import time

listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("0.0.0.0", 19091))
listener.listen(64)

unix_path = "/cosocket/echo.sock"
try:
    os.unlink(unix_path)
except FileNotFoundError:
    pass
unix_listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
unix_listener.bind(unix_path)
os.chmod(unix_path, 0o777)
unix_listener.listen(64)
print("echo-ready", flush=True)

sequence_lock = threading.Lock()
connection_sequence = 0

def next_connection_id():
    global connection_sequence
    with sequence_lock:
        connection_sequence += 1
        return connection_sequence

def split_send(connection, first, second):
    connection.sendall(first)
    time.sleep(0.05)
    connection.sendall(second)
    time.sleep(5)

def read_line(connection):
    data = bytearray()
    while not data.endswith(b"\n"):
        chunk = connection.recv(4096)
        if not chunk:
            break
        data.extend(chunk)
    return bytes(data)

def serve(connection):
    identifier = next_connection_id()
    request_count = 0
    try:
        while True:
            data = read_line(connection)
            if not data:
                break
            request_count += 1
            if data == b"keepalive\n":
                connection.sendall(
                    f"{identifier}:{request_count}\n".encode("ascii"))
                continue
            if data == b"backlog\n":
                time.sleep(0.6)
                connection.sendall(
                    f"{identifier}:{request_count}\n".encode("ascii"))
                continue
            if data == b"keepalive-dirty\n":
                connection.sendall(b"dirty\nextra")
                time.sleep(5)
                break
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
            else:
                connection.sendall(data)
            break
    finally:
        connection.close()

def accept_forever(server):
    while True:
        connection, _ = server.accept()
        threading.Thread(target=serve, args=(connection,), daemon=True).start()

threading.Thread(
    target=accept_forever,
    args=(unix_listener,),
    daemon=True).start()
accept_forever(listener)
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
  echo 'TCP and Unix echo peer did not become ready.' >&2
  diagnose
  exit 1
fi

docker create --name "$source_container" "$image" >/dev/null
docker cp \
  "$source_container:/app/.hoplite/conf/nginx.conf" \
  "$resolver_config"
docker rm "$source_container" >/dev/null
python3 - "$resolver_config" "$blackhole_config" <<'PY'
from pathlib import Path
import re
import sys

source = Path(sys.argv[1])
blackhole = Path(sys.argv[2])
text = source.read_text()
text, replacements = re.subn(
    r"worker_processes\s+(?:auto|[0-9]+);",
    "worker_processes 1;",
    text,
    count=1,
)
if replacements != 1:
    raise SystemExit("generated Nginx configuration has no worker_processes directive")
needle = "http {\n"
if needle not in text:
    raise SystemExit("generated Nginx configuration has no http block")
configured = text.replace(
    needle,
    needle
    + "    resolver 127.0.0.11 ipv6=off valid=1s;\n"
    + "    resolver_timeout 1s;\n",
    1,
)
source.write_text(configured)
blackhole.write_text(
    configured.replace(
        "resolver 127.0.0.11 ipv6=off valid=1s;",
        "resolver 192.0.2.1 ipv6=off valid=1s;",
        1,
    ).replace("resolver_timeout 1s;", "resolver_timeout 5s;", 1)
)
PY
# docker cp preserves the source config mode, while the sibling mktemp file
# remains 0600. Both bind mounts must be readable by the non-root image user.
chmod 0644 "$resolver_config" "$blackhole_config"

docker run --detach \
  --name "$app_container" \
  --network "$network" \
  --env HOPLITE_WORKERS=1 \
  --mount "type=volume,source=${socket_volume},target=/cosocket" \
  --mount "type=bind,source=${resolver_config},target=/app/.hoplite/conf/nginx.conf,readonly" \
  -p 127.0.0.1::8080 \
  "$image" >/dev/null

port="$(published_port "$app_container" || true)"
if [[ -z "$port" ]]; then
  echo 'Docker did not publish the Hoplite fixture port.' >&2
  diagnose
  exit 1
fi
base="http://127.0.0.1:${port}"
if ! wait_ready "$app_container" "$base"; then
  echo 'Hoplite cosocket fixture did not become ready.' >&2
  diagnose
  exit 1
fi

request_expect_at "$base" /cosocket/echo ping tcp-event-loop
request_expect_at "$base" /cosocket/unix unix tcp-unix-domain
request_expect_at "$base" /cosocket/dns dns tcp-nginx-resolver
request_expect_at \
  "$base" \
  /cosocket/dns-result \
  'host not found' \
  tcp-nginx-resolver-error
request_expect_at "$base" /cosocket/receiveany part tcp-receiveany
request_expect_at "$base" /cosocket/receiveuntil 'alpha|beta|omega' tcp-receiveuntil
request_expect_at \
  "$base" \
  /cosocket/receiveuntil-inclusive \
  'alpha--boundary--|omega' \
  tcp-receiveuntil-inclusive
request_expect_at \
  "$base" \
  /cosocket/receiveuntil-chunked \
  'abc|def|true|tail' \
  tcp-receiveuntil-chunked
request_expect_at "$base" /cosocket/setoption setoption tcp-setoption
request_expect_at \
  "$base" \
  /cosocket/shutdown-send \
  after-fin \
  tcp-shutdown-send

numeric_first="$(request_keepalive_at \
  "$base" "$echo_ip" numeric-reuse long)"
numeric_second="$(request_keepalive_at \
  "$base" "$echo_ip" numeric-reuse long)"
assert_keepalive_pair "$numeric_first" "$numeric_second" \
  'numeric TCP keepalive'

hostname_first="$(request_keepalive_at \
  "$base" cosocket-echo.test hostname-reuse long)"
hostname_second="$(request_keepalive_at \
  "$base" cosocket-echo.test hostname-reuse long)"
assert_keepalive_pair "$hostname_first" "$hostname_second" \
  'resolver-backed hostname keepalive'

unix_first="$(request_keepalive_unix_at "$base")"
unix_second="$(request_keepalive_unix_at "$base")"
assert_keepalive_pair "$unix_first" "$unix_second" \
  'Unix-domain keepalive'

transport_numeric="$(request_keepalive_at \
  "$base" "$echo_ip" transport-isolation long)"
transport_hostname="$(request_keepalive_at \
  "$base" cosocket-echo.test transport-isolation long)"
if [[ "$(connection_id "$transport_numeric")" \
      == "$(connection_id "$transport_hostname")" ]] \
  || [[ "$(reuse_count "$transport_numeric")" != 0 ]] \
  || [[ "$(reuse_count "$transport_hostname")" != 0 ]]; then
  echo "numeric and hostname pool identities collided: $transport_numeric / $transport_hostname" >&2
  diagnose
  exit 1
fi

pool_alpha="$(request_keepalive_at \
  "$base" "$echo_ip" explicit-alpha long)"
pool_beta="$(request_keepalive_at \
  "$base" "$echo_ip" explicit-beta long)"
if [[ "$(connection_id "$pool_alpha")" \
      == "$(connection_id "$pool_beta")" ]] \
  || [[ "$(reuse_count "$pool_alpha")" != 0 ]] \
  || [[ "$(reuse_count "$pool_beta")" != 0 ]]; then
  echo "explicit pool names collided: $pool_alpha / $pool_beta" >&2
  diagnose
  exit 1
fi

expiry_first="$(request_keepalive_at \
  "$base" "$echo_ip" expiry short)"
sleep .3
expiry_second="$(request_keepalive_at \
  "$base" "$echo_ip" expiry short)"
if [[ "$(connection_id "$expiry_first")" \
      == "$(connection_id "$expiry_second")" ]] \
  || [[ "$(reuse_count "$expiry_first")" != 0 ]] \
  || [[ "$(reuse_count "$expiry_second")" != 0 ]]; then
  echo "expired idle connection was reused: $expiry_first / $expiry_second" >&2
  diagnose
  exit 1
fi

request_expect_at \
  "$base" \
  /cosocket/keepalive-dirty \
  'connection in dubious state' \
  tcp-keepalive-dirty

fifo_first_file="${backlog_dir}/fifo-first"
fifo_second_file="${backlog_dir}/fifo-second"
fifo_third_file="${backlog_dir}/fifo-third"
request_backlog_at "$base" backlog-fifo two normal >"$fifo_first_file" &
fifo_first_pid=$!
sleep .10
request_backlog_at "$base" backlog-fifo two normal >"$fifo_second_file" &
fifo_second_pid=$!
sleep .10
request_backlog_at "$base" backlog-fifo two normal >"$fifo_third_file" &
fifo_third_pid=$!
sleep .10
fifo_overflow="$(request_backlog_at "$base" backlog-fifo two normal)"
if [[ "$fifo_overflow" != 'pool backlog overflow' ]]; then
  echo "bounded backlog did not reject overflow: $fifo_overflow" >&2
  diagnose
  exit 1
fi
wait "$fifo_first_pid"
wait "$fifo_second_pid"
wait "$fifo_third_pid"
fifo_first="$(cat "$fifo_first_file")"
fifo_second="$(cat "$fifo_second_file")"
fifo_third="$(cat "$fifo_third_file")"
assert_backlog_fifo "$fifo_first" "$fifo_second" "$fifo_third"

zero_holder_file="${backlog_dir}/zero-holder"
request_backlog_at "$base" backlog-zero zero normal >"$zero_holder_file" &
zero_holder_pid=$!
sleep .10
zero_full="$(request_backlog_at "$base" backlog-zero zero normal)"
if [[ "$zero_full" != 'pool capacity unavailable' ]]; then
  echo "zero backlog did not reject a full pool: $zero_full" >&2
  diagnose
  exit 1
fi
wait "$zero_holder_pid"

timeout_holder_file="${backlog_dir}/timeout-holder"
request_backlog_at "$base" backlog-timeout one normal >"$timeout_holder_file" &
timeout_holder_pid=$!
sleep .10
timeout_result="$(request_backlog_at "$base" backlog-timeout one short)"
if [[ "$timeout_result" != 'pool wait timeout' ]]; then
  echo "queued connect did not consume its connect timeout: $timeout_result" >&2
  diagnose
  exit 1
fi
wait "$timeout_holder_pid"
timeout_holder="$(cat "$timeout_holder_file")"
timeout_follow="$(request_backlog_at "$base" backlog-timeout one normal)"
assert_keepalive_pair "$timeout_holder" "$timeout_follow" \
  'backlog timeout removal'

cancel_holder_file="${backlog_dir}/cancel-holder"
request_backlog_at "$base" backlog-cancel one normal >"$cancel_holder_file" &
cancel_holder_pid=$!
sleep .10
curl --silent --show-error \
  --connect-timeout 1 \
  --max-time 0.12 \
  --header "x-cosocket-host: ${echo_ip}" \
  --header 'x-cosocket-pool: backlog-cancel' \
  --header 'x-cosocket-backlog: one' \
  --header 'x-cosocket-timeout: normal' \
  "$base/cosocket/backlog" >/dev/null 2>&1 || true
sleep .10
cancel_follow="$(request_backlog_at "$base" backlog-cancel one normal)"
wait "$cancel_holder_pid"
cancel_holder="$(cat "$cancel_holder_file")"
assert_keepalive_pair "$cancel_holder" "$cancel_follow" \
  'backlog cancellation removal'

for request in $(seq 1 5); do
  request_expect_at "$base" /cosocket/echo ping tcp-event-loop
done

docker run --detach \
  --name "$noresolver_container" \
  --network "$network" \
  --env HOPLITE_WORKERS=1 \
  --mount "type=volume,source=${socket_volume},target=/cosocket" \
  -p 127.0.0.1::8080 \
  "$image" >/dev/null
noresolver_port="$(published_port "$noresolver_container" || true)"
noresolver_base="http://127.0.0.1:${noresolver_port}"
if [[ -z "$noresolver_port" ]] \
  || ! wait_ready "$noresolver_container" "$noresolver_base"; then
  echo 'Hoplite no-resolver fixture did not become ready.' >&2
  diagnose
  exit 1
fi
request_expect_at \
  "$noresolver_base" \
  /cosocket/dns-result \
  'resolver not configured' \
  tcp-nginx-resolver-error

docker run --detach \
  --name "$cancel_container" \
  --network "$network" \
  --env HOPLITE_WORKERS=1 \
  --mount "type=volume,source=${socket_volume},target=/cosocket" \
  --mount "type=bind,source=${blackhole_config},target=/app/.hoplite/conf/nginx.conf,readonly" \
  -p 127.0.0.1::8080 \
  "$image" >/dev/null
cancel_port="$(published_port "$cancel_container" || true)"
cancel_base="http://127.0.0.1:${cancel_port}"
if [[ -z "$cancel_port" ]] \
  || ! wait_ready "$cancel_container" "$cancel_base"; then
  echo 'Hoplite resolver-cancellation fixture did not become ready.' >&2
  diagnose
  exit 1
fi
curl --silent --show-error \
  --connect-timeout 1 \
  --max-time 0.2 \
  --header 'x-cosocket-name: cosocket-echo.test' \
  "$cancel_base/cosocket/dns-result" >/dev/null 2>&1 || true
sleep .5
request_expect_at "$cancel_base" /cosocket/echo ping tcp-event-loop
if ! docker stop --time 3 "$cancel_container" >/dev/null; then
  echo 'Worker shutdown did not cancel the outstanding resolver context.' >&2
  diagnose
  exit 1
fi

curl --silent --show-error \
  --connect-timeout 1 \
  --max-time 5 \
  --header "x-cosocket-host: ${echo_ip}" \
  --header 'x-cosocket-pool: backlog-worker-exit' \
  --header 'x-cosocket-backlog: one' \
  --header 'x-cosocket-timeout: normal' \
  "$base/cosocket/backlog" >/dev/null 2>&1 &
shutdown_holder_pid=$!
sleep .10
curl --silent --show-error \
  --connect-timeout 1 \
  --max-time 5 \
  --header "x-cosocket-host: ${echo_ip}" \
  --header 'x-cosocket-pool: backlog-worker-exit' \
  --header 'x-cosocket-backlog: one' \
  --header 'x-cosocket-timeout: normal' \
  "$base/cosocket/backlog" >/dev/null 2>&1 &
shutdown_waiter_pid=$!
sleep .10
if ! docker stop --time 3 "$app_container" >/dev/null; then
  echo 'Worker shutdown did not drain the cosocket backlog.' >&2
  diagnose
  exit 1
fi
wait "$shutdown_holder_pid" >/dev/null 2>&1 || true
wait "$shutdown_waiter_pid" >/dev/null 2>&1 || true

printf 'Validated numeric, Unix-domain, and Nginx-resolved TCP cosockets; worker-local keepalive reuse and bounded FIFO backlog admission, overflow, zero-backlog capacity, timeout, cancellation, and worker-shutdown draining; identity isolation, expiry, dirty rejection, bounded DNS failure, resolver cancellation, receive patterns, setoption, and send shutdown through %s.\n' \
  "$image"
