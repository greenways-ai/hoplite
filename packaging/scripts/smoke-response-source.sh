#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-response-source-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
fixture_file="$(mktemp)"
body_file="$(mktemp)"
headers_file="$(mktemp)"
range_file="$(mktemp)"
slow_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$fixture_file" "$body_file" "$headers_file" "$range_file" "$slow_file"
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
}

request() {
  local method="$1"
  local url="$2"
  shift 2
  curl --silent --show-error \
    --max-time 30 \
    --request "$method" \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$@" \
    "$url"
}

header_value() {
  local name="$1"
  awk -v expected="$name" '
    BEGIN { IGNORECASE = 1 }
    index($0, expected ":") == 1 {
      sub(/^[^:]*:[[:space:]]*/, "")
      sub(/\r$/, "")
      value = $0
    }
    END { print value }
  ' "$headers_file"
}

python3 - "$fixture_file" <<'PY'
from pathlib import Path
import sys
pattern = b"greenways-hoplite-response-source\n"
size = 524288
Path(sys.argv[1]).write_bytes((pattern * ((size + len(pattern) - 1) // len(pattern)))[:size])
PY

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
  diagnose
  exit 1
fi

base="http://127.0.0.1:${port}"
ready=false
for _ in $(seq 1 60); do
  status="$(request GET "$base/hello" || true)"
  if [[ "$status" == 200 ]] && [[ "$(cat "$body_file")" == 'Hello from Hoplite' ]]; then
    ready=true
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  diagnose
  exit 1
fi

status="$(request POST "$base/blob/upload" --data-binary @"$fixture_file")"
if [[ "$status" != 201 ]] || [[ "$(cat "$body_file")" != 'Stored response source fixture' ]]; then
  echo "blob upload failed: status=$status body=$(cat "$body_file")" >&2
  diagnose
  exit 1
fi

status="$(request GET "$base/blob/source")"
if [[ "$status" != 200 ]] || ! cmp -s "$fixture_file" "$body_file"; then
  echo "complete response source failed: status=$status" >&2
  diagnose
  exit 1
fi
if [[ "$(header_value Content-Length)" != 524288 ]]; then
  echo "complete response source has wrong content length" >&2
  diagnose
  exit 1
fi

status="$(curl --silent --show-error --max-time 30 \
  --head \
  --dump-header "$headers_file" \
  --output "$body_file" \
  --write-out '%{http_code}' \
  "$base/blob/source")"
if [[ "$status" != 200 ]] || [[ "$(header_value Content-Length)" != 524288 ]] \
  || [[ -s "$body_file" ]]; then
  echo "HEAD response source failed: status=$status length=$(header_value Content-Length)" >&2
  diagnose
  exit 1
fi

status="$(request GET "$base/blob/source-range")"
dd if="$fixture_file" of="$range_file" bs=1 skip=17 count=4096 status=none
if [[ "$status" != 206 ]] || ! cmp -s "$range_file" "$body_file"; then
  echo "range response source failed: status=$status" >&2
  diagnose
  exit 1
fi
if [[ "$(header_value Content-Range)" != 'bytes 17-4112/524288' ]] \
  || [[ "$(header_value Content-Length)" != 4096 ]]; then
  echo "range response source headers are invalid" >&2
  diagnose
  exit 1
fi

status="$(curl --silent --show-error --max-time 30 \
  --limit-rate 64k \
  --dump-header "$headers_file" \
  --output "$slow_file" \
  --write-out '%{http_code}' \
  "$base/blob/source")"
if [[ "$status" != 200 ]] || ! cmp -s "$fixture_file" "$slow_file"; then
  echo "slow response source resumption failed: status=$status" >&2
  diagnose
  exit 1
fi

for route in invalid-plan stale-source; do
  status="$(request GET "$base/blob/$route")"
  if [[ "$status" != 500 ]]; then
    echo "response source rejection failed for $route: status=$status" >&2
    diagnose
    exit 1
  fi
done

status="$(request GET "$base/hello")"
if [[ "$status" != 200 ]] || [[ "$(cat "$body_file")" != 'Hello from Hoplite' ]]; then
  echo 'response source failures damaged the worker queue' >&2
  diagnose
  exit 1
fi

printf 'Validated source-backed HTTP responses through %s.\n' "$image"
