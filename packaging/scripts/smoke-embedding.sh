#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

set +e
bash "$ROOT/.github/scripts/smoke-embedding-impl.sh" > "$WORK/output.log" 2>&1
status=$?
set -e

cat "$WORK/output.log"
if [[ "$status" -ne 0 ]]; then
  tail -c 12000 "$WORK/output.log" > "$WORK/output.tail"
  encoded="$(base64 -w0 "$WORK/output.tail")"
  echo "::error file=packaging/scripts/smoke-embedding.sh,title=embedding-log-base64::$encoded"
fi
exit "$status"
