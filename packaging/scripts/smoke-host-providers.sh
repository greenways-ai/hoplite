#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-host-providers-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
body_file="$(mktemp)"
headers_file="$(mktemp)"

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
  echo '--- generated nginx configuration ---' >&2
  docker exec "$container" sh -c 'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
  echo '--- container processes ---' >&2
  docker top "$container" -eo pid,args >&2 || true
}

request() {
  local url="$1"
  curl --silent --show-error \
    --max-time 15 \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
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
for _ in $(seq 1 60); do
  status="$(request "$base/hello" || true)"
  if [[ "$status" == 200 ]] \
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
  echo 'Hoplite did not become ready for host-provider conformance.' >&2
  diagnose
  exit 1
fi

status="$(request "$base/delay")"
if [[ "$status" != 200 ]] \
  || [[ "$(cat "$body_file")" != 'Hara coroutine resumed from the Nginx event loop' ]]; then
  echo "nginx/sleep provider conformance failed: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi

for route in unknown-service unknown-operation invalid-arguments; do
  status="$(request "$base/host/$route")"
  if [[ "$status" != 500 ]]; then
    echo "Host provider rejection failed for $route: status=$status body=$(cat "$body_file")" >&2
    diagnose
    exit 1
  fi
done

status="$(request "$base/hello")"
if [[ "$status" != 200 ]] \
  || [[ "$(cat "$body_file")" != 'Hello from Hoplite' ]]; then
  echo 'Rejected host calls damaged the worker or request queue.' >&2
  diagnose
  exit 1
fi

printf 'Validated registered native host providers through %s.\n' "$image"
