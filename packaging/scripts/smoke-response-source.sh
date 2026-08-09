#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-response-source-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
volume="${container}-data"
body_file="$(mktemp)"
headers_file="$(mktemp)"
fixture_file="$(mktemp)"
slow_file="$(mktemp)"
expected_range_file="$(mktemp)"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  docker volume rm -f "$volume" >/dev/null 2>&1 || true
  rm -f \
    "$body_file" \
    "$headers_file" \
    "$fixture_file" \
    "$slow_file" \
    "$expected_range_file"
}
trap cleanup EXIT INT TERM

diagnose() {
  echo '--- response headers ---' >&2
  sed -n '1,80p' "$headers_file" >&2 || true
  echo '--- response body prefix ---' >&2
  head -c 512 "$body_file" | od -An -tx1c >&2 || true
  echo '--- docker ps ---' >&2
  docker ps -a --filter "name=^/${container}$" >&2 || true
  echo '--- container state ---' >&2
  docker inspect "$container" --format '{{json .State}}' >&2 || true
  echo '--- container environment ---' >&2
  docker exec "$container" sh -c \
    'env | grep -E "^(HOPLITE_HARA_BLOB|HOPLITE_HARA_STORE|HOPLITE_SERVER_CACHE|HOPLITE_WORKERS)=" | sort' \
    >&2 || true
  echo '--- container logs ---' >&2
  docker logs "$container" >&2 || true
  echo '--- Hoplite and Nginx error logs ---' >&2
  docker exec "$container" sh -c '
    for path in /app/.hoplite/error.log /app/.hoplite/logs/error.log /app/.hoplite/logs/*.log; do
      if [ -f "$path" ]; then
        echo "--- $path ---"
        tail -n 200 "$path"
      fi
    done
  ' >&2 || true
  echo '--- generated nginx configuration ---' >&2
  docker exec "$container" sh -c 'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
  echo '--- persistent blob layout ---' >&2
  docker exec "$container" sh -c \
    'find /var/lib/hoplite/blob -maxdepth 6 -printf "%M %u:%g %s %p\n" 2>/dev/null | sort' \
    >&2 || true
  echo '--- persistent data volume ---' >&2
  docker volume inspect "$volume" >&2 || true
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

start_container() {
  docker run --detach \
    --name "$container" \
    -e HOPLITE_WORKERS=1 \
    --mount "type=volume,src=${volume},dst=/var/lib/hoplite" \
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
}

assert_persisted_object() {
  local label="$1"
  local actual_size
  local actual_digest
  if ! docker exec "$container" test -f "$object_blob" \
    || ! docker exec "$container" test -f "$object_meta"; then
    echo "$label failed: committed object files are absent" >&2
    diagnose
    exit 1
  fi
  actual_size="$(docker exec "$container" sh -c 'wc -c < "$1"' sh "$object_blob" \
    | tr -d ' ')"
  actual_digest="$(docker exec "$container" sha256sum "$object_blob" | awk '{print $1}')"
  if [[ "$actual_size" != "$expected_size" ]] \
    || [[ "$actual_digest" != "$expected_digest" ]]; then
    echo "$label failed: size=$actual_size digest=$actual_digest" >&2
    diagnose
    exit 1
  fi
}

assert_full_response() {
  local label="$1"
  local status
  status="$(request GET "$base/response-source")"
  if [[ "$status" != 200 ]] \
    || ! cmp -s "$fixture_file" "$body_file" \
    || [[ "$(header_value content-length)" != "$expected_size" ]] \
    || [[ "$(header_value x-hoplite-source)" != true ]]; then
    echo "$label failed: status=$status size=$(wc -c < "$body_file")" >&2
    diagnose
    exit 1
  fi
}

assert_range_response() {
  local label="$1"
  local status
  status="$(request GET "$base/response-source/range")"
  if [[ "$status" != 206 ]] \
    || ! cmp -s "$expected_range_file" "$body_file" \
    || [[ "$(header_value content-length)" != "$range_length" ]] \
    || [[ "$(header_value content-range)" != "$expected_content_range" ]] \
    || [[ "$(header_value accept-ranges)" != bytes ]] \
    || [[ "$(header_value x-hoplite-source)" != true ]]; then
    echo "$label failed: status=$status size=$(wc -c < "$body_file")" >&2
    diagnose
    exit 1
  fi
}

assert_head_response() {
  local url="$1"
  local expected_status="$2"
  local expected_length="$3"
  local expected_range="${4:-}"
  local status
  status="$(curl --silent --show-error \
    --max-time 15 \
    --head \
    --dump-header "$headers_file" \
    --output /dev/null \
    --write-out '%{http_code}' \
    "$url")"
  if [[ "$status" != "$expected_status" ]] \
    || [[ "$(header_value content-length)" != "$expected_length" ]] \
    || [[ "$(header_value x-hoplite-source)" != true ]]; then
    echo "HEAD response-source conformance failed: status=$status url=$url" >&2
    diagnose
    exit 1
  fi
  if [[ -n "$expected_range" ]] \
    && [[ "$(header_value content-range)" != "$expected_range" ]]; then
    echo "HEAD range metadata mismatch: url=$url" >&2
    diagnose
    exit 1
  fi
}

python3 - "$fixture_file" <<'PY'
from pathlib import Path
import sys

Path(sys.argv[1]).write_bytes(b"0123456789abcdef" * 65536)
PY

expected_size=1048576
expected_digest=aca1cd027e979588d14b877b7b0cb8585ad9fec599eb45801992ee5382b3760f
object_prefix="${expected_digest:0:2}"
object_stem="${expected_digest:2}"
object_root=/var/lib/hoplite/blob/objects/sha256
object_blob="${object_root}/${object_prefix}/${object_stem}.blob"
object_meta="${object_root}/${object_prefix}/${object_stem}.meta"
range_offset=4096
range_length=65536
range_end=$((range_offset + range_length - 1))
expected_content_range="bytes ${range_offset}-${range_end}/${expected_size}"

if [[ "$(wc -c < "$fixture_file" | tr -d ' ')" != "$expected_size" ]] \
  || [[ "$(sha256sum "$fixture_file" | awk '{print $1}')" != "$expected_digest" ]]; then
  echo 'Response-source fixture generation is not deterministic.' >&2
  exit 1
fi

dd if="$fixture_file" \
  of="$expected_range_file" \
  bs=1 \
  skip="$range_offset" \
  count="$range_length" \
  status=none
if [[ "$(wc -c < "$expected_range_file" | tr -d ' ')" != "$range_length" ]]; then
  echo 'Response-source range fixture generation is not deterministic.' >&2
  exit 1
fi

docker volume create "$volume" >/dev/null
start_container

status="$(request POST "$base/response-source" --data-binary "@$fixture_file")"
if [[ "$status" != 200 ]] \
  || ! cmp -s "$fixture_file" "$body_file" \
  || [[ "$(header_value content-length)" != "$expected_size" ]] \
  || [[ "$(header_value x-hoplite-source)" != true ]]; then
  echo "Response-source upload/stream failed: status=$status size=$(wc -c < "$body_file")" >&2
  diagnose
  exit 1
fi

assert_persisted_object 'Initial persistent object installation'
assert_head_response "$base/response-source" 200 "$expected_size"
assert_range_response 'Initial non-zero response-source range'
assert_head_response \
  "$base/response-source/range" \
  206 \
  "$range_length" \
  "$expected_content_range"

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

# Remove the complete serving process, retain only the trusted data volume, and
# prove a fresh worker can independently open the exact immutable object.
docker rm -f "$container" >/dev/null
start_container

assert_persisted_object 'Post-recreation persistent object installation'
assert_full_response 'Post-recreation full response-source retrieval'
assert_range_response 'Post-recreation non-zero response-source range'
assert_head_response "$base/response-source" 200 "$expected_size"

printf 'Validated persistent bounded hara.response-source/1 streaming through %s on port %s.\n' \
  "$image" "$port"
