#!/usr/bin/env bash
set -euo pipefail

hoplite_image="${1:-ghcr.io/greenways-ai/hoplite:latest}"
nginx_image="${2:-hoplite-nginx-baseline}"
output="${3:-src/data/http-benchmark.json}"
rounds="${HOPLITE_BENCHMARK_ROUNDS:-3}"
duration="${HOPLITE_BENCHMARK_DURATION:-20s}"
threads="${HOPLITE_BENCHMARK_THREADS:-4}"
connections="${HOPLITE_BENCHMARK_CONNECTIONS:-128}"
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

for command in docker wrk jq curl sha256sum awk sort; do command -v "$command" >/dev/null; done

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

sample_memory() {
  local container="$1"
  docker stats --no-stream --format '{{.MemUsage}}' "$container" 2>/dev/null \
    | awk -F/ '{gsub(/[[:space:]]/, "", $1); print $1}'
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

published_port() {
  local container="$1"
  docker port "$container" 8080/tcp | head -n 1 | awk -F: '{print $NF}'
}

measure_target() {
  local id="$1"
  local label="$2"
  local image="$3"
  local executable="$4"
  local container="hoplite-benchmark-${run_key}-${id}"
  local target_dir="$work/$id"
  mkdir -p "$target_dir"
  containers+=("$container")

  docker run --detach --name "$container" -p 127.0.0.1::8080 "$image" >/dev/null
  local port url
  port="$(published_port "$container")"
  url="http://127.0.0.1:${port}/hello"

  for _ in $(seq 1 60); do
    if curl --fail --silent "$url" > "$target_dir/body"; then break; fi
    sleep 1
  done
  curl --fail --silent "$url" > "$target_dir/body"

  local response_bytes response_sha image_size_bytes image_size_mib executable_size_bytes executable_size_mib
  response_bytes="$(wc -c < "$target_dir/body" | tr -d '[:space:]')"
  response_sha="$(sha256sum "$target_dir/body" | awk '{print $1}')"
  image_size_bytes="$(docker image inspect "$image" --format '{{.Size}}')"
  image_size_mib="$(awk -v bytes="$image_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
  executable_size_bytes="$(docker exec "$container" stat -c '%s' "$executable")"
  executable_size_mib="$(awk -v bytes="$executable_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"

  : > "$target_dir/idle.memory"
  for _ in $(seq 1 10); do
    sample_memory "$container" | while read -r sample; do memory_to_mib "$sample"; echo; done >> "$target_dir/idle.memory"
    sleep .2
  done
  local idle_memory_mib
  idle_memory_mib="$(median_file "$target_dir/idle.memory")"

  : > "$target_dir/samples.ndjson"
  wrk -t1 -c16 -d2s "$url" >/dev/null

  for round in $(seq 1 "$rounds"); do
    local log="$target_dir/round-${round}.log"
    local memory="$target_dir/round-${round}.memory"
    : > "$memory"

    (
      while docker inspect "$container" >/dev/null 2>&1; do
        sample_memory "$container" | while read -r sample; do memory_to_mib "$sample"; echo; done >> "$memory" || true
        sleep .2
      done
    ) &
    local sampler=$!

    wrk -t"$threads" -c"$connections" -d"$duration" --latency "$url" > "$log"
    kill "$sampler" >/dev/null 2>&1 || true
    wait "$sampler" 2>/dev/null || true

    local requests_per_second latency_p50_raw latency_p99_raw latency_p50_ms latency_p99_ms peak_memory_mib
    requests_per_second="$(awk '/Requests\/sec:/ {print $2}' "$log")"
    latency_p50_raw="$(awk '$1 == "50%" {print $2}' "$log")"
    latency_p99_raw="$(awk '$1 == "99%" {print $2}' "$log")"
    latency_p50_ms="$(latency_to_ms "$latency_p50_raw")"
    latency_p99_ms="$(latency_to_ms "$latency_p99_raw")"
    peak_memory_mib="$(sort -n "$memory" | tail -n 1)"
    peak_memory_mib="${peak_memory_mib:-0}"

    jq -n \
      --argjson round "$round" \
      --argjson requestsPerSecond "$requests_per_second" \
      --argjson latencyP50Ms "$latency_p50_ms" \
      --argjson latencyP99Ms "$latency_p99_ms" \
      --argjson peakMemoryMiB "$peak_memory_mib" \
      '{round: $round, requestsPerSecond: $requestsPerSecond, latencyP50Ms: $latencyP50Ms, latencyP99Ms: $latencyP99Ms, peakMemoryMiB: $peakMemoryMiB}' \
      >> "$target_dir/samples.ndjson"
  done

  jq -s \
    --arg id "$id" \
    --arg label "$label" \
    --arg image "$image" \
    --arg executable "$executable" \
    --arg responseSha256 "$response_sha" \
    --argjson responseBytes "$response_bytes" \
    --argjson imageSizeMiB "$image_size_mib" \
    --argjson executableSizeMiB "$executable_size_mib" \
    --argjson idleMemoryMiB "$idle_memory_mib" '
    def median(values): values | sort | .[(length / 2 | floor)];
    {
      id: $id,
      label: $label,
      image: $image,
      executable: $executable,
      responseBytes: $responseBytes,
      responseSha256: $responseSha256,
      imageSizeMiB: $imageSizeMiB,
      executableSizeMiB: $executableSizeMiB,
      idleMemoryMiB: $idleMemoryMiB,
      metrics: {
        requestsPerSecond: median(map(.requestsPerSecond)),
        latencyP50Ms: median(map(.latencyP50Ms)),
        latencyP99Ms: median(map(.latencyP99Ms)),
        peakMemoryMiB: (map(.peakMemoryMiB) | max)
      },
      samples: .
    }' "$target_dir/samples.ndjson" > "$target_dir/target.json"

  docker rm -f "$container" >/dev/null
}

measure_target hoplite Hoplite "$hoplite_image" /usr/local/bin/hoplite
measure_target nginx "Plain Nginx" "$nginx_image" /opt/nginx/sbin/nginx

hoplite_sha="$(jq -r '.responseSha256' "$work/hoplite/target.json")"
nginx_sha="$(jq -r '.responseSha256' "$work/nginx/target.json")"
if [[ "$hoplite_sha" != "$nginx_sha" ]]; then
  echo "Hoplite and Nginx returned different response bodies" >&2
  exit 1
fi

commit="${GITHUB_SHA:-$(git rev-parse HEAD)}"
generated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cpu="$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n 1)"
logical_cpus="$(nproc)"
mkdir -p "$(dirname "$output")"

jq -n \
  --arg generatedAt "$generated_at" \
  --arg commit "$commit" \
  --arg runner "Ubuntu 24.04 GitHub-hosted runner" \
  --arg cpu "$cpu" \
  --arg duration "$duration" \
  --argjson logicalCpus "$logical_cpus" \
  --argjson threads "$threads" \
  --argjson connections "$connections" \
  --argjson rounds "$rounds" \
  --slurpfile hoplite "$work/hoplite/target.json" \
  --slurpfile nginx "$work/nginx/target.json" '
  {
    status: "measured",
    generatedAt: $generatedAt,
    commit: $commit,
    runner: $runner,
    cpu: $cpu,
    logicalCpus: $logicalCpus,
    load: {threads: $threads, connections: $connections, duration: $duration, rounds: $rounds},
    payload: {
      bodyBytes: $hoplite[0].responseBytes,
      sha256: $hoplite[0].responseSha256,
      body: "Hello from Hoplite\\n"
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
