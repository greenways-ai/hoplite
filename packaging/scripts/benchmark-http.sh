#!/usr/bin/env bash
set -euo pipefail

hoplite_image="${1:-ghcr.io/greenways-ai/hoplite:latest}"
nginx_image="${2:-hoplite-nginx-baseline}"
output="${3:-src/data/http-benchmark.json}"
rounds="${HOPLITE_BENCHMARK_ROUNDS:-3}"
duration="${HOPLITE_BENCHMARK_DURATION:-20s}"
threads="${HOPLITE_BENCHMARK_THREADS:-4}"
connections="${HOPLITE_BENCHMARK_CONNECTIONS:-128}"
idle_samples="${HOPLITE_BENCHMARK_IDLE_SAMPLES:-10}"
run_key="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
run_key="${run_key//[^A-Za-z0-9_.-]/-}"
work="$(mktemp -d "${TMPDIR:-/tmp}/hoplite-benchmark.XXXXXX")"
containers=()

cleanup() {
  for container in "${containers[@]:-}"; do
    docker rm -f "$container" >/dev/null 2>&1 || true
  done
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

require_positive_integer() {
  local label="$1"
  local value="$2"
  if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
    echo "$label must be a positive integer; got $value" >&2
    exit 1
  fi
}

for pair in \
  "rounds:$rounds" \
  "threads:$threads" \
  "connections:$connections" \
  "idle samples:$idle_samples"; do
  require_positive_integer "${pair%%:*}" "${pair#*:}"
done

for command in docker wrk jq curl awk sort cmp; do
  command -v "$command" >/dev/null || {
    echo "Missing benchmark dependency: $command" >&2
    exit 1
  }
done

if command -v sha256sum >/dev/null 2>&1; then
  sha256_file() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_file() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  echo "Missing SHA-256 tool: install sha256sum or shasum" >&2
  exit 1
fi

latency_to_ms() {
  local value="$1"
  awk -v value="$value" 'BEGIN {
    number = value + 0
    if (value ~ /us$/) printf "%.6f", number / 1000
    else if (value ~ /ms$/) printf "%.6f", number
    else if (value ~ /s$/) printf "%.6f", number * 1000
    else printf "%.6f", number
  }'
}

memory_to_mib() {
  local value="$1"
  awk -v value="$value" 'BEGIN {
    number = value + 0
    if (value ~ /GiB$/) printf "%.6f", number * 1024
    else if (value ~ /MiB$/) printf "%.6f", number
    else if (value ~ /KiB$/) printf "%.6f", number / 1024
    else if (value ~ /GB$/) printf "%.6f", number * 953.674
    else if (value ~ /MB$/) printf "%.6f", number * 0.953674
    else if (value ~ /kB$/) printf "%.6f", number / 1073.742
    else if (value ~ /B$/) printf "%.6f", number / 1048576
    else printf "%.6f", number
  }'
}

median_file() {
  awk 'NF { values[++count] = $1 } END {
    if (!count) { print "0"; exit }
    for (i = 1; i <= count; i++) for (j = i + 1; j <= count; j++) if (values[i] > values[j]) {
      temp = values[i]; values[i] = values[j]; values[j] = temp
    }
    if (count % 2) printf "%.6f", values[(count + 1) / 2]
    else printf "%.6f", (values[count / 2] + values[count / 2 + 1]) / 2
  }' "$1"
}

sample_memory() {
  local container="$1"
  docker stats --no-stream --format '{{.MemUsage}}' "$container" 2>/dev/null \
    | awk -F/ '{gsub(/[[:space:]]/, "", $1); print $1}'
}

