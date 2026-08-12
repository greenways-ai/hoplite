#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CONTEXT="$(cd "$ROOT/.." && pwd)"
DOCKERFILE="$ROOT/packaging/docker/Dockerfile"
APP="${1:-packaging/fixtures/multi-module}"
WORK="$(mktemp -d)"
FIRST="$WORK/first"
SECOND="$WORK/second"
SEED="${HOPLITE_REPRODUCIBILITY_SEED:-$(date -u +%s%N)-$$}"
trap 'rm -rf "$WORK"' EXIT

if [[ ! -d "$CONTEXT/hara" || ! -d "$CONTEXT/hoplite" ]]; then
  echo "reproducibility check requires sibling hoplite and hara checkouts" >&2
  exit 1
fi

build_application() {
  local nonce=$1
  local output=$2

  mkdir -p "$output"
  docker build \
    --file "$DOCKERFILE" \
    --target application-artifacts \
    --build-arg "HOPLITE_APP=$APP" \
    --build-arg "HOPLITE_REPRODUCIBILITY_NONCE=$nonce" \
    --output "type=local,dest=$output" \
    "$CONTEXT"
}

build_application "$SEED-first" "$FIRST"
build_application "$SEED-second" "$SECOND"

for output in "$FIRST" "$SECOND"; do
  for required in app.hbx apps.hta conf/nginx.conf; do
    test -f "$output/$required" || {
      echo "generated application is missing $required" >&2
      exit 1
    }
  done

  if find "$output" -type f \
      \( -name '*.hal' -o -name project.edn -o -name hara.extension.edn \) \
      -print -quit | grep -q .; then
    echo "generated application artifact contains source input" >&2
    find "$output" -type f \
      \( -name '*.hal' -o -name project.edn -o -name hara.extension.edn \) \
      -print >&2
    exit 1
  fi
done

(
  cd "$FIRST"
  find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum
) > "$WORK/first.sha256"
(
  cd "$SECOND"
  find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum
) > "$WORK/second.sha256"

diff -u "$WORK/first.sha256" "$WORK/second.sha256"
diff -r --no-dereference "$FIRST" "$SECOND"

printf '%s\n' \
  "independent application builds: byte-identical" \
  "application fixture: $APP" \
  "generated files: $(wc -l < "$WORK/first.sha256" | tr -d '[:space:]')"
cat "$WORK/first.sha256"
