#!/usr/bin/env bash
set -euo pipefail

hoplite_image="${1:-hoplite-footprint}"
nginx_image="${2:-hoplite-nginx-baseline}"
java_image="${3:-hoplite-footprint-java}"
python_image="${4:-hoplite-footprint-python}"
lua_image="${5:-hoplite-footprint-lua}"
output="${6:-src/data/stack-footprints.json}"
idle_samples="${HOPLITE_FOOTPRINT_IDLE_SAMPLES:-12}"
run_key="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
run_key="${run_key//[^A-Za-z0-9_.-]/-}"
work="$(mktemp -d "${TMPDIR:-/tmp}/hoplite-footprints.XXXXXX")"
containers=()

cleanup() {
  for container in "${containers[@]:-}"; do
    docker rm -f "$container" >/dev/null 2>&1 || true
  done
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

if ! [[ "$idle_samples" =~ ^[1-9][0-9]*$ ]]; then
  echo "HOPLITE_FOOTPRINT_IDLE_SAMPLES must be a positive integer" >&2
  exit 1
fi

for command in docker jq curl awk cmp; do
  command -v "$command" >/dev/null || {
    echo "Missing footprint dependency: $command" >&2
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

wait_and_capture() {
  local container="$1"
  local label="$2"
  local url="$3"
  local directory="$4"
  local ready=false
  for _ in $(seq 1 60); do
    if curl --fail --silent --show-error --output /dev/null "$url"; then
      ready=true
      break
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != "true" ]]; then
      docker logs "$container" >&2 || true
      echo "$label exited before becoming ready" >&2
      exit 1
    fi
    sleep 1
  done
  if [[ "$ready" != "true" ]]; then
    docker logs "$container" >&2 || true
    echo "$label did not become ready" >&2
    exit 1
  fi

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
  if [[ "$status" != "200" || "$content_type_lower" != "text/plain; charset=utf-8" || "$x_hoplite_lower" != "true" ]]; then
    echo "$label did not match the footprint response contract" >&2
    printf 'status=%s content-type=%s x-hoplite=%s\n' "$status" "${content_type:-<missing>}" "${x_hoplite:-<missing>}" >&2
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

measure_component() {
  local id="$1"
  local label="$2"
  local kind="$3"
  local image="$4"
  local artifact="$5"
  local container="hoplite-footprint-${run_key}-${id}"
  local directory="$work/$id"
  mkdir -p "$directory"
  containers+=("$container")

  docker run --detach --name "$container" -p 127.0.0.1::8080 "$image" >/dev/null
  local port url
  port="$(published_port "$container")"
  url="http://127.0.0.1:${port}/hello"
  wait_and_capture "$container" "$label" "$url" "$directory"

  local image_size_bytes image_size_mib artifact_size_bytes artifact_size_mib
  local process_count worker_count image_id
  image_size_bytes="$(docker image inspect "$image" --format '{{.Size}}')"
  image_size_mib="$(awk -v bytes="$image_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
  image_id="$(docker image inspect "$image" --format '{{.Id}}')"
  artifact_size_bytes="$(docker exec "$container" stat -c '%s' "$artifact")"
  artifact_size_mib="$(awk -v bytes="$artifact_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
  process_count="$(docker top "$container" -eo pid | awk 'NR > 1 && NF { count++ } END { print count + 0 }')"
  worker_count="$(docker top "$container" -eo args | awk 'NR > 1 && /nginx: worker process/ { count++ } END { print count + 0 }')"

  : > "$directory/memory"
  for _ in $(seq 1 "$idle_samples"); do
    docker stats --no-stream --format '{{.MemUsage}}' "$container" 2>/dev/null \
      | awk -F/ '{gsub(/[[:space:]]/, "", $1); print $1}' \
      | while read -r sample; do memory_to_mib "$sample"; echo; done \
      >> "$directory/memory"
    sleep .25
  done
  local idle_memory_mib
  idle_memory_mib="$(median_file "$directory/memory")"

  jq -n \
    --arg id "$id" \
    --arg label "$label" \
    --arg kind "$kind" \
    --arg image "$image" \
    --arg imageId "$image_id" \
    --arg artifact "$artifact" \
    --argjson imageSizeMiB "$image_size_mib" \
    --argjson artifactSizeMiB "$artifact_size_mib" \
    --argjson processCount "$process_count" \
    --argjson nginxWorkerCount "$worker_count" \
    --argjson idleMemoryMiB "$idle_memory_mib" \
    --slurpfile contract "$directory/contract.json" \
    '{id: $id, label: $label, kind: $kind, image: $image, imageId: $imageId, artifact: $artifact, responseContract: $contract[0], imageSizeMiB: $imageSizeMiB, artifactSizeMiB: $artifactSizeMiB, processCount: $processCount, nginxWorkerCount: $nginxWorkerCount, idleMemoryMiB: $idleMemoryMiB}' \
    > "$directory/component.json"

  docker rm -f "$container" >/dev/null
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

measure_component hoplite "Hoplite server" production-server "$hoplite_image" /usr/local/bin/hoplite-server
measure_component nginx "Plain Nginx" reverse-proxy "$nginx_image" /opt/nginx/sbin/nginx
measure_component java "JDK HTTP server sample" runtime-sample "$java_image" /app/app.jar
measure_component python "Python stdlib HTTP server sample" runtime-sample "$python_image" /app/server.py
measure_component lua "Nginx + ngx_http_lua sample" in-process-script "$lua_image" /usr/sbin/nginx

for component in nginx java python lua; do
  if ! cmp -s \
    <(jq -S . "$work/hoplite/contract.json") \
    <(jq -S . "$work/$component/contract.json"); then
    echo "$component returned a different stable response contract" >&2
    diff -u \
      <(jq -S . "$work/hoplite/contract.json") \
      <(jq -S . "$work/$component/contract.json") >&2 || true
    exit 1
  fi
done

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
  --arg haraRevision "$hara_revision" \
  --arg nginxVersion "$nginx_version" \
  --arg workflowRunId "$workflow_run_id" \
  --arg workflowRunUrl "$workflow_run_url" \
  --argjson logicalCpus "$logical_cpus" \
  --argjson idleSamples "$idle_samples" \
  --slurpfile contract "$work/hoplite/contract.json" \
  --slurpfile hoplite "$work/hoplite/component.json" \
  --slurpfile nginx "$work/nginx/component.json" \
  --slurpfile java "$work/java/component.json" \
  --slurpfile python "$work/python/component.json" \
  --slurpfile lua "$work/lua/component.json" '
  {
    schemaVersion: 2,
    status: "measured",
    benchmark: "deployment-footprints",
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
    responseContract: ($contract[0] + {matchedAcrossTargets: true}),
    methodology: {
      idleSamplesPerComponent: $idleSamples,
      payloadBytes: $contract[0].bodyBytes,
      imageAccounting: "Logical Docker image sizes are summed for multi-service stacks; shared registry layers may deduplicate in practice.",
      memoryAccounting: "Median Docker container memory from idle samples after the route became ready.",
      scope: "Minimal deployment-envelope samples returning one fixed response. The JDK and Python targets are standard-library servers, not framework benchmarks. The Lua target uses Debian ngx_http_lua, not a full OpenResty distribution."
    },
    components: {
      hoplite: $hoplite[0],
      nginx: $nginx[0],
      java: $java[0],
      python: $python[0],
      lua: $lua[0]
    },
    stacks: {
      hoplite: {
        label: "Hoplite server",
        components: [$hoplite[0].label],
        serviceCount: 1,
        processCount: $hoplite[0].processCount,
        deploymentImageMiB: $hoplite[0].imageSizeMiB,
        idleMemoryMiB: $hoplite[0].idleMemoryMiB,
        primaryArtifactMiB: $hoplite[0].artifactSizeMiB
      },
      java: {
        label: "Nginx + JDK sample",
        components: [$nginx[0].label, $java[0].label],
        serviceCount: 2,
        processCount: ($nginx[0].processCount + $java[0].processCount),
        deploymentImageMiB: ($nginx[0].imageSizeMiB + $java[0].imageSizeMiB),
        idleMemoryMiB: ($nginx[0].idleMemoryMiB + $java[0].idleMemoryMiB),
        primaryArtifactMiB: $java[0].artifactSizeMiB
      },
      python: {
        label: "Nginx + Python stdlib sample",
        components: [$nginx[0].label, $python[0].label],
        serviceCount: 2,
        processCount: ($nginx[0].processCount + $python[0].processCount),
        deploymentImageMiB: ($nginx[0].imageSizeMiB + $python[0].imageSizeMiB),
        idleMemoryMiB: ($nginx[0].idleMemoryMiB + $python[0].idleMemoryMiB),
        primaryArtifactMiB: $python[0].artifactSizeMiB
      },
      lua: {
        label: "Nginx + ngx_http_lua sample",
        components: [$lua[0].label],
        serviceCount: 1,
        processCount: $lua[0].processCount,
        deploymentImageMiB: $lua[0].imageSizeMiB,
        idleMemoryMiB: $lua[0].idleMemoryMiB,
        primaryArtifactMiB: $lua[0].artifactSizeMiB
      }
    }
  }' > "$output"

cat "$output"