published_port() {
  local container="$1"
  local port=""
  for _ in $(seq 1 40); do
    port="$(docker port "$container" 8080/tcp 2>/dev/null | head -n 1 | awk -F: '{print $NF}' || true)"
    if [[ -n "$port" ]]; then
      printf '%s\n' "$port"
      return 0
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != "true" ]]; then
      docker inspect "$container" --format '{{json .State}}' >&2 || true
      docker logs "$container" >&2 || true
      echo "Container $container exited before Docker exposed port 8080" >&2
      return 1
    fi
    sleep .1
  done
  docker inspect "$container" --format '{{json .NetworkSettings.Ports}}' >&2 || true
  echo "Docker did not expose port 8080 for $container" >&2
  return 1
}

wait_until_ready() {
  local container="$1"
  local label="$2"
  local url="$3"
  for _ in $(seq 1 60); do
    if curl --fail --silent --show-error --output /dev/null "$url"; then
      return 0
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != "true" ]]; then
      docker logs "$container" >&2 || true
      echo "$label exited before becoming ready" >&2
      return 1
    fi
    sleep 1
  done
  docker logs "$container" >&2 || true
  echo "$label did not become ready" >&2
  return 1
}

header_value() {
  local name="$1"
  local file="$2"
  awk -v expected="$name" '
    BEGIN { expected = tolower(expected) }
    {
      line = $0
      sub(/\r$/, "", line)
      separator = index(line, ":")
      if (separator == 0) next
      key = tolower(substr(line, 1, separator - 1))
      if (key != expected) next
      value = substr(line, separator + 1)
      sub(/^[[:space:]]+/, "", value)
      print value
      exit
    }
  ' "$file"
}

capture_contract() {
  local id="$1"
  local label="$2"
  local url="$3"
  local directory="$work/$id"
  local status content_type content_type_lower x_hoplite x_hoplite_lower content_length response_bytes response_sha

  status="$(curl --silent --show-error \
    --dump-header "$directory/headers" \
    --output "$directory/body" \
    --write-out '%{http_code}' \
    "$url")"
  content_type="$(header_value content-type "$directory/headers")"
  x_hoplite="$(header_value x-hoplite "$directory/headers")"
  content_length="$(header_value content-length "$directory/headers")"
  content_type_lower="$(printf '%s' "$content_type" | tr '[:upper:]' '[:lower:]')"
  x_hoplite_lower="$(printf '%s' "$x_hoplite" | tr '[:upper:]' '[:lower:]')"
  response_bytes="$(wc -c < "$directory/body" | tr -d '[:space:]')"
  response_sha="$(sha256_file "$directory/body")"

  printf 'Hello from Hoplite\n' > "$directory/expected-body"
  if ! cmp -s "$directory/expected-body" "$directory/body"; then
    echo "$label returned an unexpected response body" >&2
    exit 1
  fi
  if [[ "$status" != "200" ]]; then
    echo "$label returned HTTP $status instead of 200" >&2
    exit 1
  fi
  if [[ "$content_type_lower" != "text/plain; charset=utf-8" ]]; then
    echo "$label returned content-type ${content_type:-<missing>}" >&2
    exit 1
  fi
  if [[ "$x_hoplite_lower" != "true" ]]; then
    echo "$label returned x-hoplite ${x_hoplite:-<missing>}" >&2
    exit 1
  fi
  if [[ -n "$content_length" && "$content_length" != "$response_bytes" ]]; then
    echo "$label returned content-length $content_length for $response_bytes bytes" >&2
    exit 1
  fi

  jq -n \
    --argjson status "$status" \
    --arg contentType "$content_type_lower" \
    --arg xHoplite "$x_hoplite_lower" \
    --argjson bodyBytes "$response_bytes" \
    --arg bodySha256 "$response_sha" \
    '{status: $status, contentType: $contentType, xHoplite: $xHoplite, bodyBytes: $bodyBytes, bodySha256: $bodySha256}' \
    > "$directory/contract.json"
}

