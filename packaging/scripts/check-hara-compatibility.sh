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

expected_hara="$(bash packaging/scripts/hara-revision.sh)"
expected_hara_native="$(bash packaging/scripts/hara-native-revision.sh)"
hara_root="${HARA_ROOT:-../hara}"
hara_native_root="${HARA_NATIVE_ROOT:-../hara-native}"
allow_dirty_hara_native="${HOPLITE_ALLOW_DIRTY_HARA_NATIVE:-0}"

if [[ "$allow_dirty_hara_native" != "0" && "$allow_dirty_hara_native" != "1" ]]; then
  printf 'hara-compatibility: HOPLITE_ALLOW_DIRTY_HARA_NATIVE must be 0 or 1\n' >&2
  exit 64
fi

if [[ "$allow_dirty_hara_native" == "1" && ("${CI:-}" == "true" || "${GITHUB_ACTIONS:-}" == "true") ]]; then
  printf 'hara-compatibility: CI and release builds require a clean Hara Native checkout\n' >&2
  exit 65
fi

if ! git -C "$hara_root" rev-parse --git-dir >/dev/null 2>&1; then
  printf 'hara-compatibility: Hara checkout is unavailable at %s\n' "$hara_root" >&2
  printf 'hara-compatibility: expected-hara=%s\n' "$expected_hara" >&2
  exit 66
fi

if ! git -C "$hara_native_root" rev-parse --git-dir >/dev/null 2>&1; then
  printf 'hara-compatibility: Hara Native checkout is unavailable at %s\n' "$hara_native_root" >&2
  printf 'hara-compatibility: expected-hara-native=%s\n' "$expected_hara_native" >&2
  exit 66
fi

actual_hara="$(git -C "$hara_root" rev-parse HEAD)"
actual_hara_native="$(git -C "$hara_native_root" rev-parse HEAD)"
hoplite="$(git rev-parse HEAD 2>/dev/null || printf 'unknown')"
printf 'hara-compatibility: hoplite=%s\n' "$hoplite"
printf 'hara-compatibility: expected-hara=%s\n' "$expected_hara"
printf 'hara-compatibility: actual-hara=%s\n' "$actual_hara"
printf 'hara-compatibility: expected-hara-native=%s\n' "$expected_hara_native"
printf 'hara-compatibility: actual-hara-native=%s\n' "$actual_hara_native"

if [[ "$actual_hara" != "$expected_hara" ]]; then
  printf 'hara-compatibility: checked-out Hara revision does not match packaging/hara-revision\n' >&2
  exit 65
fi

if [[ "$actual_hara_native" != "$expected_hara_native" ]]; then
  printf 'hara-compatibility: checked-out Hara Native revision does not match packaging/hara-native-revision\n' >&2
  exit 65
fi

if [[ -n "$(git -C "$hara_root" status --porcelain)" ]]; then
  printf 'hara-compatibility: Hara source checkout is dirty; build and release inputs must be committed\n' >&2
  exit 65
fi

if [[ -n "$(git -C "$hara_native_root" status --porcelain)" ]]; then
  if [[ "$allow_dirty_hara_native" != "1" ]]; then
    printf 'hara-compatibility: Hara Native checkout is dirty; set HOPLITE_ALLOW_DIRTY_HARA_NATIVE=1 only for local development\n' >&2
    exit 65
  fi
  printf 'hara-compatibility: allowing a dirty local Hara Native checkout\n'
fi

if [[ -f "$hara_root/core/rust/hal-src/std/foundation.hal" ]]; then
  hara_foundation_source="$hara_root/core/rust/hal-src/std/foundation.hal"
elif [[ -f "$hara_root/core/lib/src/std/foundation.hal" ]]; then
  hara_foundation_source="$hara_root/core/lib/src/std/foundation.hal"
else
  printf 'hara-compatibility: Hara source checkout has no std/foundation.hal input\n' >&2
  exit 66
fi

for required in \
  "$hara_foundation_source" \
  "$hara_native_root/core/rust/Cargo.toml" \
  "$hara_native_root/core/rust/src/lib.rs" \
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
  shift 2
  local -a target_args=("$@")
  local listing
  local -a matches=()

  listing="$(cargo test \
    --manifest-path "$manifest" \
    --locked "${target_args[@]}" "$filter" -- --list)"
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
    --locked "${target_args[@]}" "${matches[0]}" -- --exact
}

run_focused_test \
  core/Cargo.toml \
  app_sources_evaluate_and_preserve_handler_vars \
  --bin hoplite
run_focused_test \
  core/runtime/Cargo.toml \
  request_adapter_retains_and_pulls_hara_stream_bodies \
  --lib
run_focused_test \
  core/runtime/Cargo.toml \
  request_v3_body_survives_async_work_and_closes_with_request_scope \
  --lib

printf 'hara-compatibility: focused embedding boundary passed\n'
