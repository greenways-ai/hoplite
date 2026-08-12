#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
container="hoplite-worker-reload-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
container="${container//[^A-Za-z0-9_.-]/-}"
body_file="$(mktemp)"
headers_file="$(mktemp)"
before_workers="$(mktemp)"
after_workers="$(mktemp)"
expected_body='Hello from Hoplite'
port=''
base=''

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$body_file" "$headers_file" "$before_workers" "$after_workers"
}
trap cleanup EXIT INT TERM

diagnose() {
  echo '--- last response headers ---' >&2
  sed -n '1,80p' "$headers_file" >&2 || true
  echo '--- last response body ---' >&2
  cat "$body_file" >&2 || true
  echo '--- docker ps ---' >&2
  docker ps -a --filter "name=^/${container}$" >&2 || true
  echo '--- container state ---' >&2
  docker inspect "$container" --format '{{json .State}}' >&2 || true
  echo '--- container processes ---' >&2
  docker top "$container" -eo pid,ppid,args >&2 || true
  echo '--- container logs ---' >&2
  docker logs "$container" >&2 || true
  echo '--- Hoplite error log ---' >&2
  docker exec "$container" sh -c \
    'tail -n 240 /app/.hoplite/error.log 2>/dev/null || true' >&2 || true
  echo '--- generated Nginx configuration ---' >&2
  docker exec "$container" sh -c \
    'cat /app/.hoplite/conf/nginx.conf 2>/dev/null || true' >&2 || true
}

request() {
  curl --silent --show-error \
    --connect-timeout 1 \
    --max-time 3 \
    --dump-header "$headers_file" \
    --output "$body_file" \
    --write-out '%{http_code}' \
    "$base/hello"
}

master_pid() {
  docker top "$container" -eo pid,ppid,args \
    | awk 'NR > 1 && /nginx: master process/ { print $1; exit }'
}

worker_pids() {
  docker top "$container" -eo pid,ppid,args \
    | awk 'NR > 1 && /nginx: worker process/ && !/worker process is shutting down/ { print $1 }' \
    | LC_ALL=C sort -n
}

artifact_manifest() {
  docker exec "$container" sh -c '
    cd /app/.hoplite
    sha256sum app.hbx apps.hta conf/nginx.conf | LC_ALL=C sort
  '
}

assert_ready() {
  local label="$1"
  local ready=false
  local status='000'
  for _ in $(seq 1 90); do
    status="$(request || true)"
    if [[ "$status" == 200 ]] \
      && [[ "$(cat "$body_file")" == "$expected_body" ]]; then
      ready=true
      break
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]]; then
      break
    fi
    sleep .2
  done
  if [[ "$ready" != true ]]; then
    echo "$label failed: status=$status" >&2
    diagnose
    exit 1
  fi
}

assert_worker_count() {
  local label="$1"
  local actual
  worker_pids > "$after_workers"
  actual="$(wc -l < "$after_workers" | tr -d '[:space:]')"
  if [[ "$actual" != 2 ]]; then
    echo "$label failed: expected 2 workers, found $actual" >&2
    diagnose
    exit 1
  fi
}

start_container() {
  docker run --detach \
    --name "$container" \
    -e HOPLITE_WORKERS=2 \
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
  assert_ready 'Hoplite worker startup'
  assert_worker_count 'Hoplite worker startup'
}

reload_generation() {
  local generation="$1"
  local expected_master="$2"
  local expected_artifacts="$3"
  local current_master=''
  local current_artifacts=''
  local reloaded=false

  worker_pids > "$before_workers"
  docker kill --signal HUP "$container" >/dev/null

  for _ in $(seq 1 150); do
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != true ]]; then
      break
    fi
    current_master="$(master_pid || true)"
    worker_pids > "$after_workers"
    if [[ -n "$current_master" ]] \
      && [[ "$current_master" == "$expected_master" ]] \
      && [[ "$(wc -l < "$after_workers" | tr -d '[:space:]')" == 2 ]] \
      && ! comm -12 "$before_workers" "$after_workers" | grep -q . \
      && [[ "$(request || true)" == 200 ]] \
      && [[ "$(cat "$body_file")" == "$expected_body" ]]; then
      reloaded=true
      break
    fi
    sleep .2
  done

  if [[ "$reloaded" != true ]]; then
    echo "reload generation $generation did not replace both workers cleanly" >&2
    diagnose
    exit 1
  fi

  current_artifacts="$(artifact_manifest)"
  if [[ "$current_artifacts" != "$expected_artifacts" ]]; then
    echo "reload generation $generation changed immutable application artifacts" >&2
    diff -u <(printf '%s\n' "$expected_artifacts") \
      <(printf '%s\n' "$current_artifacts") >&2 || true
    diagnose
    exit 1
  fi

  for _ in $(seq 1 8); do
    if [[ "$(request)" != 200 ]] \
      || [[ "$(cat "$body_file")" != "$expected_body" ]]; then
      echo "reload generation $generation damaged repeated dispatch" >&2
      diagnose
      exit 1
    fi
  done

  printf 'Validated worker reload generation %s with master %s and workers %s.\n' \
    "$generation" \
    "$current_master" \
    "$(tr '\n' ',' < "$after_workers" | sed 's/,$//')"
}

start_container
initial_master="$(master_pid)"
initial_artifacts="$(artifact_manifest)"
if [[ -z "$initial_master" ]]; then
  echo 'Could not identify the initial Nginx master process.' >&2
  diagnose
  exit 1
fi

reload_generation 1 "$initial_master" "$initial_artifacts"
reload_generation 2 "$initial_master" "$initial_artifacts"

# Remove the complete serving process and prove that the same immutable image
# can create a fresh master and workers without changing the generated bundle,
# exact manifest, or Nginx configuration.
docker rm -f "$container" >/dev/null
start_container
recreated_master="$(master_pid)"
recreated_artifacts="$(artifact_manifest)"
if [[ -z "$recreated_master" ]] \
  || [[ "$recreated_master" == "$initial_master" ]] \
  || [[ "$recreated_artifacts" != "$initial_artifacts" ]]; then
  echo 'Fresh container recreation did not preserve immutable startup evidence.' >&2
  diagnose
  exit 1
fi

printf 'Validated two graceful worker reloads and fresh source-free recreation through %s on port %s.\n' \
  "$image" "$port"
