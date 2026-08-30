#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
revision_file="$root/packaging/hara-native-revision"

if [[ ! -f "$revision_file" ]]; then
  printf 'hara-native-revision: missing %s\n' "$revision_file" >&2
  exit 66
fi

revision="$(tr -d '[:space:]' < "$revision_file")"
if [[ ! "$revision" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'hara-native-revision: expected a 40-character lowercase Git SHA in %s\n' "$revision_file" >&2
  exit 65
fi

printf '%s\n' "$revision"
