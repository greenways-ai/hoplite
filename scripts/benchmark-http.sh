#!/usr/bin/env bash
set -euo pipefail

image="${1:-ghcr.io/greenways-ai/hoplite:latest}"
output="${2:-src/data/http-benchmark.json}"
container="hoplite-http-benchmark"
rounds="${HOPLITE_BENCHMARK_ROUNDS:-3}"
duration="${HOPLITE_BENCHMARK_DURATION:-20s}"
threads="${HOPLITE_BENCHMARK_THREADS:-4}"
connections="${HOPLITE_BENCHMARK_CONNECTIONS:-128}"
port="${HOPLITE_BENCHMARK_PORT:-18080}"
url="http://127.0.0.1:${port}/hello"
work="$(mktemp -d "${TMPDIR:-/tmp}/hoplite-benchmark.XXXXXX")"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

command -v docker >/dev/null
command -v wrk >/dev/null
command -v jq >/dev/null
command -v curl >/dev/null

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

docker rm -f "$container" >/dev/null 2>&1 || true
docker run --detach --name "$container" -p "${port}:8080" "$image" >/dev/null

for _ in $(seq 1 60); do
  if curl --fail --silent "$url" >/dev/null; then
    break
  fi
  sleep 1
done
curl --fail --silent "$url" >/dev/null

response_bytes="$(curl --fail --silent "$url" | wc -c | tr -d '[:space:]')"
image_size_bytes="$(docker image inspect "$image" --format '{{.Size}}')"
image_size_mib="$(awk -v bytes="$image_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
cpu="$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n 1)"
logical_cpus="$(nproc)"
: > "$work/samples.ndjson"

# Warm the route before recording the measured rounds.
wrk -t1 -c16 -d2s "$url" >/dev/null

for round in $(seq 1 "$rounds"); do
  log="$work/round-${round}.log"
  memory="$work/round-${round}.memory"
  : > "$memory"

  (
    while docker inspect "$container" >/dev/null 2>&1; do
      docker stats --no-stream --format '{{.MemUsage}}' "$container" 2>/dev/null \
        | awk -F/ '{gsub(/[[:space:]]/, "", $1); print $1}' >> "$memory" || true
      sleep .2
    done
  ) &
  sampler=$!

  wrk -t"$threads" -c"$connections" -d"$duration" --latency "$url" > "$log"
  kill "$sampler" >/dev/null 2>&1 || true
  wait "$sampler" 2>/dev/null || true

  requests_per_second="$(awk '/Requests\/sec:/ {print $2}' "$log")"
  latency_p50_raw="$(awk '$1 == "50%" {print $2}' "$log")"
  latency_p99_raw="$(awk '$1 == "99%" {print $2}' "$log")"
  latency_p50_ms="$(latency_to_ms "$latency_p50_raw")"
  latency_p99_ms="$(latency_to_ms "$latency_p99_raw")"
  peak_memory_mib="$(awk 'NF {print}' "$memory" | while read -r sample; do memory_to_mib "$sample"; echo; done | sort -n | tail -n 1)"
  peak_memory_mib="${peak_memory_mib:-0}"

  jq -n \
    --argjson round "$round" \
    --argjson requestsPerSecond "$requests_per_second" \
    --argjson latencyP50Ms "$latency_p50_ms" \
    --argjson latencyP99Ms "$latency_p99_ms" \
    --argjson peakMemoryMiB "$peak_memory_mib" \
    '{round: $round, requestsPerSecond: $requestsPerSecond, latencyP50Ms: $latencyP50Ms, latencyP99Ms: $latencyP99Ms, peakMemoryMiB: $peakMemoryMiB}' \
    >> "$work/samples.ndjson"
done

commit="${GITHUB_SHA:-$(git rev-parse HEAD)}"
generated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
runner="Ubuntu 24.04 GitHub-hosted runner"
command="wrk -t${threads} -c${connections} -d${duration} --latency ${url}"
mkdir -p "$(dirname "$output")"

jq -s \
  --arg generatedAt "$generated_at" \
  --arg commit "$commit" \
  --arg runner "$runner" \
  --arg cpu "$cpu" \
  --arg command "$command" \
  --argjson logicalCpus "$logical_cpus" \
  --argjson rounds "$rounds" \
  --argjson responseBytes "$response_bytes" \
  --argjson imageSizeMiB "$image_size_mib" '
  def median(values): values | sort | .[(length / 2 | floor)];
  {
    status: "measured",
    generatedAt: $generatedAt,
    commit: $commit,
    runner: $runner,
    cpu: $cpu,
    logicalCpus: $logicalCpus,
    command: $command,
    rounds: $rounds,
    responseBytes: $responseBytes,
    imageSizeMiB: $imageSizeMiB,
    metrics: {
      requestsPerSecond: median(map(.requestsPerSecond)),
      latencyP50Ms: median(map(.latencyP50Ms)),
      latencyP99Ms: median(map(.latencyP99Ms)),
      peakMemoryMiB: (map(.peakMemoryMiB) | max)
    },
    samples: .
  }' "$work/samples.ndjson" > "$output"

cat "$output"
