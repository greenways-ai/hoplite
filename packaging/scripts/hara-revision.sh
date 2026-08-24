#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
revision_file="${HOPLITE_HARA_REVISION_FILE:-$root/packaging/hara-revision}"

if [[ ! -f "$revision_file" ]]; then
  printf 'hara-revision: missing revision file: %s\n' "$revision_file" >&2
  exit 66
fi

revision="$(tr -d '[:space:]' < "$revision_file")"
if [[ ! "$revision" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'hara-revision: expected one complete lowercase commit SHA in %s\n' \
    "$revision_file" >&2
  exit 65
fi

printf '%s\n' "$revision"
