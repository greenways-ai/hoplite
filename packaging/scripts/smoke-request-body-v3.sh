#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-body-v3-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
body_file="$(mktemp)"
headers_file="$(mktemp)"
large_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file" "$large_file"
}
trap cleanup EXIT INT TERM

diagnose() {
  echo '--- docker ps ---' >&2
  docker ps -a --filter "name=^/${container}$" >&2 || true
  echo '--- container state ---' >&2
  docker inspect "$container" --format '{{json .State}}' >&2 || true
  echo '--- container logs ---' >&2
  docker logs "$container" >&2 || true
  echo '--- Hoplite error log ---' >&2
  docker exec "$container" sh -c 'cat /app/.hoplite/error.log 2>/dev/null || true' >&2 || true
  echo '--- Hoplite access log ---' >&2
  docker exec "$container" sh -c 'cat /app/.hoplite/access.log 2>/dev/null || true' >&2 || true
  echo '--- generated nginx configuration ---' >&2
  docker exec "$container" sh -c 'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
  echo '--- container processes ---' >&2
  docker top "$container" -eo pid,args >&2 || true
}

request() {
  local method="$1"
  local url="$2"
  shift 2
  curl --silent --show-error \
    --max-time 15 \
    --request "$method" \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$@" \
    "$url"
}

docker run --detach --name "$container" -p 127.0.0.1::8080 "$image" >/dev/null

port=''
for _ in $(seq 1 50); do
  port="$(docker port "$container" 8080/tcp 2>/dev/null \
    | head -n 1 | awk -F: '{print $NF}' || true)"
  if [[ -n "$port" ]]; then
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]]; then
    diagnose
    exit 1
  fi
  sleep .1
done
if [[ -z "$port" ]]; then
  echo 'Docker did not publish the Hoplite port.' >&2
  diagnose
  exit 1
fi

base="http://127.0.0.1:${port}"
ready=false
last_status='000'
for _ in $(seq 1 60); do
  last_status="$(curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 1 \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/hello" || true)"
  if [[ "$last_status" == 200 ]] \
    && [[ "$(cat "$body_file")" == 'Hello from Hoplite' ]]; then
    ready=true
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]]; then
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  echo "Hoplite did not become ready; last HTTP status: $last_status" >&2
  echo '--- last response headers ---' >&2
  cat "$headers_file" >&2 || true
  echo '--- last response body ---' >&2
  cat "$body_file" >&2 || true
  diagnose
  exit 1
fi

status="$(request POST "$base/body-handle" --data-binary 'hello')"
if [[ "$status" != 200 ]] \
  || [[ "$(cat "$body_file")" != 'native-body-handle' ]]; then
  echo "Declared body conformance failed: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi

head -c 1048577 /dev/zero > "$large_file"
status="$(request POST "$base/body-handle" --data-binary "@$large_file")"
if [[ "$status" != 413 ]]; then
  echo "Oversized body was not rejected: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi

status="$(printf 'hello' | curl --http1.1 --silent --show-error \
  --max-time 15 \
  --output "$body_file" \
  --write-out '%{http_code}' \
  --request POST \
  --header 'Transfer-Encoding: chunked' \
  --header 'Content-Length:' \
  --data-binary @- \
  "$base/body-handle")"
if [[ "$status" != 411 ]]; then
  echo "Unknown-length body was not rejected: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi

status="$(request GET "$base/hello")"
if [[ "$status" != 200 ]] \
  || [[ "$(cat "$body_file")" != 'Hello from Hoplite' ]]; then
  echo 'Body requests damaged the body-free V2 route.' >&2
  diagnose
  exit 1
fi

printf 'Validated request-body V3 through %s on port %s.\n' "$image" "$port"
