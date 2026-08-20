#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

forbidden='HAB1|HBB2|hoplite\.application-bundle/1|hoplite\.public-surfaces/1'
matches="$(
  git grep -n -E "$forbidden" -- \
    . \
    ':!packaging/scripts/verify-alpha-versioning.sh' \
    || true
)"
if [[ -n "$matches" ]]; then
  printf '%s\n' "$matches" >&2
  echo "stale stable-looking version identifiers remain" >&2
  exit 1
fi

grep -F 'pub const FORMAT: &str = "hoplite.application-bundle/0-alpha";' \
  core/application-bundle/src/lib.rs >/dev/null
grep -F 'pub const MAGIC: &[u8; 4] = b"HAB0";' \
  core/application-bundle/src/lib.rs >/dev/null
grep -F 'pub const HARA_BUNDLE_MAGIC: &[u8; 4] = b"HBX0";' \
  core/application-bundle/src/lib.rs >/dev/null
grep -F 'pub const RUNTIME_ABI_VERSION: u32 = 5;' \
  core/application-bundle/src/lib.rs >/dev/null

hara_revision="$(tr -d '[:space:]' < packaging/hara-revision)"
case "$hara_revision" in
  ''|*[!0-9a-f]*)
    echo "invalid Hara revision: $hara_revision" >&2
    exit 1
    ;;
esac
test "${#hara_revision}" -eq 40
for workflow in .github/workflows/ci.yml .github/workflows/cosocket.yml; do
  grep -F "revision=\"\$(tr -d '[:space:]' < packaging/hara-revision)\"" \
    "$workflow" >/dev/null
  grep -F 'ref: ${{ steps.hara.outputs.revision }}' "$workflow" >/dev/null
done
if git grep -n -E 'HARA_REF:[[:space:]]+[0-9a-f]{40}' -- .github/workflows; then
  echo "workflow duplicates the canonical packaging/hara-revision pin" >&2
  exit 1
fi

grep -F 'docs/versioning.md' README.md >/dev/null
grep -F 'hoplite.application-bundle/0-alpha' docs/application-bundle.md >/dev/null
grep -F 'Hara bytecode bundle | `HBX0`' docs/versioning.md >/dev/null
grep -F 'hoplite_bootstrap_application_v2' core/nginx/hoplite_runtime.h >/dev/null
grep -F 'hoplite_handler_invoke_v4' core/nginx/hoplite_runtime.h >/dev/null
grep -F 'hoplite_app_invoke_v4' core/nginx/hoplite_runtime.h >/dev/null
grep -F 'hoplite_abi_version() < 5' \
  core/nginx/ngx_http_hoplite_module.c >/dev/null
grep -F 'hoplite.startup-diagnostic/0-alpha' docs/startup-diagnostics.md >/dev/null
grep -F 'hoplite.runtime-measurement/0-alpha' docs/runtime-measurements.md >/dev/null

echo "alpha version policy verified"