start_target() {
  local id="$1"
  local label="$2"
  local image="$3"
  local executable="$4"
  local container="hoplite-benchmark-${run_key}-${id}"
  local directory="$work/$id"
  mkdir -p "$directory"
  : > "$directory/samples.ndjson"
  containers+=("$container")

  docker run --detach --name "$container" -p 127.0.0.1::8080 "$image" >/dev/null
  local port url
  port="$(published_port "$container")"
  url="http://127.0.0.1:${port}/hello"
  printf '%s\n' "$container" > "$directory/container"
  printf '%s\n' "$url" > "$directory/url"
  wait_until_ready "$container" "$label" "$url"
  capture_contract "$id" "$label" "$url"

  local image_size_bytes image_size_mib executable_size_bytes executable_size_mib
  local process_count worker_count image_id
  image_size_bytes="$(docker image inspect "$image" --format '{{.Size}}')"
  image_size_mib="$(awk -v bytes="$image_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
  image_id="$(docker image inspect "$image" --format '{{.Id}}')"
  executable_size_bytes="$(docker exec "$container" stat -c '%s' "$executable")"
  executable_size_mib="$(awk -v bytes="$executable_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
  process_count="$(docker top "$container" -eo pid | awk 'NR > 1 && NF { count++ } END { print count + 0 }')"
  worker_count="$(docker top "$container" -eo args | awk 'NR > 1 && /nginx: worker process/ { count++ } END { print count + 0 }')"

  : > "$directory/idle.memory"
  for _ in $(seq 1 "$idle_samples"); do
    sample_memory "$container" \
      | while read -r sample; do memory_to_mib "$sample"; echo; done \
      >> "$directory/idle.memory"
    sleep .2
  done
  local idle_memory_mib
  idle_memory_mib="$(median_file "$directory/idle.memory")"

  jq -n \
    --arg id "$id" \
    --arg label "$label" \
    --arg image "$image" \
    --arg imageId "$image_id" \
    --arg executable "$executable" \
    --argjson imageSizeMiB "$image_size_mib" \
    --argjson executableSizeMiB "$executable_size_mib" \
    --argjson processCount "$process_count" \
    --argjson nginxWorkerCount "$worker_count" \
    --argjson idleMemoryMiB "$idle_memory_mib" \
    '{id: $id, label: $label, image: $image, imageId: $imageId, executable: $executable, imageSizeMiB: $imageSizeMiB, executableSizeMiB: $executableSizeMiB, processCount: $processCount, nginxWorkerCount: $nginxWorkerCount, idleMemoryMiB: $idleMemoryMiB}' \
    > "$directory/meta.json"
}

run_round() {
  local id="$1"
  local round="$2"
  local order_in_round="$3"
  local sequence="$4"
  local directory="$work/$id"
  local container url log memory sampler
  container="$(cat "$directory/container")"
  url="$(cat "$directory/url")"
  log="$directory/round-${round}.log"
  memory="$directory/round-${round}.memory"
  : > "$memory"

  (
    while docker inspect "$container" >/dev/null 2>&1; do
      sample_memory "$container" \
        | while read -r sample; do memory_to_mib "$sample"; echo; done \
        >> "$memory" || true
      sleep .2
    done
  ) &
  sampler=$!

  wrk -t"$threads" -c"$connections" -d"$duration" --latency "$url" > "$log"
  kill "$sampler" >/dev/null 2>&1 || true
  wait "$sampler" 2>/dev/null || true

  local requests_per_second latency_p50_raw latency_p99_raw latency_p50_ms latency_p99_ms peak_memory_mib
  requests_per_second="$(awk '/Requests\/sec:/ {print $2}' "$log")"
  latency_p50_raw="$(awk '$1 == "50%" {print $2}' "$log")"
  latency_p99_raw="$(awk '$1 == "99%" {print $2}' "$log")"
  if [[ -z "$requests_per_second" || -z "$latency_p50_raw" || -z "$latency_p99_raw" ]]; then
    cat "$log" >&2
    echo "Could not parse wrk output for $id round $round" >&2
    exit 1
  fi
  latency_p50_ms="$(latency_to_ms "$latency_p50_raw")"
  latency_p99_ms="$(latency_to_ms "$latency_p99_raw")"
  peak_memory_mib="$(sort -n "$memory" | tail -n 1)"
  peak_memory_mib="${peak_memory_mib:-0}"

  jq -n \
    --argjson round "$round" \
    --argjson orderInRound "$order_in_round" \
    --argjson sequence "$sequence" \
    --argjson requestsPerSecond "$requests_per_second" \
    --argjson latencyP50Ms "$latency_p50_ms" \
    --argjson latencyP99Ms "$latency_p99_ms" \
    --argjson peakMemoryMiB "$peak_memory_mib" \
    '{round: $round, orderInRound: $orderInRound, sequence: $sequence, requestsPerSecond: $requestsPerSecond, latencyP50Ms: $latencyP50Ms, latencyP99Ms: $latencyP99Ms, peakMemoryMiB: $peakMemoryMiB}' \
    >> "$directory/samples.ndjson"
}

