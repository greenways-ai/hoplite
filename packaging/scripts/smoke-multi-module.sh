#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-multi-module}"
container="hoplite-multi-module-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
body_file="$(mktemp)"
headers_file="$(mktemp)"
expected='alias|foundation|composition|composition|application'

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file"
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
    && [[ "$(cat "$body_file")" == "$expected" ]]; then
    ready=true
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]]; then
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  echo "Multi-module Hoplite did not become ready; last HTTP status: $last_status" >&2
  echo '--- last response headers ---' >&2
  cat "$headers_file" >&2 || true
  echo '--- last response body ---' >&2
  cat "$body_file" >&2 || true
  diagnose
  exit 1
fi

if ! tr -d '\r' < "$headers_file" \
  | grep -Fqi 'x-hoplite-fixture: multi-module'; then
  echo 'Multi-module response omitted its fixture identity header.' >&2
  diagnose
  exit 1
fi

# Repeat the request to prove the prepared handler remains usable after the
# first dispatch rather than depending on one-shot bootstrap state.
last_status="$(curl --silent --show-error \
  --max-time 15 \
  --dump-header "$headers_file" \
  --output "$body_file" \
  --write-out '%{http_code}' \
  "$base/hello")"
if [[ "$last_status" != 200 ]] \
  || [[ "$(cat "$body_file")" != "$expected" ]]; then
  echo 'Prepared multi-module handler did not survive repeated dispatch.' >&2
  diagnose
  exit 1
fi

printf 'Validated three-module HAB1 dispatch through %s on port %s.\n' \
  "$image" "$port"
