#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

mode="${1:---run}"
case "$mode" in
  --run|--verify-only) ;;
  *)
    printf 'usage: packaging/scripts/check-hara-compatibility.sh [--run|--verify-only]\n' >&2
    exit 64
    ;;
esac

expected="$(bash packaging/scripts/hara-revision.sh)"
hara_root="${HARA_ROOT:-../hara}"

if ! git -C "$hara_root" rev-parse --git-dir >/dev/null 2>&1; then
  printf 'hara-compatibility: Hara checkout is unavailable at %s\n' "$hara_root" >&2
  printf 'hara-compatibility: expected=%s\n' "$expected" >&2
  exit 66
fi

actual="$(git -C "$hara_root" rev-parse HEAD)"
hoplite="$(git rev-parse HEAD 2>/dev/null || printf 'unknown')"
printf 'hara-compatibility: hoplite=%s\n' "$hoplite"
printf 'hara-compatibility: expected-hara=%s\n' "$expected"
printf 'hara-compatibility: actual-hara=%s\n' "$actual"

if [[ "$actual" != "$expected" ]]; then
  printf 'hara-compatibility: checked-out Hara revision does not match packaging/hara-revision\n' >&2
  exit 65
fi

for required in \
  "$hara_root/core/rust/Cargo.toml" \
  "$hara_root/core/rust/src/lib.rs" \
  core/Cargo.toml \
  core/runtime/Cargo.toml; do
  if [[ ! -f "$required" ]]; then
    printf 'hara-compatibility: missing required embedding input: %s\n' "$required" >&2
    exit 66
  fi
done

if [[ "$mode" == "--verify-only" ]]; then
  printf 'hara-compatibility: revision boundary verified\n'
  exit 0
fi

cargo metadata \
  --manifest-path core/Cargo.toml \
  --locked --no-deps >/dev/null

cargo check \
  --manifest-path core/Cargo.toml \
  --workspace --locked

run_focused_test() {
  local manifest="$1"
  local filter="$2"
  local listing
  local -a matches=()

  listing="$(cargo test \
    --manifest-path "$manifest" \
    --locked --lib "$filter" -- --list)"
  mapfile -t matches < <(
    printf '%s\n' "$listing" | sed -n 's/: test$//p'
  )

  if [[ "${#matches[@]}" -ne 1 ]]; then
    printf 'hara-compatibility: expected exactly one focused test for %s in %s; found %s\n' \
      "$filter" "$manifest" "${#matches[@]}" >&2
    printf '%s\n' "$listing" >&2
    exit 70
  fi

  printf 'hara-compatibility: running %s\n' "${matches[0]}"
  cargo test \
    --manifest-path "$manifest" \
    --locked --lib "${matches[0]}" -- --exact
}

run_focused_test \
  core/Cargo.toml \
  app_sources_evaluate_and_preserve_handler_vars
run_focused_test \
  core/runtime/Cargo.toml \
  request_adapter_retains_and_pulls_hara_stream_bodies
run_focused_test \
  core/runtime/Cargo.toml \
  request_v3_body_survives_async_work_and_closes_with_request_scope

printf 'hara-compatibility: focused embedding boundary passed\n'
