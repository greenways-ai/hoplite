#!/usr/bin/env bash
set -euo pipefail

image="${1:-hoplite-ci}"
key="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
key="${key//[^A-Za-z0-9_.-]/-}"
source_container="hoplite-startup-source-$key"
failed_container="hoplite-startup-failed-$key"
work="$(mktemp -d "${TMPDIR:-/tmp}/hoplite-startup.XXXXXX")"

cleanup() {
  docker rm -f "$source_container" "$failed_container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

docker create --name "$source_container" "$image" >/dev/null
docker cp "$source_container:/app/.hoplite/app.hbx" "$work/app.hbx"
size="$(wc -c < "$work/app.hbx" | tr -d '[:space:]')"
if [[ ! "$size" =~ ^[1-9][0-9]*$ ]]; then
  echo "Could not read the production HAB0 bundle" >&2
  exit 1
fi
checksum_byte="$(od -An -tu1 -j 4 -N 1 "$work/app.hbx" | tr -d '[:space:]')"
replacement=0
if [[ "$checksum_byte" == 0 ]]; then replacement=1; fi
printf "\\$(printf '%03o' "$replacement")" \
  | dd of="$work/app.hbx" bs=1 seek=4 conv=notrunc status=none

docker create \
  --name "$failed_container" \
  --mount "type=bind,src=$work/app.hbx,dst=/app/.hoplite/app.hbx,readonly" \
  "$image" >/dev/null
if docker start --attach "$failed_container" >"$work/stdout" 2>"$work/stderr"; then
  echo "Tampered HAB0 unexpectedly became ready" >&2
  exit 1
fi
docker cp "$failed_container:/app/.hoplite/error.log" "$work/error.log" >/dev/null 2>&1 || true

grep -F '"sequence":1,"stage":"configuration","status":"ok"' "$work/error.log" >/dev/null
grep -F '"sequence":2,"stage":"bundle","status":"failed","class":"application-bundle-checksum-mismatch"' "$work/error.log" >/dev/null
if grep -Eq '"stage":"(modules|routes|readiness)"' "$work/error.log"; then
  echo "Tampered HAB0 emitted a startup stage after bundle failure" >&2
  cat "$work/error.log" >&2
  exit 1
fi
if grep -Eq '(/Users/|/home/|0x[0-9a-fA-F]+)' "$work/error.log"; then
  echo "Startup diagnostics leaked a path or native pointer" >&2
  cat "$work/error.log" >&2
  exit 1
fi

echo "Validated fail-stopped path-free startup diagnostics through $image"
