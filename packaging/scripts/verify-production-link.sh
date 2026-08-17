#!/usr/bin/env bash
set -euo pipefail

binary="${1:?usage: verify-production-link.sh NGINX-BINARY}"
test -f "$binary"
symbols="$(mktemp)"
trap 'rm -f "$symbols"' EXIT
nm "$binary" >"$symbols"

for forbidden in \
  hoplite_bootstrap_modules \
  hoplite_work_start \
  compile_source \
  compile_bytecode_artifact \
  eval_text_mode; do
  if grep -F "$forbidden" "$symbols" >/dev/null; then
    echo "production Nginx link retains source/compiler authority: $forbidden" >&2
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

echo "production Nginx link exposes bytecode serving authority without source compilation entry points"
