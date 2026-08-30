#!/usr/bin/env bash
set -euo pipefail

binary="${1:?usage: verify-production-link.sh NGINX-BINARY}"
test -f "$binary"
symbols="$(mktemp)"
exports="$(mktemp)"
trap 'rm -f "$symbols" "$exports"' EXIT
# The production boundary is the set of symbols a native module can resolve.
# Archive-local Rust implementation names are not ABI: the linker deliberately
# hides them with --exclude-libs,ALL so section collection can decide their
# residency independently.  Inspect global symbols only; inspecting every
# private symbol makes the check fail before the binary is stripped without
# proving an externally reachable compiler entry point.
nm "$binary" >"$symbols"
nm -g "$binary" >"$exports"

for forbidden in \
  hoplite_bootstrap_modules \
  hoplite_work_start \
  compile_source \
  compile_bytecode_artifact \
  eval_text_mode; do
  if grep -F "$forbidden" "$exports" >/dev/null; then
    echo "production Nginx exports source/compiler authority: $forbidden" >&2
    exit 1
  fi
done

for required in \
  hoplite_bootstrap_application_files_v2 \
  hoplite_app_invoke_v4 \
  hoplite_work_poll; do
  grep -F "$required" "$symbols" >/dev/null || {
    echo "production Nginx link is missing runtime symbol: $required" >&2
    exit 1
  }
done

echo "production Nginx exports bytecode serving authority without source compilation entry points"
