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

core_test="app::tests::app_sources_evaluate_and_preserve_handler_vars"
core_listing="$(cargo test \
  --manifest-path core/Cargo.toml \
  --locked --lib -- --list)"
grep -Fqx "$core_test: test" <<<"$core_listing" || {
  printf 'hara-compatibility: missing focused test %s\n' "$core_test" >&2
  exit 70
}
cargo test \
  --manifest-path core/Cargo.toml \
  --locked --lib "$core_test" -- --exact

runtime_tests=(
  "tests::request_adapter_retains_and_pulls_hara_stream_bodies"
  "tests::request_v3_body_survives_async_work_and_closes_with_request_scope"
)
runtime_listing="$(cargo test \
  --manifest-path core/runtime/Cargo.toml \
  --locked --lib -- --list)"
for test_name in "${runtime_tests[@]}"; do
  grep -Fqx "$test_name: test" <<<"$runtime_listing" || {
    printf 'hara-compatibility: missing focused test %s\n' "$test_name" >&2
    exit 70
  }
  cargo test \
    --manifest-path core/runtime/Cargo.toml \
    --locked --lib "$test_name" -- --exact
done

printf 'hara-compatibility: focused embedding boundary passed\n'
