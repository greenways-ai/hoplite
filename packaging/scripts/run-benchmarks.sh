#!/usr/bin/env bash
set -euo pipefail

script_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "$script_root/../.." && pwd)"
workspace_root="$(dirname "$repository_root")"
hara_root="${HARA_ROOT:-$workspace_root/hara}"
output_argument="${1:-benchmark-output}"
if [[ "$output_argument" = /* ]]; then
  output_root="$output_argument"
else
  output_root="$repository_root/$output_argument"
fi

for command in docker git jq bash; do
  command -v "$command" >/dev/null || {
    echo "Missing benchmark dependency: $command" >&2
    exit 1
  }
done

if [[ ! -f "$hara_root/core/rust/Cargo.toml" ]]; then
  cat >&2 <<MESSAGE
Hara is required as a sibling checkout at:
  $hara_root

Create or select a sibling Hara checkout with:
  git clone https://github.com/hara-lang/hara.git "$hara_root"
  git -C "$hara_root" checkout "$(tr -d '[:space:]' < "$repository_root/packaging/hara-revision")"
MESSAGE
  exit 1
fi

expected_hara_revision="$(tr -d '[:space:]' < "$repository_root/packaging/hara-revision")"
actual_hara_revision="$(git -C "$hara_root" rev-parse HEAD)"
if [[ "$actual_hara_revision" != "$expected_hara_revision" ]]; then
  echo "Hara checkout is $actual_hara_revision; expected $expected_hara_revision" >&2
  echo "Run: git -C '$hara_root' checkout '$expected_hara_revision'" >&2
  exit 1
fi

hoplite_revision="$(git -C "$repository_root" rev-parse HEAD)"
nginx_version="$(sed -n 's/^NGINX_VERSION := //p' "$repository_root/core/Makefile")"
if [[ -z "$nginx_version" ]]; then
  echo "Could not resolve Nginx version from core/Makefile" >&2
  exit 1
fi

run_key="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
run_key="${run_key//[^A-Za-z0-9_.-]/-}"
image_prefix="hoplite-measure-${run_key}"
hoplite_image="${image_prefix}-server"
nginx_image="${image_prefix}-nginx"
java_image="${image_prefix}-java"
python_image="${image_prefix}-python"
lua_image="${image_prefix}-lua"
images=("$hoplite_image" "$nginx_image" "$java_image" "$python_image" "$lua_image")

cleanup() {
  if [[ "${HOPLITE_BENCHMARK_KEEP_IMAGES:-false}" == "true" ]]; then
    printf 'Keeping benchmark images:\n'
    printf '  %s\n' "${images[@]}"
    return
  fi
  docker image rm -f "${images[@]}" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

mkdir -p "$output_root"
rm -f "$output_root/http-benchmark.json" "$output_root/stack-footprints.json"

printf 'Building Hoplite production image...\n'
docker build \
  -f "$repository_root/packaging/docker/Dockerfile" \
  -t "$hoplite_image" \
  "$workspace_root"

printf 'Building same-toolchain Nginx baseline...\n'
docker build \
  -f "$repository_root/packaging/docker/Dockerfile.nginx-baseline" \
  -t "$nginx_image" \
  "$repository_root"

printf 'Building deployment-envelope samples...\n'
docker build -f "$repository_root/packaging/docker/footprints/java/Dockerfile" -t "$java_image" "$repository_root"
docker build -f "$repository_root/packaging/docker/footprints/python/Dockerfile" -t "$python_image" "$repository_root"
docker build -f "$repository_root/packaging/docker/footprints/lua/Dockerfile" -t "$lua_image" "$repository_root"

export HOPLITE_REPOSITORY_ROOT="$repository_root"
export HOPLITE_BENCHMARK_COMMIT="$hoplite_revision"
export HOPLITE_HARA_REVISION="$expected_hara_revision"
export HOPLITE_NGINX_VERSION="$nginx_version"

(
  cd "$repository_root"
  bash packaging/scripts/benchmark-http.sh \
    "$hoplite_image" \
    "$nginx_image" \
    "$output_root/http-benchmark.json"
  bash packaging/scripts/measure-stack-footprints.sh \
    "$hoplite_image" \
    "$nginx_image" \
    "$java_image" \
    "$python_image" \
    "$lua_image" \
    "$output_root/stack-footprints.json"
  bash packaging/scripts/validate-benchmark-data.sh \
    "$output_root/http-benchmark.json" \
    "$output_root/stack-footprints.json"
)

jq -n \
  --slurpfile http "$output_root/http-benchmark.json" \
  --slurpfile footprint "$output_root/stack-footprints.json" '
  {
    commit: $http[0].commit,
    generatedAt: $http[0].generatedAt,
    requestsPerSecond: {
      hoplite: $http[0].targets.hoplite.metrics.requestsPerSecond,
      nginx: $http[0].targets.nginx.metrics.requestsPerSecond,
      percentOfNginx: $http[0].comparison.throughputPercentOfNginx
    },
    hoplite: {
      executableMiB: $http[0].targets.hoplite.executableSizeMiB,
      imageMiB: $http[0].targets.hoplite.imageSizeMiB,
      idleMemoryMiB: $http[0].targets.hoplite.idleMemoryMiB,
      processCount: $http[0].targets.hoplite.processCount
    },
    footprintSamples: ($footprint[0].stacks | keys)
  }'

printf 'Benchmark reports written to %s\n' "$output_root"
