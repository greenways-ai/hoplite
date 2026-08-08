#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-response-source-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
body_file="$(mktemp)"
headers_file="$(mktemp)"
fixture_file="$(mktemp)"
slow_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file" "$fixture_file" "$slow_file"
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
  awk -v wanted="$name" '
    BEGIN { IGNORECASE = 1 }
    {
      line = $0
      sub(/\r$/, "", line)
      split(line, parts, ":")
      if (tolower(parts[1]) == tolower(wanted)) {
        sub(/^[^:]*:[[:space:]]*/, "", line)
        print line
        exit
      }
    }
  ' "$headers_file"
}

python3 - "$fixture_file" <<'PY'
from pathlib import Path
import sys

Path(sys.argv[1]).write_bytes(b"0123456789abcdef" * 65536)
PY

expected_size=1048576
expected_digest=aca1cd027e979588d14b877b7b0cb8585ad9fec599eb45801992ee5382b3760f
if [[ "$(wc -c < "$fixture_file" | tr -d ' ')" != "$expected_size" ]] \
  || [[ "$(sha256sum "$fixture_file" | awk '{print $1}')" != "$expected_digest" ]]; then
  echo 'Response-source fixture generation is not deterministic.' >&2
  exit 1
fi

docker run --detach \
  --name "$container" \
  -e HOPLITE_WORKERS=1 \
  -p 127.0.0.1::8080 \
  "$image" >/dev/null

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
  status="$(request GET "$base/hello" || true)"
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
  echo 'Hoplite did not become ready for response-source conformance.' >&2
  diagnose
  exit 1
fi

status="$(request POST "$base/response-source" --data-binary "@$fixture_file")"
if [[ "$status" != 200 ]] \
  || ! cmp -s "$fixture_file" "$body_file" \
  || [[ "$(header_value content-length)" != "$expected_size" ]] \
  || [[ "$(header_value x-hoplite-source)" != true ]]; then
  echo "Response-source upload/stream failed: status=$status size=$(wc -c < "$body_file")" >&2
  diagnose
  exit 1
fi

status="$(curl --silent --show-error \
  --max-time 15 \
  --head \
  --dump-header "$headers_file" \
  --output /dev/null \
  --write-out '%{http_code}' \
  "$base/response-source")"
if [[ "$status" != 200 ]] \
  || [[ "$(header_value content-length)" != "$expected_size" ]] \
  || [[ "$(header_value x-hoplite-source)" != true ]]; then
  echo "HEAD response-source conformance failed: status=$status" >&2
  diagnose
  exit 1
fi

status="$(curl --silent --show-error \
  --max-time 20 \
  --limit-rate 128k \
  --dump-header "$headers_file" \
  --output "$slow_file" \
  --write-out '%{http_code}' \
  "$base/response-source")"
if [[ "$status" != 200 ]] \
  || ! cmp -s "$fixture_file" "$slow_file"; then
  echo "Backpressured response-source resume failed: status=$status size=$(wc -c < "$slow_file")" >&2
  diagnose
  exit 1
fi

for route in invalid stale; do
  status="$(request GET "$base/response-source/$route")"
  if [[ "$status" != 500 ]]; then
    echo "Response-source rejection failed for $route: status=$status" >&2
    diagnose
    exit 1
  fi
done

status="$(request GET "$base/hello")"
if [[ "$status" != 200 ]] \
  || [[ "$(cat "$body_file")" != 'Hello from Hoplite' ]]; then
  echo 'Rejected response sources damaged the worker or request queue.' >&2
  diagnose
  exit 1
fi

printf 'Validated bounded hara.response-source/1 streaming through %s on port %s.\n' \
  "$image" "$port"