finalize_target() {
  local id="$1"
  local directory="$work/$id"
  jq -s \
    --slurpfile meta "$directory/meta.json" \
    --slurpfile contract "$directory/contract.json" '
    def median:
      sort as $values
      | ($values | length) as $count
      | if $count == 0 then null
        elif ($count % 2) == 1 then $values[($count / 2 | floor)]
        else (($values[($count / 2 - 1)] + $values[($count / 2)]) / 2)
        end;
    $meta[0] + {
      responseContract: $contract[0],
      metrics: {
        requestsPerSecond: (map(.requestsPerSecond) | median),
        latencyP50Ms: (map(.latencyP50Ms) | median),
        latencyP99Ms: (map(.latencyP99Ms) | median),
        peakMemoryMiB: (map(.peakMemoryMiB) | max)
      },
      samples: .
    }' "$directory/samples.ndjson" > "$directory/target.json"
}

host_cpu() {
  if command -v lscpu >/dev/null 2>&1; then
    lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n 1
  elif command -v sysctl >/dev/null 2>&1; then
    sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m
  else
    uname -m
  fi
}

host_cpus() {
  if command -v nproc >/dev/null 2>&1; then
    nproc
  elif command -v sysctl >/dev/null 2>&1; then
    sysctl -n hw.logicalcpu
  else
    printf '1\n'
  fi
}

start_target hoplite "Hoplite server" "$hoplite_image" /usr/local/bin/hoplite-server
start_target nginx "Plain Nginx" "$nginx_image" /opt/nginx/sbin/nginx

if ! cmp -s <(jq -S . "$work/hoplite/contract.json") <(jq -S . "$work/nginx/contract.json"); then
  echo "Hoplite and Nginx did not return the same stable response contract" >&2
  diff -u <(jq -S . "$work/hoplite/contract.json") <(jq -S . "$work/nginx/contract.json") >&2 || true
  exit 1
fi

for id in hoplite nginx; do
  url="$(cat "$work/$id/url")"
  wrk -t1 -c16 -d2s "$url" >/dev/null
done

sequence=0
for round in $(seq 1 "$rounds"); do
  if (( round % 2 == 1 )); then
    order=(hoplite nginx)
  else
    order=(nginx hoplite)
  fi
  order_in_round=0
  for id in "${order[@]}"; do
    sequence=$((sequence + 1))
    order_in_round=$((order_in_round + 1))
    run_round "$id" "$round" "$order_in_round" "$sequence"
  done
done

finalize_target hoplite
finalize_target nginx

