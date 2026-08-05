#!/usr/bin/env bash
set -euo pipefail

hoplite_image="${1:-hoplite-footprint}"
nginx_image="${2:-hoplite-nginx-baseline}"
java_image="${3:-hoplite-footprint-java}"
python_image="${4:-hoplite-footprint-python}"
lua_image="${5:-hoplite-footprint-lua}"
output="${6:-src/data/stack-footprints.json}"
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

for command in docker jq curl sha256sum awk sort; do command -v "$command" >/dev/null; done

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
      echo "Container $container exited before Docker exposed its port" >&2
      return 1
    fi
    sleep .1
  done
  docker inspect "$container" --format '{{json .NetworkSettings.Ports}}' >&2 || true
  echo "Docker did not expose port 8080 for $container" >&2
  return 1
}

measure_component() {
  local id="$1"
  local label="$2"
  local image="$3"
  local artifact="$4"
  local container="hoplite-footprint-${run_key}-${id}"
  local directory="$work/$id"
  mkdir -p "$directory"
  containers+=("$container")

  docker run --detach --name "$container" -p 127.0.0.1::8080 "$image" >/dev/null
  local port url
  port="$(published_port "$container")"
  url="http://127.0.0.1:${port}/hello"

  local ready=false
  for _ in $(seq 1 60); do
    if curl --fail --silent "$url" > "$directory/body"; then
      ready=true
      break
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != "true" ]]; then
      docker logs "$container" >&2 || true
      echo "$label container exited before becoming ready" >&2
      exit 1
    fi
    sleep 1
  done
  if [[ "$ready" != "true" ]]; then
    docker logs "$container" >&2 || true
    echo "$label did not become ready" >&2
    exit 1
  fi
  curl --fail --silent "$url" > "$directory/body"

  local response_bytes response_sha image_size_bytes image_size_mib artifact_size_bytes artifact_size_mib
  response_bytes="$(wc -c < "$directory/body" | tr -d '[:space:]')"
  response_sha="$(sha256sum "$directory/body" | awk '{print $1}')"
  image_size_bytes="$(docker image inspect "$image" --format '{{.Size}}')"
  image_size_mib="$(awk -v bytes="$image_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"
  artifact_size_bytes="$(docker exec "$container" stat -c '%s' "$artifact")"
  artifact_size_mib="$(awk -v bytes="$artifact_size_bytes" 'BEGIN { printf "%.6f", bytes / 1048576 }')"

  : > "$directory/memory"
  for _ in $(seq 1 12); do
    docker stats --no-stream --format '{{.MemUsage}}' "$container" 2>/dev/null \
      | awk -F/ '{gsub(/[[:space:]]/, "", $1); print $1}' \
      | while read -r sample; do memory_to_mib "$sample"; echo; done >> "$directory/memory"
    sleep .25
  done
  local idle_memory_mib
  idle_memory_mib="$(median_file "$directory/memory")"

  jq -n \
    --arg id "$id" \
    --arg label "$label" \
    --arg image "$image" \
    --arg artifact "$artifact" \
    --arg responseSha256 "$response_sha" \
    --argjson responseBytes "$response_bytes" \
    --argjson imageSizeMiB "$image_size_mib" \
    --argjson artifactSizeMiB "$artifact_size_mib" \
    --argjson idleMemoryMiB "$idle_memory_mib" \
    '{id: $id, label: $label, image: $image, artifact: $artifact, responseBytes: $responseBytes, responseSha256: $responseSha256, imageSizeMiB: $imageSizeMiB, artifactSizeMiB: $artifactSizeMiB, idleMemoryMiB: $idleMemoryMiB}' \
    > "$directory/component.json"

  docker rm -f "$container" >/dev/null
}

measure_component hoplite Hoplite "$hoplite_image" /usr/local/bin/hoplite
measure_component nginx "Plain Nginx" "$nginx_image" /opt/nginx/sbin/nginx
measure_component java "Java application" "$java_image" /app/app.jar
measure_component python "Python application" "$python_image" /app/server.py
measure_component lua "Nginx + Lua" "$lua_image" /usr/sbin/nginx

expected_sha="$(jq -r '.responseSha256' "$work/hoplite/component.json")"
for component in nginx java python lua; do
  actual_sha="$(jq -r '.responseSha256' "$work/$component/component.json")"
  if [[ "$actual_sha" != "$expected_sha" ]]; then
    echo "$component returned a different response body" >&2
    exit 1
  fi
done

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
  --argjson logicalCpus "$logical_cpus" \
  --slurpfile hoplite "$work/hoplite/component.json" \
  --slurpfile nginx "$work/nginx/component.json" \
  --slurpfile java "$work/java/component.json" \
  --slurpfile python "$work/python/component.json" \
  --slurpfile lua "$work/lua/component.json" '
  {
    status: "measured",
    generatedAt: $generatedAt,
    commit: $commit,
    runner: $runner,
    cpu: $cpu,
    logicalCpus: $logicalCpus,
    methodology: {
      idleSamplesPerComponent: 12,
      payloadBytes: $hoplite[0].responseBytes,
      imageAccounting: "Logical Docker image sizes are summed for multi-service stacks; shared registry layers may deduplicate in practice.",
      scope: "Minimal representative services returning the same response; these are footprint samples, not full framework applications."
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
        label: "Hoplite",
        components: ["Hoplite"],
        serviceCount: 1,
        deploymentImageMiB: $hoplite[0].imageSizeMiB,
        idleMemoryMiB: $hoplite[0].idleMemoryMiB,
        primaryArtifactMiB: $hoplite[0].artifactSizeMiB
      },
      java: {
        label: "Java",
        components: ["Plain Nginx", "Java application"],
        serviceCount: 2,
        deploymentImageMiB: ($nginx[0].imageSizeMiB + $java[0].imageSizeMiB),
        idleMemoryMiB: ($nginx[0].idleMemoryMiB + $java[0].idleMemoryMiB),
        primaryArtifactMiB: $java[0].artifactSizeMiB
      },
      python: {
        label: "Python",
        components: ["Plain Nginx", "Python application"],
        serviceCount: 2,
        deploymentImageMiB: ($nginx[0].imageSizeMiB + $python[0].imageSizeMiB),
        idleMemoryMiB: ($nginx[0].idleMemoryMiB + $python[0].idleMemoryMiB),
        primaryArtifactMiB: $python[0].artifactSizeMiB
      },
      lua: {
        label: "Lua / Nginx",
        components: ["Nginx + Lua"],
        serviceCount: 1,
        deploymentImageMiB: $lua[0].imageSizeMiB,
        idleMemoryMiB: $lua[0].idleMemoryMiB,
        primaryArtifactMiB: $lua[0].artifactSizeMiB
      }
    }
  }' > "$output"

cat "$output"