repository_root="${HOPLITE_REPOSITORY_ROOT:-.}"
commit="${HOPLITE_BENCHMARK_COMMIT:-${GITHUB_SHA:-$(git -C "$repository_root" rev-parse HEAD)}}"
generated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cpu="$(host_cpu)"
logical_cpus="$(host_cpus)"
runner="${HOPLITE_BENCHMARK_RUNNER:-${RUNNER_NAME:-local}}"
hara_revision="${HOPLITE_REVISION:-unknown}"
nginx_version="${HOPLITE_NGINX_VERSION:-unknown}"
workflow_run_id="${GITHUB_RUN_ID:-}"
workflow_run_url=""
if [[ -n "$workflow_run_id" && -n "${GITHUB_SERVER_URL:-}" && -n "${GITHUB_REPOSITORY:-}" ]]; then
  workflow_run_url="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${workflow_run_id}"
fi
mkdir -p "$(dirname "$output")"

jq -n \
  --arg generatedAt "$generated_at" \
  --arg commit "$commit" \
  --arg runner "$runner" \
  --arg cpu "$cpu" \
  --arg duration "$duration" \
  --arg haraRevision "$hara_revision" \
  --arg nginxVersion "$nginx_version" \
  --arg workflowRunId "$workflow_run_id" \
  --arg workflowRunUrl "$workflow_run_url" \
  --argjson logicalCpus "$logical_cpus" \
  --argjson threads "$threads" \
  --argjson connections "$connections" \
  --argjson rounds "$rounds" \
  --argjson idleSamples "$idle_samples" \
  --slurpfile contract "$work/hoplite/contract.json" \
  --slurpfile hoplite "$work/hoplite/target.json" \
  --slurpfile nginx "$work/nginx/target.json" '
  {
    schemaVersion: 2,
    status: "measured",
    benchmark: "equivalent-payload-http",
    generatedAt: $generatedAt,
    commit: $commit,
    runner: $runner,
    cpu: $cpu,
    logicalCpus: $logicalCpus,
    provenance: {
      haraRevision: $haraRevision,
      nginxVersion: $nginxVersion,
      workflowRunId: (if $workflowRunId == "" then null else ($workflowRunId | tonumber) end),
      workflowRunUrl: (if $workflowRunUrl == "" then null else $workflowRunUrl end)
    },
    load: {threads: $threads, connections: $connections, duration: $duration, rounds: $rounds},
    methodology: {
      warmup: "2 seconds per target before measured rounds",
      scheduling: "Target order alternates on every round",
      idleSamplesPerTarget: $idleSamples,
      memoryAccounting: "Docker container memory as reported by docker stats",
      scope: "One fixed synchronous route; not an application-capacity or framework-feature benchmark"
    },
    responseContract: ($contract[0] + {matchedAcrossTargets: true}),
    payload: {
      bodyBytes: $contract[0].bodyBytes,
      sha256: $contract[0].bodySha256,
      body: "Hello from Hoplite\n"
    },
    targets: {hoplite: $hoplite[0], nginx: $nginx[0]},
    comparison: {
      throughputPercentOfNginx: (($hoplite[0].metrics.requestsPerSecond / $nginx[0].metrics.requestsPerSecond) * 100),
      requestRateDeltaPercent: ((($hoplite[0].metrics.requestsPerSecond - $nginx[0].metrics.requestsPerSecond) / $nginx[0].metrics.requestsPerSecond) * 100),
      p50DeltaMs: ($hoplite[0].metrics.latencyP50Ms - $nginx[0].metrics.latencyP50Ms),
      p99DeltaMs: ($hoplite[0].metrics.latencyP99Ms - $nginx[0].metrics.latencyP99Ms),
      idleMemoryDeltaMiB: ($hoplite[0].idleMemoryMiB - $nginx[0].idleMemoryMiB),
      peakMemoryDeltaMiB: ($hoplite[0].metrics.peakMemoryMiB - $nginx[0].metrics.peakMemoryMiB),
      imageSizeDeltaMiB: ($hoplite[0].imageSizeMiB - $nginx[0].imageSizeMiB),
      executableSizeDeltaMiB: ($hoplite[0].executableSizeMiB - $nginx[0].executableSizeMiB)
    }
  }' > "$output"

cat "$output"
